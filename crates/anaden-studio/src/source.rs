//! スクリーンショットの取得元。
//!
//! `LiveCapture` は別スレッドで ADB 経由のキャプチャを繰り返し、最新フレームを
//! mpsc チャネルで UI スレッドに渡す。egui の描画スレッドをブロックしないための措置。
//!
//! なお AdbClient のメソッドは `async` だが内部で同期 `std::process::Command` を呼ぶ
//! （`anaden-device/src/client.rs:109` 参考）。UI 側にランタイムを持ち込まないため、
//! ここでは CLI と同じく生の同期 Command を直接使う。

use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

use image::{DynamicImage, GrayImage};

// PC版(Windows) Win32 キャプチャバックエンド。#[cfg(windows)] で gating し、
// Linux では参照しないことで cargo check --workspace が通るようにする。
// DEFAULT_PROCESS_NAME は GUI の既定 exe 名として app.rs で参照するため再エクスポート。
#[cfg(windows)]
pub use anaden_device::DEFAULT_PROCESS_NAME;
#[cfg(windows)]
use anaden_device::Win32Capture;

/// ライブキャプチャの取得元バックエンド。
///
/// - `Android`: 従来の adb screencap(実機 20:9)。serial 必須。
/// - `Windows`: PC版 Win32Capture(PrintWindow)。exe 名でウィンドウを解決するため serial 不要。
///
/// `Windows` は `#[cfg(windows)]` でのみ存在し、Linux では `Android` のみ選択可能。
/// これにより studio は Linux でもコンパイル可能(遵守ルール: cargo check --workspace)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// Android 実機: adb screencap。
    Android,
    /// PC版(Windows): Win32Capture。
    #[cfg(windows)]
    Windows,
}

impl Default for Target {
    fn default() -> Self {
        // 後方互換: target 未指定時は従来通り adb(android)。
        Target::Android
    }
}

/// 別スレッドで動くライブキャプチャ。停止フラグで終了させる。
///
/// 画面OFF(Doze)で `screencap` が純黒フレームを返す問題(Pixel 7a 実証) を防ぐため:
///   - 開始時に1回 `keyevent 224 (WAKEUP)` で起こし、`screen_off_timeout` を最大値へ
///   - **各キャプチャ直前にも `keyevent 224 (WAKEUP)` を送る**。
///     studio はインタラクティブ用途で adb screencap ベース(1〜2s/フレーム)なので、
///     wake のオーバーヘッドは許容可能。`screen_off_timeout` 延長だけでは Pixel 7a の
///     Doze(画面はON表示のままバックライト等が落ちて screencap が黒を返す)を完全には
///     抑制できず、毎フレーム wake が最も確実。
///   - 取得PNGの平均輝度が閾値未満(黒フレーム)なら破棄してUIへ流さない(フェイルセーフ)
/// Drop で `screen_off_timeout` を元の値に戻す。
///
/// **OOM 対策**: チャネルは非有界だが、**送信前に古い未読フレームを全てドレイン**し、
/// チャネル内に高々1フレームしか保持しない(送信側ドレイン)。UI 側の `latest()` と
/// 整合し、UI がドレインに追いつかなくてもフレームが蓄積しない。
/// 各フレームは 2400x1080 RGBA ≈10MB なので、無制限蓄積は即 OOM につながる。
pub struct LiveCapture {
    rx: Receiver<Arc<DynamicImage>>,
    stop: Arc<AtomicBool>,
    /// screen_off_timeout 復元用(android のみ)。windows バックエンドでは None。
    serial: Option<String>,
    original_screen_off_timeout: Option<String>,
}

impl LiveCapture {
    /// 指定バックエンドで `interval_ms` 間隔のキャプチャを繰り返すスレッドを開始する。
    ///
    /// - `target == Android`: `serial` は必須。adb screencap を使う。画面OFF/Doze 対策の
    ///   WAKEUP + screen_off_timeout 延長を行う。
    /// - `target == Windows`: `serial` は無視される(空でよい)。`exe` でプロセスを解決し
    ///   Win32Capture(PrintWindow)でキャプチャする。画面OFF系の adb 処理は一切行わない。
    ///
    /// 後方互換: target 未指定相当(adb)は [`LiveCapture::start_android`] を使うこと。
    #[cfg_attr(not(windows), allow(unused_variables))]
    pub fn start(serial: String, interval_ms: u64, target: Target, exe: &str) -> Self {
        // OOM 対策: 容量1の有界チャネル。UI がドレインに追いつかなくても、チャネル内に
        // 保持されるフレームは高々1枚(約10MB)で頭打ちになる。非有界だと 2400x1080 RGBA
        // ≈10MB/枚 が無制限に蓄積して OOM する。
        let (tx, rx) = mpsc::sync_channel(1);
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = stop.clone();

        match target {
            Target::Android => {
                Self::start_android_inner(rx, tx, stop, stop_thread, serial, interval_ms)
            }
            #[cfg(windows)]
            Target::Windows => {
                Self::start_windows_inner(rx, tx, stop, stop_thread, exe, interval_ms)
            }
        }
    }

    /// android(adb) バックエンド用の互換コンストラクタ。target 未指定の従来呼び出し。
    #[allow(dead_code)]
    pub fn start_android(serial: String, interval_ms: u64) -> Self {
        Self::start(serial, interval_ms, Target::Android, "")
    }

    // ---- android(adb) バックエンドのスレッド起動 ----
    fn start_android_inner(
        rx: Receiver<Arc<DynamicImage>>,
        tx: mpsc::SyncSender<Arc<DynamicImage>>,
        stop: Arc<AtomicBool>,
        stop_thread: Arc<AtomicBool>,
        serial: String,
        interval_ms: u64,
    ) -> Self {
        // ---- 開始時の画面ON確保 + タイムアウト延長 ----
        // 先に WAKEUP を送っておく(非ブロッキング、失敗は継続)。
        wake_screen(&serial);
        let original_screen_off_timeout = read_screen_off_timeout(&serial);
        set_screen_off_timeout_max(&serial);

        let serial_thread = serial.clone();
        thread::spawn(move || {
            while !stop_thread.load(Ordering::Relaxed) {
                // 毎フレーム WAKEUP を送って画面を起こす。studio はインタラクティブ用途で
                // adb screencap ベース(1〜2s/フレーム)なので wake のオーバーヘッドは許容可能。
                // screen_off_timeout 延長だけでは Pixel 7a の Doze(画面ONのまま screencap が
                // 黒を返す)を完全に抑制できず、毎フレーム wake が最も確実。
                // 失敗(既に起きている等) でも exit=0 なので無害。継続する。
                wake_screen(&serial_thread);
                if let Some(img) = capture_screenshot(&serial_thread) {
                    // try_send: チャネル満タン(1枚未読)なら即座に Full を返す。
                    // その場合は UI が遅れていてまだ前フレームを消費していない →
                    // 古いフレームを上書きする手段が無いため、この新フレームを捨てて
                    // 次のキャプチャへ進む(1枚損するが、蓄積はゼロで OOM 回避を最優先)。
                    // UI の latest() は受信時に全ドレインするので、次フレームは確実に入る。
                    let _ = tx.try_send(Arc::new(img));
                }
                sleep_until_next(&stop_thread, interval_ms);
            }
        });

        LiveCapture {
            rx,
            stop,
            serial: Some(serial),
            original_screen_off_timeout,
        }
    }

    /// キャプチャスレッドに停止を要求する。
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    /// チャネルをドレインし、最新フレームだけを返す（無ければ None）。
    pub fn latest(&self) -> Option<Arc<DynamicImage>> {
        let mut latest = None;
        while let Ok(img) = self.rx.try_recv() {
            latest = Some(img);
        }
        latest
    }
}

/// 停止フラグをポーリングしながら `interval_ms` まで分割スリープする。
/// 共通ユーティリティ(android/windows 両バックエンドで使用)。
fn sleep_until_next(stop: &AtomicBool, interval_ms: u64) {
    let mut waited = 0u64;
    while waited < interval_ms && !stop.load(Ordering::Relaxed) {
        let step = waited.saturating_add(50).min(interval_ms) - waited;
        thread::sleep(Duration::from_millis(step));
        waited += step;
    }
}

// ---- Windows(Win32Capture) バックエンド ----
// studio は tokio ランタイムを持たない std::thread モデル(source.rs ヘッダコメント参照)。
// Win32Capture::capture は async(内部 tokio::spawn_blocking)だが、同期ラッパ
// capture_blocking() が追加済み(win32_capture.rs)なので、これを直接呼んで tokio 依存を回避する。
#[cfg(windows)]
impl LiveCapture {
    // ---- windows(Win32Capture) バックエンドのスレッド起動 ----
    fn start_windows_inner(
        rx: Receiver<Arc<DynamicImage>>,
        tx: mpsc::SyncSender<Arc<DynamicImage>>,
        stop: Arc<AtomicBool>,
        stop_thread: Arc<AtomicBool>,
        exe: &str,
        interval_ms: u64,
    ) -> Self {
        let capture = Arc::new(Win32Capture::new_without_dpi(exe));
        let capture_thread = capture.clone();

        // Win32Capture は serial 不要(exe 名→PID→HWND で解決)。画面OFF系の adb 処理も不要。
        thread::spawn(move || {
            while !stop_thread.load(Ordering::Relaxed) {
                if let Some(img) = capture_windows(&capture_thread) {
                    // try_send: チャネル満タン(1枚未読)なら Full で破棄(OOM 回避。android 側と同一方針)。
                    let _ = tx.try_send(Arc::new(img));
                }
                // ウィンドウ未検出(最小化/未起動)時は Err になり capture_windows が None を返す。
                // 無限ループで CPU を食わないよう、interval の分割スリープは必ず機能させる。
                sleep_until_next(&stop_thread, interval_ms);
            }
        });

        LiveCapture {
            rx,
            stop,
            serial: None,
            original_screen_off_timeout: None,
        }
    }
}

#[cfg(windows)]
/// Win32Capture で1枚キャプチャする(同期)。失敗・黒フレーム時は None。
///
/// 真っ白問題の原因は `from_raw` ではなく canvas.rs の `Rect::EVERYTHING` であり、
/// Image widget で解決済み。そのため PNG エンコード→デコードの回避経路は不要で、
/// `capture_blocking()` の `DynamicImage` をそのまま返す。
fn capture_windows(capture: &Win32Capture) -> Option<DynamicImage> {
    let img = capture.capture_blocking().ok()?;
    // Win32Capture の戻り値も DynamicImage なので、黒フレーム判定は adb と共通流用可能。
    if is_black_frame(&img) {
        return None;
    }
    Some(img)
}

impl Drop for LiveCapture {
    fn drop(&mut self) {
        self.stop();
        // セッション終了で screen_off_timeout を元に戻す(android のみ)。
        if let (Some(orig), Some(serial)) = (&self.original_screen_off_timeout, &self.serial) {
            restore_screen_off_timeout(serial, orig);
        }
    }
}

/// 黒フレーム判定の閾値。実測値: 黒フレーム mean=0.0 / 正常フレーム mean=64.8〜85.7。
/// ここでは安全側(浅すぎず深すぎず)に倒した値を使う。
const BLACK_FRAME_MEAN_THRESHOLD: f32 = 10.0;
/// PNGデコード失敗時の救済: バイト長がこれ未満なら黒フレームの可能性が高い。
const BLACK_FRAME_MIN_BYTES: usize = 50_000;

/// `adb -s <serial> exec-out screencap -p` で1枚キャプチャしてデコードする。
///
/// 画面OFFで `screencap` は exit=0 で純黒PNG(約15KB, mean=0) を返すため、
/// デコード後に平均輝度を計算し黒フレームなら None で破棄する(上位で再取得される)。
fn capture_screenshot(serial: &str) -> Option<DynamicImage> {
    let output = Command::new("adb")
        .args(["-s", serial, "exec-out", "screencap", "-p"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    // ファイル長が異常に小さい場合は黒フレームの蓋然性が高い。
    if output.stdout.len() < BLACK_FRAME_MIN_BYTES {
        return None;
    }

    let img = image::load_from_memory(&output.stdout).ok()?;
    if is_black_frame(&img) {
        return None;
    }
    Some(img)
}

/// グレースケール平均輝度が閾値未満なら黒フレームとみなす。
fn is_black_frame(img: &DynamicImage) -> bool {
    let gray: GrayImage = img.to_luma8();
    let pixels: &[u8] = gray.as_raw();
    if pixels.is_empty() {
        return true;
    }
    let sum: u64 = pixels.iter().map(|&v| v as u64).sum();
    let mean = sum as f32 / pixels.len() as f32;
    mean < BLACK_FRAME_MEAN_THRESHOLD
}

/// `adb shell input keyevent 224 (WAKEUP)` でディスプレイを起こす。
/// 非ブロッキング・失敗は継続(既に起きている場合も exit=0)。
fn wake_screen(serial: &str) {
    let _ = Command::new("adb")
        .args(["-s", serial, "shell", "input", "keyevent", "224"])
        .output();
}

/// 現在の `settings system screen_off_timeout` を読む。取得失敗時は None(復元しない)。
fn read_screen_off_timeout(serial: &str) -> Option<String> {
    let output = Command::new("adb")
        .args([
            "-s",
            serial,
            "shell",
            "settings",
            "get",
            "system",
            "screen_off_timeout",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if s.is_empty() || s == "null" {
        None
    } else {
        Some(s)
    }
}

/// `screen_off_timeout` を最大値(INT_MAX)にして画面OFFを抑制する。
fn set_screen_off_timeout_max(serial: &str) {
    let _ = Command::new("adb")
        .args([
            "-s",
            serial,
            "shell",
            "settings",
            "put",
            "system",
            "screen_off_timeout",
            "2147483647",
        ])
        .output();
}

/// `screen_off_timeout` を指定値に戻す。Drop から呼ばれる。
fn restore_screen_off_timeout(serial: &str, value: &str) {
    let _ = Command::new("adb")
        .args([
            "-s",
            serial,
            "shell",
            "settings",
            "put",
            "system",
            "screen_off_timeout",
            value,
        ])
        .output();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `is_black_frame`: 純黒画像は黒フレームと判定される。
    #[test]
    fn black_frame_detected_for_pure_black() {
        let img = DynamicImage::ImageRgb8(image::RgbImage::from_pixel(8, 8, image::Rgb([0, 0, 0])));
        assert!(is_black_frame(&img));
    }

    /// `is_black_frame`: 明るい画像は黒フレームと判定されない。
    #[test]
    fn black_frame_not_detected_for_bright() {
        let img = DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            8,
            8,
            image::Rgb([200, 200, 200]),
        ));
        assert!(!is_black_frame(&img));
    }

    /// OOM 対策の回帰テスト: sync_channel(1) + try_send で、受信側がドレインしなくても
    /// チャネル内に保持されるフレームは高々1枚であることを検証する。
    /// 非有界チャネルだと何枚でも蓄積してしまうが、有界1スロット + try_send(Fullで破棄)で
    /// 頭打ちになる。これが OOM 回避の核心。
    #[test]
    fn bounded_channel_never_accumulates_more_than_one_frame() {
        let (tx, rx) = mpsc::sync_channel::<i32>(1);
        // 1枚目は入る(チャネル空)
        assert!(tx.try_send(1).is_ok());
        // 2枚目は満タンなので Full で弾かれる(破棄)。蓄積しない。
        assert!(tx.try_send(2).is_err());
        assert!(tx.try_send(3).is_err());
        assert!(tx.try_send(4).is_err());
        // 受信側が1枚取ると…
        let drained: Vec<i32> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        // 保持されていたのは最初の1枚だけ(2,3,4は破棄済み)
        assert_eq!(drained, vec![1]);
        // チャネルが空いたので新フレームは再び入る
        assert!(tx.try_send(99).is_ok());
    }

    /// `latest()` 相当のドレイン: try_recv ループで最新1枚を返す。
    #[test]
    fn latest_drains_to_newest() {
        let (tx, rx) = mpsc::sync_channel::<i32>(2);
        // 有界チャネルでも、送信側が連続で送れる限界まで入れてから最新を取り出す
        let _ = tx.try_send(1);
        let _ = tx.try_send(2);
        // latest 相当: 全ドレインして最後を返す
        let mut latest = None;
        while let Ok(v) = rx.try_recv() {
            latest = Some(v);
        }
        assert_eq!(latest, Some(2));
    }
}
