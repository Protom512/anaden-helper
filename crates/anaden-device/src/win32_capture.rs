#![cfg(windows)]

use std::sync::Mutex;

use image::{DynamicImage, ImageBuffer, RgbaImage};
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleBitmap, CreateCompatibleDC,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits, HDC, HGDIOBJ, SelectObject,
};
use windows::Win32::Storage::Xps::{PRINT_WINDOW_FLAGS, PrintWindow};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, PROCESS_PER_MONITOR_DPI_AWARE,
    SetProcessDpiAwareness, SetProcessDpiAwarenessContext,
};

use crate::error::DeviceError;

const PW_RENDERFULLCONTENT: u32 = 0x2;

pub const DEFAULT_PROCESS_NAME: &str = "AnotherEden.exe";

pub struct Win32Capture {
    process: String,
    cached_hwnd: Mutex<Option<isize>>,
}

impl Win32Capture {
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

    pub fn default_process() -> Self {
        Self::new(DEFAULT_PROCESS_NAME)
    }

    fn take_cached_hwnd(&self) -> Result<Option<isize>, DeviceError> {
        let taken = self
            .cached_hwnd
            .lock()
            .map_err(|e| DeviceError::CommandFailed {
                message: format!("HWND キャッシュロック失敗: {e}"),
            })?
            .take();
        Ok(taken)
    }

    fn restore_cached_hwnd(&self, hwnd_isize: Option<isize>) -> Result<(), DeviceError> {
        let mut guard = self
            .cached_hwnd
            .lock()
            .map_err(|e| DeviceError::CommandFailed {
                message: format!("HWND キャッシュロック失敗: {e}"),
            })?;
        if let Some(p) = hwnd_isize {
            *guard = Some(p);
        }
        Ok(())
    }

    pub async fn capture(&self) -> Result<DynamicImage, DeviceError> {
        let process = self.process.clone();
        let cached_isize = self.take_cached_hwnd()?;

        let (hwnd_isize, image) =
            tokio::task::spawn_blocking(move || resolve_and_capture(&process, cached_isize))
                .await
                .map_err(|e| DeviceError::CommandFailed {
                    message: format!("capture spawn_blocking Join 失敗: {e}"),
                })??;
        self.restore_cached_hwnd(hwnd_isize)?;
        Ok(image)
    }

    pub fn capture_blocking(&self) -> Result<DynamicImage, DeviceError> {
        let process = self.process.clone();
        let cached_isize = self.take_cached_hwnd()?;
        let (hwnd_isize, image) = resolve_and_capture(&process, cached_isize)?;
        self.restore_cached_hwnd(hwnd_isize)?;
        Ok(image)
    }
}

fn resolve_and_capture(
    process: &str,
    cached_isize: Option<isize>,
) -> Result<(Option<isize>, DynamicImage), DeviceError> {
    let hwnd = match cached_isize {
        Some(p) => {
            let hwnd = HWND(p as *mut core::ffi::c_void);
            if hwnd.is_invalid() {
                resolve_hwnd(process)?
            } else {
                hwnd
            }
        }
        None => resolve_hwnd(process)?,
    };

    let (w, h) = client_size(hwnd).map_err(|code| DeviceError::CommandFailed {
        message: format!("GetClientRect 失敗 (GetLastError={code})"),
    })?;
    if w == 0 || h == 0 {
        return Err(DeviceError::CommandFailed {
            message: format!(
                "クライアント領域が {w}x{h} です(ウィンドウが最小化されている可能性があります)"
            ),
        });
    }

    let rgba = capture_via_printwindow(hwnd, w, h).map_err(|code| DeviceError::CommandFailed {
        message: format!(
            "PrintWindow/GetDIBits 失敗 (GetLastError={code})。HWND 破棄・権限不足・クライアント領域0x0 を疑ってください"
        ),
    })?;

    let img: RgbaImage =
        ImageBuffer::from_raw(w, h, rgba).ok_or_else(|| DeviceError::CommandFailed {
            message: format!("ImageBuffer::from_raw 失敗 (size mismatch w={w} h={h})"),
        })?;

    Ok((Some(hwnd.0 as isize), DynamicImage::ImageRgba8(img)))
}

pub fn set_process_dpi_aware() {
    unsafe {
        if SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2).is_err() {
            let _ = SetProcessDpiAwareness(PROCESS_PER_MONITOR_DPI_AWARE);
        }
    }
}

fn resolve_hwnd(process: &str) -> Result<HWND, DeviceError> {
    let pid = super::win32_proc::find_pid_by_name(process).ok_or_else(|| DeviceError::CommandFailed {
        message: format!(
            "プロセス \"{process}\" が見つかりません(タスクマネージャで exe 名を確認してください)"
        ),
    })?;
    super::win32_proc::find_visible_top_level_window(pid).ok_or_else(|| DeviceError::CommandFailed {
        message: format!(
            "PID {pid} に紐づく可視トップレベルウィンドウが見つかりません(最小化/非表示の可能性)"
        ),
    })
}

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

pub(crate) fn capture_via_printwindow(hwnd: HWND, w: u32, h: u32) -> Result<Vec<u8>, i32> {
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

pub(crate) unsafe fn release_dc(hwnd: HWND, hdc: HDC) -> i32 {
    unsafe { windows::Win32::Graphics::Gdi::ReleaseDC(Some(hwnd), hdc) }
}

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

#[allow(dead_code)]
fn _unused() {
    let _ = std::convert::identity::<HGDIOBJ>;
}
