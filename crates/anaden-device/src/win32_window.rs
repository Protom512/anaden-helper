use std::mem;

use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowRect, SWP_NOACTIVATE, SWP_NOOWNERZORDER, SWP_NOSIZE, SWP_NOZORDER, SetWindowPos,
};

use crate::error::DeviceError;
use crate::win32_input::find_pid_by_name;
use crate::win32_proc::find_visible_top_level_window;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClampVerdict {
    NoChange,
    Moved { from: (i32, i32), to: (i32, i32) },
}

pub fn clamped_top_left(
    rect: (i32, i32, i32, i32),
    monitor: (i32, i32, i32, i32),
) -> Option<(i32, i32)> {
    let (rl, rt, rr, rb) = rect;
    let (ml, mt, mr, mb) = monitor;
    let w = rr - rl;
    let h = rb - rt;
    if rl >= ml && rt >= mt && rr <= mr && rb <= mb {
        return None;
    }
    let mut nx = rl;
    if rl < ml {
        nx = ml;
    } else if rr > mr {
        nx = mr - w;
    }
    let mut ny = rt;
    if rt < mt {
        ny = mt;
    } else if rb > mb {
        ny = mb - h;
    }
    Some((nx, ny))
}

pub fn clamp_window_into_monitor(process: &str) -> Result<ClampVerdict, DeviceError> {
    let pid = find_pid_by_name(process).ok_or_else(|| DeviceError::CommandFailed {
        message: format!("プロセスが見つかりません ({process})"),
    })?;
    let hwnd = find_visible_top_level_window(pid).ok_or_else(|| DeviceError::CommandFailed {
        message: format!("PID {pid} に紐づく可視ウィンドウが見つかりません ({process})"),
    })?;
    let rect = unsafe {
        let mut r: RECT = mem::zeroed();
        if GetWindowRect(hwnd, &mut r).is_err() {
            return Err(DeviceError::CommandFailed {
                message: format!("GetWindowRect 失敗 (process={process})"),
            });
        }
        r
    };
    let monitor = unsafe {
        let hmon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        if hmon.is_invalid() {
            return Err(DeviceError::CommandFailed {
                message: format!("MonitorFromWindow 失敗 (process={process})"),
            });
        }
        let mut info: MONITORINFO = mem::zeroed();
        info.cbSize = u32::try_from(mem::size_of::<MONITORINFO>()).unwrap_or_default();
        if !GetMonitorInfoW(hmon, &mut info).as_bool() {
            return Err(DeviceError::CommandFailed {
                message: format!("GetMonitorInfoW 失敗 (process={process})"),
            });
        }
        let rc = info.rcMonitor;
        (rc.left, rc.top, rc.right, rc.bottom)
    };
    let Some((nx, ny)) = clamped_top_left((rect.left, rect.top, rect.right, rect.bottom), monitor)
    else {
        return Ok(ClampVerdict::NoChange);
    };
    unsafe {
        SetWindowPos(
            hwnd,
            None,
            nx,
            ny,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOOWNERZORDER,
        )
        .map_err(|e| DeviceError::CommandFailed {
            message: format!("SetWindowPos 失敗 (process={process}): {e}"),
        })?;
    }
    Ok(ClampVerdict::Moved {
        from: (rect.left, rect.top),
        to: (nx, ny),
    })
}
