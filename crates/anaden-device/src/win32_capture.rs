#![cfg(windows)]
//! ANOTHER EDEN PC版(Windows)のウィンドウ画像取得バックエンド。
//!
//! `probe_windows_capture.rs` で動作検証済みの PrintWindow(PW_RENDERFULLCONTENT=0x2)
//! + GDI 連鎖(GetDIBits/BGRA→RGBA)をそのまま活かす。BitBlt 単体は cocos2d-x OpenGL
//!   描画を拾えず黒画像になるため使用しない(プローブ実証済み)。
//!
//! 概要:
//! - `Win32Capture::new("AnotherEden.exe")` で生成(DPI アウェア化を冪等に実施)。
//! - `capture()` は FindWindow 相当の手順(exe名→PID→可視トップレベル HWND)で
//!   ウィンドウを特定し、PrintWindow+GetDIBits で RGBA ピクセルを取り出して
//!   `image::DynamicImage` を返す。GDI 同期処理は `spawn_blocking` へ逃す。
//!
//! windows 0.62.2 の API 注意(プローブで解決済み):
//! - `PrintWindow` / `PRINT_WINDOW_FLAGS` は `Win32::Storage::Xps` にある。
//! - `SelectObject` / `DeleteObject` は `HGDIOBJ` を要求 → HBITMAP は `.into()`。
//! - BOOL 返却 GDI 関数は `Result<()>`、HWND/HDC 引数は `Option<HWND>`/`Option<HDC>`。

use std::sync::Mutex;

use image::{DynamicImage, ImageBuffer, RgbaImage};
use windows::Win32::Foundation::{HWND, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleBitmap, CreateCompatibleDC,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits, HDC, HGDIOBJ, SelectObject,
};
// PrintWindow / PRINT_WINDOW_FLAGS は Graphics::Gdi ではなく Storage::Xps にある。
use windows::Win32::Storage::Xps::{PRINT_WINDOW_FLAGS, PrintWindow};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, PROCESS_PER_MONITOR_DPI_AWARE,
    SetProcessDpiAwareness, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetParent, GetWindowThreadProcessId, IsWindowVisible,
};

use crate::client::AdbError;

/// PW_RENDERFULLCONTENT = 0x2 (Win8.1+)。GPU 描画含むフルコンテンツをレンダリング。
const PW_RENDERFULLCONTENT: u32 = 0x2;

/// ANOTHER EDEN PC版プロセスの既定 exe 名。
pub const DEFAULT_PROCESS_NAME: &str = "AnotherEden.exe";

/// PC版 Windows キャプチャバックエンド。
///
/// プロセス名(exe 名)を内包し、`capture()` ごとに exe→PID→可視トップレベル HWND を
/// 特定して PrintWindow+GetDIBits でキャプチャする。HWND はキャッシュするが、
/// 無効化(ウィンドウ再生成等)対策としてキャプチャ失敗時はキャッシュを破棄して再解決する。
///
/// 既存 `ScreenshotCapture` と同じく `image::DynamicImage` を返し、
/// `anaden-engine::pipeline_driver::Capture` trait を満たせるシグネチャとする。
pub struct Win32Capture {
    /// 対象プロセスの exe 名(大文字小文字区別なしで比較)。
    process: String,
    /// 最後に特定した HWND のキャッシュ(ポインタ値を isize で保持)。
    /// HWND は生ポインタを内包し `Send`/`Sync` ではないため、`isize` で持つことで
    /// `Win32Capture` を `Send + Sync`(Capture trait の要件)にする。
    /// 無効化対策で失敗時は None へ戻す。
    cached_hwnd: Mutex<Option<isize>>,
}

impl Win32Capture {
    /// 新しいキャプチャバックエンドを生成する。
    ///
    /// `process_name` には対象 exe 名(例: `"AnotherEden.exe"`)を渡す。空文字列を渡した
    /// 場目は [`DEFAULT_PROCESS_NAME`] を採用する。
    ///
    /// 生成時に DPI アウェア化を行う(冪等)。4K/HiDPI 環境で GetDC/GetClientRect が
    /// 論理ピクセル(DPI スケール後)を返し画像が歪む問題を回避するため、
    /// プロセス全体へ影響するが、プローブと同じく最初に1回呼ぶ。
    pub fn new(process_name: &str) -> Self {
        set_process_dpi_aware();
        let process = if process_name.trim().is_empty() {
            DEFAULT_PROCESS_NAME.to_string()
        } else {
            process_name.to_string()
        };
        Self {
            process,
            cached_hwnd: Mutex::new(None),
        }
    }

    /// DPI アウェア化を行わずに生成。eframe/egui 等、**ホストプロセスが既に
    /// DPI アウェアな場合**に使う。ホスト起動後に `set_process_dpi_aware` を呼ぶと
    /// egui の pixels_per_point と乖離してテクスチャ描画が壊れる(真っ白)ため、
    /// その回避。CLI/プローブなど単体プロセスでは `new` を使うこと。
    pub fn new_without_dpi(process_name: &str) -> Self {
        let process = if process_name.trim().is_empty() {
            DEFAULT_PROCESS_NAME.to_string()
        } else {
            process_name.to_string()
        };
        Self {
            process,
            cached_hwnd: Mutex::new(None),
        }
    }

    /// 既定のプロセス名([`DEFAULT_PROCESS_NAME`])で生成するコンビニエンスコンストラクタ。
    pub fn default_process() -> Self {
        Self::new(DEFAULT_PROCESS_NAME)
    }

    /// 対象ウィンドウのキャプチャ画像を取得する。
    ///
    /// 内部手順(全ステッププローブ検証済み):
    /// 1. キャッシュ済み HWND があればそれを使い、無ければ exe→PID→可視 HWND を解決。
    /// 2. クライアント領域サイズ(GetClientRect)取得。0x0 は最小化扱いでエラー。
    /// 3. PrintWindow(PW_RENDERFULLCONTENT)+GetDIBits で RGBA ピクセル取得。
    /// 4. `ImageBuffer::from_raw` → `DynamicImage::ImageRgba8` へ変換。
    ///
    /// 失敗時は全ステップを `AdbError::CommandFailed { message }` へ包む。
    /// HWND キャッシュが有効だったのにキャプチャ失敗した場合はキャッシュを破棄し、
    /// 次回 capture で再解決させる(ウィンドウ再生成対策)。
    ///
    /// GDI 同期処理は `tokio::task::spawn_blocking` へ逃し、async ランタイムをブロックしない。
    pub async fn capture(&self) -> Result<DynamicImage, AdbError> {
        let process = self.process.clone();
        // キャッシュは isize(HWND のポインタ値)で保持するため、そのままスレッド境界を越えられる。
        // 戻り値には HWND を含めない(画像のみ)。
        let cached_isize = self
            .cached_hwnd
            .lock()
            .map_err(|e| AdbError::CommandFailed {
                message: format!("HWND キャッシュロック失敗: {e}"),
            })?
            .take();

        // ブロッキング内で HWND(を isize 経由で再構築)を解決/使用し、成功時は
        // 使用した HWND のポインタ値を isize で持ち帰る(キャッシュ更新用)。
        let capture_result =
            tokio::task::spawn_blocking(move || resolve_and_capture(&process, cached_isize))
                .await
                .map_err(|e| AdbError::CommandFailed {
                    message: format!("capture spawn_blocking Join 失敗: {e}"),
                })??;

        // 成功時は使用した HWND をキャッシュへ戻す。失敗時は take() 済みで None のまま(次回再解決)。
        let mut guard = self
            .cached_hwnd
            .lock()
            .map_err(|e| AdbError::CommandFailed {
                message: format!("HWND キャッシュロック失敗: {e}"),
            })?;
        let (hwnd_isize, image) = capture_result;
        if let Some(p) = hwnd_isize {
            *guard = Some(p);
        }
        drop(guard);

        Ok(image)
    }

    /// [`Win32Capture::capture`] の同期版。tokio ランタイム無しで呼べる。
    ///
    /// `anaden-studio` のような tokio を持たない std::thread ベースの呼び出し元向け。
    /// 内部手順は [`capture`](Self::capture) と同一(共通の [`resolve_and_capture`] を呼ぶ)。
    /// HWND キャッシュの扱い(HWND 解決→PrintWindow→GetDIBits、失敗時キャッシュ破棄)も同等。
    pub fn capture_blocking(&self) -> Result<DynamicImage, AdbError> {
        let process = self.process.clone();
        let cached_isize = self
            .cached_hwnd
            .lock()
            .map_err(|e| AdbError::CommandFailed {
                message: format!("HWND キャッシュロック失敗: {e}"),
            })?
            .take();

        let capture_result = resolve_and_capture(&process, cached_isize)?;

        let mut guard = self
            .cached_hwnd
            .lock()
            .map_err(|e| AdbError::CommandFailed {
                message: format!("HWND キャッシュロック失敗: {e}"),
            })?;
        let (hwnd_isize, image) = capture_result;
        if let Some(p) = hwnd_isize {
            *guard = Some(p);
        }
        drop(guard);

        Ok(image)
    }
}

/// HWND 解決 + PrintWindow/GetDIBits によるキャプチャの共通同期コア。
///
/// [`Win32Capture::capture`](struct.Win32Capture.html#method.capture)(async) と
/// [`Win32Capture::capture_blocking`](struct.Win32Capture.html#method.capture_blocking)(同期)
/// の両方から呼ばれる。`spawn_blocking` 内および同期呼び出しのどちらでも動く純同期関数。
///
/// `cached_isize` はキャッシュ済み HWND のポインタ値(無ければ None)。成功時は使用した
/// HWND のポインタ値を `Some` で返し、呼び出し元でキャッシュへ戻す。失敗時は `Err` で
/// 抜けるためキャッシュは破棄扱い(次回再解決)。
fn resolve_and_capture(
    process: &str,
    cached_isize: Option<isize>,
) -> Result<(Option<isize>, DynamicImage), AdbError> {
    let hwnd = match cached_isize {
        Some(p) => {
            let hwnd = HWND(p as *mut core::ffi::c_void);
            if hwnd.is_invalid() {
                // 無効値はキャッシュ破棄扱いで再解決へ。
                resolve_hwnd(process)?
            } else {
                hwnd
            }
        }
        None => resolve_hwnd(process)?,
    };

    let (w, h) = client_size(hwnd).map_err(|code| AdbError::CommandFailed {
        message: format!("GetClientRect 失敗 (GetLastError={code})"),
    })?;
    if w == 0 || h == 0 {
        return Err(AdbError::CommandFailed {
            message: format!(
                "クライアント領域が {w}x{h} です(ウィンドウが最小化されている可能性があります)"
            ),
        });
    }

    let rgba = capture_via_printwindow(hwnd, w, h).map_err(|code| AdbError::CommandFailed {
        message: format!(
            "PrintWindow/GetDIBits 失敗 (GetLastError={code})。HWND 破棄・権限不足・クライアント領域0x0 を疑ってください"
        ),
    })?;

    let img: RgbaImage =
        ImageBuffer::from_raw(w, h, rgba).ok_or_else(|| AdbError::CommandFailed {
            message: format!("ImageBuffer::from_raw 失敗 (size mismatch w={w} h={h})"),
        })?;

    Ok((Some(hwnd.0 as isize), DynamicImage::ImageRgba8(img)))
}

/// プロセス全体の DPI アウェア化を試みる(冪等)。
///
/// `PER_MONITOR_AWARE_V2` を優先し、失敗時(古い OS 等)は `SetProcessDPIAware` 相当の
/// `SetProcessDpiAwareness(PROCESS_PER_MONITOR_DPI_AWARE)` へフォールバックする。
/// 既に設定済みの場合はエラーになるが無視してよい(冪等)。
pub fn set_process_dpi_aware() {
    unsafe {
        if SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2).is_err() {
            let _ = SetProcessDpiAwareness(PROCESS_PER_MONITOR_DPI_AWARE);
        }
    }
}

/// EnumWindows コールバック間で受け渡す状態。
struct HwndSearchState {
    target_pid: u32,
    found: Option<HWND>,
}

/// プロセス名 → PID → 可視 HWND を一括解決。失敗時は CommandFailed へ包む。
fn resolve_hwnd(process: &str) -> Result<HWND, AdbError> {
    let pid = super::win32_proc::find_pid_by_name(process).ok_or_else(|| AdbError::CommandFailed {
        message: format!(
            "プロセス \"{process}\" が見つかりません(タスクマネージャで exe 名を確認してください)"
        ),
    })?;
    find_visible_hwnd_for_pid(pid).ok_or_else(|| AdbError::CommandFailed {
        message: format!(
            "PID {pid} に紐づく可視トップレベルウィンドウが見つかりません(最小化/非表示の可能性)"
        ),
    })
}

/// EnumWindows で全トップレベルウィンドウを走査し、指定 PID に属する可視ウィンドウを返す。///
/// splash/子ウィンドウを弾くため `GetParent(hwnd)` が親なし(Err or invalid)のもののみ候補とする。
pub(crate) fn find_visible_hwnd_for_pid(pid: u32) -> Option<HWND> {
    let mut state = HwndSearchState {
        target_pid: pid,
        found: None,
    };
    let lparam = LPARAM(&mut state as *mut HwndSearchState as isize);
    unsafe {
        let _ = EnumWindows(Some(enum_proc), lparam);
    }
    state.found
}

/// EnumWindows コールバック。指定 PID に属する可視**トップレベル**ウィンドウを記録。
///
/// # Safety
/// `lparam` は呼び出し元の `&mut HwndSearchState` 由来。各 unsafe 呼び出しは
/// EnumWindows が渡す有効な HWND を前提とする。
unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> windows::core::BOOL {
    unsafe {
        let state = &mut *(lparam.0 as *mut HwndSearchState);
        if state.found.is_some() {
            return windows::Win32::Foundation::TRUE;
        }
        if !IsWindowVisible(hwnd).as_bool() {
            return windows::Win32::Foundation::TRUE;
        }
        // トップレベル(親なし)のみ。windows 0.62 では GetParent は Result<HWND> を返し、
        // 親が無い(トップレベル)場合は Err となる。Err = 親なし = 候補として採用。
        match GetParent(hwnd) {
            Ok(parent) if !parent.is_invalid() => {
                // 有効な親を持つ = 子/splash 系ウィンドウ。スキップ。
                return windows::Win32::Foundation::TRUE;
            }
            _ => {}
        }
        let mut wpid: u32 = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut wpid as *mut u32));
        if wpid == state.target_pid {
            state.found = Some(hwnd);
        }
    }
    windows::Win32::Foundation::TRUE
}

/// HWND のクライアント領域(幅 x 高さ)を返す。失敗時は GetLastError 相当(HRESULT code)を i32 で返す。
pub(crate) fn client_size(hwnd: HWND) -> Result<(u32, u32), i32> {
    use windows::Win32::UI::WindowsAndMessaging::GetClientRect;
    let mut rect = RECT::default();
    unsafe {
        GetClientRect(hwnd, &mut rect).map_err(|e| e.code().0)?;
    }
    let w = (rect.right - rect.left).max(0) as u32;
    let h = (rect.bottom - rect.top).max(0) as u32;
    Ok((w, h))
}

/// PrintWindow(PW_RENDERFULLCONTENT=2) → GetDIBits(BI_RGB,32bpp,Top-Down DIB) で
/// RGBA ピクセル列(BGRA→RGBA 変換済み)を構築する。
///
/// 戻り値の長さは (w * h * 4)。エラー時は直近の GetLastError(HRESULT code) を i32 で返す。
///
/// BitBlt(SRCCOPY) は意図的に行わない: PrintWindow が GPU 描画含むフルコンテンツを
/// mem_dc へ描いた直後に BitBlt で GDI 通常 DC の内容を上書きすると黒/空画像になる
/// (プローブ実証済み)。
pub(crate) fn capture_via_printwindow(hwnd: HWND, w: u32, h: u32) -> Result<Vec<u8>, i32> {
    unsafe {
        let last_err = || windows::Win32::Foundation::GetLastError().0 as i32;

        // hwnd_dc 取得。0x00000001 が sentinel の invalid ハンドル。
        let hwnd_dc = GetDC(Some(hwnd));
        if hwnd_dc.is_invalid() {
            return Err(last_err());
        }

        // mem_dc は 0.62.2 で直接 HDC を返す(is_invalid で失敗判定)。
        let mem_dc = CreateCompatibleDC(Some(hwnd_dc));
        if mem_dc.is_invalid() {
            let code = last_err();
            let _ = release_dc(hwnd, hwnd_dc);
            return Err(code);
        }

        // 互換ビットマップ。0.62.2 で直接 HBITMAP を返す。HGDIOBJ に .into() 可能。
        let bmp = CreateCompatibleBitmap(hwnd_dc, w as i32, h as i32);
        if bmp.is_invalid() {
            let code = last_err();
            let _ = DeleteDC(mem_dc);
            let _ = release_dc(hwnd, hwnd_dc);
            return Err(code);
        }

        // SelectObject で互換ビットマップを mem_dc へ選択。元オブジェクトは後で復元。
        let old_obj = SelectObject(mem_dc, bmp.into());

        // (主軸) PrintWindow に PW_RENDERFULLCONTENT(0x2) を渡して GPU 描画含む
        // フルコンテンツを mem_dc へレンダリング。PrintWindow は BOOL 返却。
        if !PrintWindow(hwnd, mem_dc, PRINT_WINDOW_FLAGS(PW_RENDERFULLCONTENT)).as_bool() {
            let code = last_err();
            let _ = SelectObject(mem_dc, old_obj);
            let _ = DeleteObject(bmp.into());
            let _ = DeleteDC(mem_dc);
            let _ = release_dc(hwnd, hwnd_dc);
            return Err(code);
        }

        // BITMAPINFO(BI_RGB, 32bpp, Top-Down) を構築して GetDIBits で生ピクセル取り出し。
        let mut bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w as i32,
                biHeight: -(h as i32), // 負 = Top-Down DIB
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                biSizeImage: 0,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            },
            bmiColors: [Default::default()],
        };

        let mut pixels = vec![0u8; (w as usize) * (h as usize) * 4];
        let got = GetDIBits(
            mem_dc,
            bmp,
            0,
            h,
            Some(pixels.as_mut_ptr() as *mut _),
            &mut bi,
            DIB_RGB_COLORS,
        );
        if got == 0 {
            let code = last_err();
            let _ = SelectObject(mem_dc, old_obj);
            let _ = DeleteObject(bmp.into());
            let _ = DeleteDC(mem_dc);
            let _ = release_dc(hwnd, hwnd_dc);
            return Err(code);
        }

        // クリーンアップ
        let _ = SelectObject(mem_dc, old_obj);
        let _ = DeleteObject(bmp.into());
        let _ = DeleteDC(mem_dc);
        let _ = release_dc(hwnd, hwnd_dc);

        // 32bpp BI_RGB は BGRA 順。RGBA(RgbaImage が期待) へ変換。
        for chunk in pixels.chunks_exact_mut(4) {
            chunk.swap(0, 2); // B <-> R
            // alpha は GDI では未定義(0 のことあり)。完全不透明に固定。
            chunk[3] = 0xFF;
        }

        Ok(pixels)
    }
}

/// GetDC で取得した HDC を ReleaseDC で解放。
///
/// # Safety
/// `hwnd`/`hdc` が有効であること。呼び出し元は既に `unsafe` ブロック内にあること。
pub(crate) unsafe fn release_dc(hwnd: HWND, hdc: HDC) -> i32 {
    unsafe { windows::Win32::Graphics::Gdi::ReleaseDC(Some(hwnd), hdc) }
}

/// 全ピクセルの輝度 Y=0.299R+0.587G+0.114B の母分散 σ² を計算。
///
/// キャプチャ黒画面検出(デバッグ/サニティチェック)用のユーティリティ。
/// 黒一色なら 0 に漸近する。
#[allow(dead_code)]
pub fn luminance_variance(img: &DynamicImage) -> f64 {
    let rgba = img.to_rgba8();
    let n = (rgba.width() as usize) * (rgba.height() as usize);
    if n == 0 {
        return 0.0;
    }
    let mut sum: f64 = 0.0;
    let mut sum_sq: f64 = 0.0;
    for px in rgba.pixels() {
        let r = px[0] as f64;
        let g = px[1] as f64;
        let b = px[2] as f64;
        let y = 0.299 * r + 0.587 * g + 0.114 * b;
        sum += y;
        sum_sq += y * y;
    }
    let mean = sum / n as f64;
    sum_sq / n as f64 - mean * mean
}

// HGDIOBJ への変換が SelectObject/DeleteObject で必要になることを明示参照(ドキュメント用)。
#[allow(dead_code)]
fn _unused() {
    let _ = std::convert::identity::<HGDIOBJ>;
}
