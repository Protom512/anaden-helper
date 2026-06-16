//! スクリーンショットの取得元。
//!
//! `LiveCapture` は別スレッドで ADB 経由のキャプチャを繰り返し、最新フレームを
//! mpsc チャネルで UI スレッドに渡す。egui の描画スレッドをブロックしないための措置。
//!
//! なお AdbClient のメソッドは `async` だが内部で同期 `std::process::Command` を呼ぶ
//! （`anaden-device/src/client.rs:109` 参考）。UI 側にランタイムを持ち込まないため、
//! ここでは CLI と同じく生の同期 Command を直接使う。

use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use image::{DynamicImage, GrayImage};

/// 別スレッドで動くライブキャプチャ。停止フラグで終了させる。
///
/// 画面OFF(Doze)で `screencap` が純黒フレームを返す問題(Pixel 7a 実証) を防ぐため:
///   - 開始時に1回 `keyevent 224 (WAKEUP)` で起こし、`screen_off_timeout` を最大値へ
///   - 取得PNGの平均輝度が閾値未満(黒フレーム)なら破棄してUIへ流さない(フェイルセーフ)
/// Drop で `screen_off_timeout` を元の値に戻す。
///
/// **性能上の注意**: キャプチャ直前の毎フレーム WAKEUP は廃止した。毎回 keyevent を送ると
/// adb 呼び出しが倍増してフレームレートが落ちる。`screen_off_timeout` 延長で黒フレームは
/// 抑制でき、万が一の黒フレームは `is_black_frame` で弾く。
pub struct LiveCapture {
    rx: Receiver<Arc<DynamicImage>>,
    stop: Arc<AtomicBool>,
    serial: String,
    original_screen_off_timeout: Option<String>,
}

impl LiveCapture {
    /// 指定シリアルのデバイスを `interval_ms` 間隔でキャプチャし続けるスレッドを開始する。
    pub fn start(serial: String, interval_ms: u64) -> Self {
        let (tx, rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = stop.clone();

        // ---- 開始時の画面ON確保 + タイムアウト延長 ----
        // 先に WAKEUP を送っておく(非ブロッキング、失敗は継続)。
        wake_screen(&serial);
        let original_screen_off_timeout = read_screen_off_timeout(&serial);
        set_screen_off_timeout_max(&serial);

        let serial_thread = serial.clone();
        thread::spawn(move || {
            while !stop_thread.load(Ordering::Relaxed) {
                // NOTE: キャプチャ直前の WAKEUP は廃止した。毎フレーム keyevent を送ると
                // adb 呼び出しが倍増してフレームレートが落ちる。開始時の screen_off_timeout
                // 延長(上記)で十分に黒フレームを抑制できる。黒フレームガード(is_black_frame)
                // は万が一のフェイルセーフとして残す。
                if let Some(img) = capture_screenshot(&serial_thread) {
                    // チャネルが詰まっても最新は保ちたいが、UI 側で逐次ドレインするので
                    // 送信失敗（受信側なし）は無視する。
                    let _ = tx.send(Arc::new(img));
                }
                // 短いスリープを分割して停止応答を良くする
                let mut waited = 0u64;
                while waited < interval_ms && !stop_thread.load(Ordering::Relaxed) {
                    let step = waited.saturating_add(50).min(interval_ms) - waited;
                    thread::sleep(Duration::from_millis(step));
                    waited += step;
                }
            }
        });

        LiveCapture {
            rx,
            stop,
            serial,
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

impl Drop for LiveCapture {
    fn drop(&mut self) {
        self.stop();
        // セッション終了で screen_off_timeout を元に戻す。
        if let Some(orig) = &self.original_screen_off_timeout {
            restore_screen_off_timeout(&self.serial, orig);
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
