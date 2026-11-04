#![cfg(windows)]

use windows::Win32::Foundation::{HWND, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleBitmap, CreateCompatibleDC,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits, HDC, HGDIOBJ, SelectObject,
};
use windows::Win32::Storage::Xps::{PRINT_WINDOW_FLAGS, PrintWindow};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, PROCESS_PER_MONITOR_DPI_AWARE,
    SetProcessDpiAwareness, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetParent, GetWindowThreadProcessId, IsWindowVisible,
};

use image::{DynamicImage, ImageBuffer};

const PW_RENDERFULLCONTENT: u32 = 0x2;

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
    unsafe {
        if SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2).is_err() {
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

    let pid = step_find_pid(&args.process);
    let hwnd = step_find_visible_hwnd(pid);
    let (w, h) = step_client_size(hwnd);

    let dyn_img = step_capture_image(hwnd, w, h);
    verdict(dyn_img, &args.out, args.lum_threshold);
}

fn step_find_pid(process: &str) -> u32 {
    let Some(p) = find_pid_by_name(process) else {
        println!("PROCESS_NOT_FOUND");
        println!(
            "[FAIL] プロセス \"{}\" が見つかりません。タスクマネージャで正確なプロセス名(大文字小文字含む)を確認してください。",
            process
        );
        std::process::exit(2);
    };
    println!("[STEP1] process found: {} -> PID {}", process, p);
    p
}

fn step_find_visible_hwnd(pid: u32) -> HWND {
    let Some(h) = find_visible_hwnd_for_pid(pid) else {
        println!("PROCESS_NOT_FOUND");
        println!(
            "[FAIL] PID {} に紐づく可視トップレベルウィンドウが見つかりません。ウィンドウが最小化/非表示の可能性があります。",
            pid
        );
        std::process::exit(2);
    };
    println!("[STEP2] visible HWND found for PID {}: {:?}", pid, h);
    h
}

fn step_client_size(hwnd: HWND) -> (u32, u32) {
    let size = client_size(hwnd).unwrap_or_else(|code| {
        println!("CAPTURE_API_FAILED: GetClientRect GetLastError={}", code);
        std::process::exit(3);
    });
    if size.0 > 0 && size.1 > 0 {
        println!("[STEP3] client area = {}x{}", size.0, size.1);
        return size;
    }
    println!(
        "CAPTURE_API_FAILED: client area is {}x{} (0x0)",
        size.0, size.1
    );
    println!("[FAIL] クライアント領域が 0x0 です。ウィンドウが最小化されている可能性があります。");
    std::process::exit(3);
}

fn step_capture_image(hwnd: HWND, w: u32, h: u32) -> DynamicImage {
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
    match ImageBuffer::from_raw(w, h, rgba) {
        Some(img) => DynamicImage::ImageRgba8(img),
        None => {
            println!(
                "CAPTURE_API_FAILED: ImageBuffer::from_raw failed (size mismatch w={} h={})",
                w, h
            );
            std::process::exit(3);
        }
    }
}

fn verdict(dyn_img: DynamicImage, out: &str, lum_threshold: f64) {
    let variance = luminance_variance(&dyn_img);
    println!("variance={}", variance);

    match dyn_img.save(out) {
        Ok(()) => println!("[STEP6] saved: {}", out),
        Err(e) => eprintln!("[warn] save failed ({}): {}", out, e),
    }
    println!("[MANUAL] 目視してください: {}", out);

    if variance >= lum_threshold {
        println!(
            "[PASS] キャプチャ分散={:.3} >= 閾値 {:.3} → 黒画面でない(実描画取得成功)",
            variance, lum_threshold
        );
        println!("RESULT=OK_CONTENT_CAPTURED");
        std::process::exit(0);
    }
    println!(
        "[FAIL] キャプチャ分散={:.6} < 閾値 {:.3} → 黒画面(GPU描画が取れていない)",
        variance, lum_threshold
    );
    println!("RESULT=BLACK_FRAME variance={}", variance);
    println!(
        "[HINT] PrintWindow 自体は成功だが GPU 描画が取れていません。\
         fallback の WGC(Windows.Graphics.Capture) escalate が必要です。\
         第2段プローブ probe_windows_capture_wgc.rs を参照。"
    );
    std::process::exit(1);
}

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

struct HwndSearchState {
    target_pid: u32,
    found: Option<HWND>,
}

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

unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> windows::core::BOOL {
    unsafe {
        let state = &mut *(lparam.0 as *mut HwndSearchState);
        if state.found.is_some() {
            return windows::Win32::Foundation::TRUE;
        }
        if !IsWindowVisible(hwnd).as_bool() {
            return windows::Win32::Foundation::TRUE;
        }
        match GetParent(hwnd) {
            Ok(parent) if !parent.is_invalid() => {
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

fn capture_via_printwindow(hwnd: HWND, w: u32, h: u32) -> Result<Vec<u8>, i32> {
    unsafe {
        let last_err = || windows::Win32::Foundation::GetLastError().0 as i32;

        let hwnd_dc = GetDC(Some(hwnd));
        if hwnd_dc.is_invalid() {
            return Err(last_err());
        }

        let mem_dc = CreateCompatibleDC(Some(hwnd_dc));
        if mem_dc.is_invalid() {
            let code = last_err();
            let _ = release_dc(hwnd, hwnd_dc);
            return Err(code);
        }

        let bmp = CreateCompatibleBitmap(hwnd_dc, w as i32, h as i32);
        if bmp.is_invalid() {
            let code = last_err();
            let _ = DeleteDC(mem_dc);
            let _ = release_dc(hwnd, hwnd_dc);
            return Err(code);
        }

        let old_obj = SelectObject(mem_dc, bmp.into());

        if !PrintWindow(hwnd, mem_dc, PRINT_WINDOW_FLAGS(PW_RENDERFULLCONTENT)).as_bool() {
            let code = last_err();
            let _ = SelectObject(mem_dc, old_obj);
            let _ = DeleteObject(bmp.into());
            let _ = DeleteDC(mem_dc);
            let _ = release_dc(hwnd, hwnd_dc);
            return Err(code);
        }

        let mut bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w as i32,
                biHeight: -(h as i32),
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

        let _ = SelectObject(mem_dc, old_obj);
        let _ = DeleteObject(bmp.into());
        let _ = DeleteDC(mem_dc);
        let _ = release_dc(hwnd, hwnd_dc);

        for chunk in pixels.chunks_exact_mut(4) {
            chunk.swap(0, 2);
            chunk[3] = 0xFF;
        }

        Ok(pixels)
    }
}

unsafe fn release_dc(hwnd: HWND, hdc: HDC) -> i32 {
    unsafe { windows::Win32::Graphics::Gdi::ReleaseDC(Some(hwnd), hdc) }
}

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
    sum_sq / n as f64 - mean * mean
}

#[allow(dead_code)]
fn _unused() {
    let _ = std::convert::identity::<HGDIOBJ>;
}
