#![cfg(windows)]
//! ANOTHER EDEN PC版のウィンドウ画像取得プローブ(Windows 専用)。
//!
//! 目的: AnotherEden.exe(32bit cocos2d-x, OpenGL 描画, アンチチート wfsdrv 稼働中)の
//! ウィンドウ画像を PrintWindow + GDI で取得し、GPU 描画が黒一色になっていないか
//! (= 実際のゲーム画面が取れているか) を**輝度分散**で自動判定する。
//!
//! 最終目標は `anaden-engine/src/pipeline_driver.rs:60-64` の Capture trait
//!   `async fn capture(&self) -> Result<DynamicImage, AdbError>`
//! へ Windows 実装バックエンド(WindowsCapture)を足せるかの実証。
//!
//! ## 主軸 API: PrintWindow(PW_RENDERFULLCONTENT=0x2) + GDI 連鎖
//!
//! BitBlt 単体は DirectX/OpenGL 等 GPU 描画サーフェスを拾えず黒画像になる
//! (AnotherEden は cocos2d-x OpenGL 描画なので確実に黒)。PrintWindow に
//! PW_RENDERFULLCONTENT(0x2, Win8.1+) を渡すと「GPU 描画含むフルコンテンツ」を
//! レンダリングしてくれる。これが BitBlt vs PrintWindow の差別化点。
//!
//! アンチチート wfsdrv は読み取り系 API を握り潰す証拠が無い(launcher-opengl-fix.md
//! で副次要因止まり)なので、まず PrintWindow を試すのが最短。
//!
//! ### windows 0.62 の feature 落とし穴(解決済み)
//! PrintWindow は `Win32::Graphics::Gdi` ではなく **`Win32::Storage::Xps`** にある。
//! フラグ型は `PRINT_WINDOW_FLAGS`(newtype around u32)。
//! ルート `Cargo.toml` の windows features に `Win32_Storage_Xps` を追加済みでないと
//! コンパイル不可。BitBlt/CreateCompatibleDC/GetDIBits/SelectObject は
//! `Win32::Graphics::Gdi`(既存 feature)。
//!
//! また windows 0.62.2 では多くの BOOL 返却 GDI 関数が `Result<()>` を返し、
//! HWND/HDC を取る関数の該当引数は `Option<HWND>`/`Option<HDC>` を要求する。
//! `SelectObject`/`DeleteObject` は `HGDIOBJ` を要求するため HBITMAP は `.into()` で渡す。
//!
//! ## フォールバック(本プローブでは設計のみ): Windows.Graphics.Capture (WGC)
//!
//! PrintWindow が黒になる場合の escalate 先。WGC は Direct3D/DirectX 描画を含め
//! GPU サーフェスをフレームごとに取り出せる現世代の正攻法。手順:
//!   1. `GraphicsCaptureItem::CreateFromWindow(hwnd)` で HWND から item 生成
//!   2. `Direct3D11CaptureFramePool::Create(d3d11device, format, size)` で pool 作成
//!   3. `frame_pool.CreateCaptureSession(item)` → `session.StartCapture()`
//!   4. `FrameArrived` で `Direct3DSurface` を取り出し CPU 側へ copy → RGBA へ
//!
//! Cargo.toml の WGC features(Graphics_Capture / Win32_Graphics_Direct3D11 /
//! Win32_Graphics_Dxgi / Win32_System_WinRT / Win32_System_WinRT_Direct3D11 /
//! Win32_System_WinRT_Graphics_Capture)は既に定義済み。本プローブでは
//! 「PrintWindow 黒 → WGC escalate」の分岐判定ロジックとこのコメントのみ残し、
//! WGC 実装は第2段プローブ probe_windows_capture_wgc.rs とする。
//!
//! ## 使い方
//!
//! ゲーム起動済み(Launcher.exe 起動 → AnotherEden.exe のウィンドウ表示状態)で:
//! ```bash
//! cargo run --release -p anaden-device --example probe_windows_capture -- \
//!   --process AnotherEden.exe --out capture_probe.png
//! ```
//!
//! ## exit code(自動判定)
//! - 0: RESULT=OK_CONTENT_CAPTURED (分散 >= 閾値 → 実描画が取れた)
//! - 1: RESULT=BLACK_FRAME         (PrintWindow 成功だが分散 < 閾値 → GPU描画不可)
//! - 2: PROCESS_NOT_FOUND          (プロセス/HWND 特定失敗)
//! - 3: CAPTURE_API_FAILED         (PrintWindow/BitBlt/GetDIBits のいずれか失敗)

use windows::Win32::Foundation::{HWND, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleBitmap, CreateCompatibleDC,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits, HDC, HGDIOBJ, SelectObject,
};
// PrintWindow / PRINT_WINDOW_FLAGS は Graphics::Gdi ではなく Storage::Xps にある。
use windows::Win32::Storage::Xps::{PRINT_WINDOW_FLAGS, PrintWindow};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetParent, GetWindowThreadProcessId, IsWindowVisible,
};
// DPI アウェア化(4K/HiDPI 環境で GetDC/GetClientRect が論理ピクセルを返す問題を回避)。
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, PROCESS_PER_MONITOR_DPI_AWARE,
    SetProcessDpiAwareness, SetProcessDpiAwarenessContext,
};

use image::{DynamicImage, ImageBuffer, RgbaImage};

/// PW_RENDERFULLCONTENT = 0x2 (Win8.1+)。GPU 描画含むフルコンテンツをレンダリング。
const PW_RENDERFULLCONTENT: u32 = 0x2;

/// 輝度分散のデフォルト閾値。これ未満なら黒画面と判定する。
const DEFAULT_LUM_THRESHOLD: f64 = 1.0;

#[derive(Debug)]
struct Args {
    process: String,
    out: String,
    lum_threshold: f64,
}

fn parse_args() -> Result<Args, String> {
    let mut process = "AnotherEden.exe".to_string();
    let mut out = "capture_probe.png".to_string();
    let mut lum_threshold = DEFAULT_LUM_THRESHOLD;

    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--process" => {
                process = args.get(i + 1).cloned().ok_or("--process needs value")?;
                i += 2;
            }
            "--out" => {
                out = args.get(i + 1).cloned().ok_or("--out needs value")?;
                i += 2;
            }
            "--lum-threshold" => {
                lum_threshold = args
                    .get(i + 1)
                    .and_then(|v| v.parse().ok())
                    .ok_or("--lum-threshold needs f64")?;
                i += 2;
            }
            other => return Err(format!("unknown arg: {other}")),
        }
    }
    Ok(Args {
        process,
        out,
        lum_threshold,
    })
}

fn main() {
    // DPI アウェア化を最初に行う(4K/3840x2160 等のスケーリング環境で、GetDC/GetClientRect
    // が論理ピクセル(DPIスケール後)を返し画像が歪む問題を回避)。HWND 生成前に呼ぶ必要がある。
    // PER_MONITOR_AWARE_V2 が失敗する古い環境では SetProcessDPIAware にフォールバック。
    // (本プローブは HWND を自前生成しないが、GetDC/GetClientRect のピクセル解釈に影響する)
    unsafe {
        if SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2).is_err() {
            // 古い OS 向けフォールバック: システム DPI アウェア(PROCESS_PER_MONITOR_DPI_AWARE が
            // 取れなければ PROCESS_SYSTEM_DPI_AWARE 相当の system 設定を試す)。
            let _ = SetProcessDpiAwareness(PROCESS_PER_MONITOR_DPI_AWARE);
        }
    }

    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("arg error: {e}");
            std::process::exit(2);
        }
    };

    println!("=== probe_windows_capture ===");
    println!(
        "process={} out={} lum_threshold={}",
        args.process, args.out, args.lum_threshold
    );

    // (1) プロセス名 -> PID
    let pid = match find_pid_by_name(&args.process) {
        Some(p) => {
            println!("[STEP1] process found: {} -> PID {}", args.process, p);
            p
        }
        None => {
            // 自動判定不可: ユーザーがタスクマネージャでプロセス名を確認する必要あり。
            println!("PROCESS_NOT_FOUND");
            println!(
                "[FAIL] プロセス \"{}\" が見つかりません。タスクマネージャで正確なプロセス名(大文字小文字含む)を確認してください。",
                args.process
            );
            std::process::exit(2);
        }
    };

    // (2) PID -> 可視トップレベル HWND
    let hwnd = match find_visible_hwnd_for_pid(pid) {
        Some(h) => {
            println!("[STEP2] visible HWND found for PID {}: {:?}", pid, h);
            h
        }
        None => {
            println!("PROCESS_NOT_FOUND");
            println!(
                "[FAIL] PID {} に紐づく可視トップレベルウィンドウが見つかりません。ウィンドウが最小化/非表示の可能性があります。",
                pid
            );
            std::process::exit(2);
        }
    };

    // (3) クライアント領域サイズ取得
    let (w, h) = match client_size(hwnd) {
        Ok(s) if s.0 > 0 && s.1 > 0 => {
            println!("[STEP3] client area = {}x{}", s.0, s.1);
            s
        }
        Ok(s) => {
            println!("CAPTURE_API_FAILED: client area is {}x{} (0x0)", s.0, s.1);
            println!(
                "[FAIL] クライアント領域が 0x0 です。ウィンドウが最小化されている可能性があります。"
            );
            std::process::exit(3);
        }
        Err(code) => {
            println!("CAPTURE_API_FAILED: GetClientRect GetLastError={}", code);
            std::process::exit(3);
        }
    };

    // (4) PrintWindow + GDI 連鎖で RGBA ピクセルを取り出す
    let rgba = match capture_via_printwindow(hwnd, w, h) {
        Ok(buf) => {
            println!(
                "[STEP4] PrintWindow+GetDIBits succeeded: {} bytes",
                buf.len()
            );
            buf
        }
        Err(code) => {
            println!("CAPTURE_API_FAILED: GetLastError={}", code);
            println!(
                "[FAIL] PrintWindow/BitBlt/GetDIBits のいずれかが失敗しました。HWND 破棄・権限不足(管理者権限)・クライアント領域0x0 を疑ってください。"
            );
            std::process::exit(3);
        }
    };

    // (5) DynamicImage 構築
    let img: RgbaImage = match ImageBuffer::from_raw(w, h, rgba) {
        Some(b) => b,
        None => {
            println!(
                "CAPTURE_API_FAILED: ImageBuffer::from_raw failed (size mismatch w={} h={})",
                w, h
            );
            std::process::exit(3);
        }
    };
    let dyn_img = DynamicImage::ImageRgba8(img);

    // (6) 輝度分散計算
    let variance = luminance_variance(&dyn_img);
    println!("variance={}", variance);

    // (7) PNG 保存(目視確認用)
    match dyn_img.save(&args.out) {
        Ok(()) => println!("[STEP6] saved: {}", args.out),
        Err(e) => eprintln!("[warn] save failed ({}): {}", args.out, e),
    }
    println!("[MANUAL] 目視してください: {}", args.out);

    // (8) 自動判定
    if variance >= args.lum_threshold {
        println!(
            "[PASS] キャプチャ分散={:.3} >= 閾値 {:.3} → 黒画面でない(実描画取得成功)",
            variance, args.lum_threshold
        );
        println!("RESULT=OK_CONTENT_CAPTURED");
        std::process::exit(0);
    } else {
        println!(
            "[FAIL] キャプチャ分散={:.6} < 閾値 {:.3} → 黒画面(GPU描画が取れていない)",
            variance, args.lum_threshold
        );
        println!("RESULT=BLACK_FRAME variance={}", variance);
        println!(
            "[HINT] PrintWindow 自体は成功だが GPU 描画が取れていません。\
             fallback の WGC(Windows.Graphics.Capture) escalate が必要です。\
             第2段プローブ probe_windows_capture_wgc.rs を参照。"
        );
        std::process::exit(1);
    }
}

/// `CreateToolhelp32Snapshot` + `Process32First/Next` で exe 名(小文字比較)から PID を得る。
fn find_pid_by_name(exe: &str) -> Option<u32> {
    let needle = exe.to_ascii_lowercase();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).ok()?;
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        if Process32FirstW(snap, &mut entry).is_ok() {
            loop {
                let end = entry
                    .szExeFile
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(entry.szExeFile.len());
                let name = String::from_utf16_lossy(&entry.szExeFile[..end]);
                if name.to_ascii_lowercase() == needle {
                    let _ = windows::Win32::Foundation::CloseHandle(snap);
                    return Some(entry.th32ProcessID);
                }
                if Process32NextW(snap, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = windows::Win32::Foundation::CloseHandle(snap);
    }
    None
}

/// EnumWindows コールバック間で受け渡す状態。
struct HwndSearchState {
    target_pid: u32,
    found: Option<HWND>,
}

/// EnumWindows で全トップレベルウィンドウを走査し、指定 PID に属する可視ウィンドウを返す。
fn find_visible_hwnd_for_pid(pid: u32) -> Option<HWND> {
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
/// splash/子ウィンドウ(スプラッシュ画面や隠しオーナー等)を弾くため、`GetParent(hwnd)` が
/// null(=親なし = トップレベル) のみを候補とする。これをしないと最初に見つかった splash 等
/// の小ウィンドウが HWND に選ばれ、クライアント領域が 0x0 や意図しないサイズになる。
unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> windows::core::BOOL {
    // safety: lparam は呼び出し元の &mut HwndSearchState 由来。関数内の各 unsafe 呼び出しは
    // 妥当な HWND/ポインタを前提とする(EnumWindows が有効な HWND を渡す)。
    unsafe {
        let state = &mut *(lparam.0 as *mut HwndSearchState);
        if state.found.is_some() {
            // 既に候補決定済みなら以降の API 呼び出しを省略。
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

/// HWND のクライアント領域幅 x 高さを返す。失敗時は GetLastError 相当(簡易: -1)。
fn client_size(hwnd: HWND) -> Result<(u32, u32), i32> {
    use windows::Win32::UI::WindowsAndMessaging::GetClientRect;
    let mut rect = RECT::default();
    unsafe {
        GetClientRect(hwnd, &mut rect).map_err(|e| e.code().0)?;
    }
    let w = (rect.right - rect.left).max(0) as u32;
    let h = (rect.bottom - rect.top).max(0) as u32;
    Ok((w, h))
}

/// PrintWindow(PW_RENDERFULLCONTENT=2) → BitBlt(SRCCOPY) → GetDIBits(BI_RGB,32bpp) で
/// RGBA ピクセル列(Top-Down DIB で取得し BGRA→RGBA 変換)を構築する。
///
/// 戻り値の長さは (w * h * 4)。エラー時は直近の GetLastError(HRESULT code) を i32 で返す。
fn capture_via_printwindow(hwnd: HWND, w: u32, h: u32) -> Result<Vec<u8>, i32> {
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

        // ※ BitBlt(SRCCOPY, hwnd_dc -> mem_dc) は**意図的に行わない**。
        // PrintWindow が GPU 描画含むフルコンテンツを mem_dc へ描いた直後に BitBlt で
        // GDI 通常DC(非GPUサーフェス)の内容を上書きすると黒/空画像になり、本プローブの
        // 黒画面判定を汚染する。PrintWindow の結果(mem_dc)をそのまま使う。

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
/// 呼び出し元が既に `unsafe` ブロック内にあり、`hwnd`/`hdc` が有効であること。
unsafe fn release_dc(hwnd: HWND, hdc: HDC) -> i32 {
    unsafe { windows::Win32::Graphics::Gdi::ReleaseDC(Some(hwnd), hdc) }
}

/// 全ピクセルの輝度 Y=0.299R+0.587G+0.114B の分散 σ² を計算。
fn luminance_variance(img: &DynamicImage) -> f64 {
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
    // 母分散(σ²)。黒一色なら 0 に漸近。
    sum_sq / n as f64 - mean * mean
}

// HGDIOBJ への変換が SelectObject/DeleteObject で必要になることを明示参照(ドキュメント用)。
#[allow(dead_code)]
fn _unused() {
    let _ = std::convert::identity::<HGDIOBJ>;
}
