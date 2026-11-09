#![cfg(windows)]

use std::mem;
use std::time::Duration;

use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_MOUSE, MOUSE_EVENT_FLAGS, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_LEFTDOWN,
    MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_VIRTUALDESK, MOUSEINPUT, SendInput,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetSystemMetrics, GetWindowRect, GetWindowThreadProcessId, IsIconic,
    PostMessageW, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
    SW_RESTORE, SetCursorPos, SetForegroundWindow, SetProcessDPIAware, ShowWindow, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MOUSEMOVE,
};

use anaden_core::InputAction;

use crate::error::DeviceError;

const MK_LBUTTON: usize = 0x0001;

const SWIPE_STEP_MS: u64 = 10;

const DEFAULT_LONGPRESS_MS: u64 = 600;

const FOREGROUND_SETTLE_MS: u64 = 150;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
pub enum InputMethod {
    #[default]
    SendInput,
    PostMessage,
}

pub struct Win32InputExecutor {
    process: String,
    method: InputMethod,
}

impl Win32InputExecutor {
    pub fn new(process: &str) -> Self {
        enable_dpi_awareness();
        Self {
            process: process.to_string(),
            method: InputMethod::default(),
        }
    }

    pub fn with_method(process: &str, method: InputMethod) -> Self {
        enable_dpi_awareness();
        Self {
            process: process.to_string(),
            method,
        }
    }

    #[must_use]
    pub fn new_without_dpi(process: &str) -> Self {
        Self {
            process: process.to_string(),
            method: InputMethod::default(),
        }
    }

    pub fn method(&self) -> InputMethod {
        self.method
    }

    pub async fn execute(&self, action: &InputAction) -> Result<(), DeviceError> {
        if let InputAction::Wait(d) = action {
            tokio::time::sleep(*d).await;
            return Ok(());
        }

        let process = self.process.clone();
        let method = self.method;
        let action = action.clone();
        tokio::task::spawn_blocking(move || run_action_sync(&process, method, &action))
            .await
            .map_err(|e| DeviceError::CommandFailed {
                message: format!("入力ワーカがパニック/キャンセル: {e}"),
            })?
    }

    pub fn execute_blocking(&self, action: &InputAction) -> Result<(), DeviceError> {
        run_action_sync(&self.process, self.method, action)
    }
}

fn run_action_sync(
    process: &str,
    method: InputMethod,
    action: &InputAction,
) -> Result<(), DeviceError> {
    match action {
        InputAction::Tap(point) => click(process, method, point.x as i32, point.y as i32, 60),
        InputAction::LongPress(point, hold_ms) => {
            let hold = if *hold_ms == 0 {
                DEFAULT_LONGPRESS_MS
            } else {
                *hold_ms
            };
            click(process, method, point.x as i32, point.y as i32, hold)
        }
        InputAction::Swipe {
            from,
            to,
            duration_ms,
        } => swipe(
            process,
            method,
            from.x as i32,
            from.y as i32,
            to.x as i32,
            to.y as i32,
            *duration_ms,
        ),
        InputAction::Wait(_) => Ok(()),
    }
}

fn click(
    process: &str,
    method: InputMethod,
    x: i32,
    y: i32,
    hold_ms: u64,
) -> Result<(), DeviceError> {
    let (pid, hwnd) = resolve_pid_and_hwnd(process)?;
    match method {
        InputMethod::SendInput => {
            if !bring_to_foreground(hwnd) {
                return Err(DeviceError::CommandFailed {
                    message: format!(
                        "前景化失敗 (process={process})。別アプリがフォアを握るか SetForegroundWindow 拒否。"
                    ),
                });
            }
            std::thread::sleep(Duration::from_millis(FOREGROUND_SETTLE_MS));
            if !foreground_belongs_to(pid) {
                return Err(DeviceError::CommandFailed {
                    message: format!(
                        "SendInput 直前に前景が別ウィンドウへ奪われた (process={process})。誤クリック防止のため注入中止。"
                    ),
                });
            }
            let (origin_x, origin_y) = client_to_screen_abs(hwnd, 0, 0);
            let (fire_x, fire_y) = (origin_x + x, origin_y + y);
            if !point_in_window_rect(hwnd, fire_x, fire_y) {
                return Err(DeviceError::CommandFailed {
                    message: format!(
                        "発火座標({fire_x},{fire_y})が対象ウィンドウ画面矩形外 (process={process})。誤クリック防止のため注入中止。"
                    ),
                });
            }
            let sent = sendinput_click(fire_x, fire_y, hold_ms);
            if sent != 2 {
                return Err(DeviceError::CommandFailed {
                    message: format!(
                        "SendInput 戻り値<2 (got={sent})。UIPI/デスクトップ分離/管理者権限不足でブロックの疑い。"
                    ),
                });
            }
            Ok(())
        }
        InputMethod::PostMessage => {
            let (down_ok, up_ok) = postmessage_click(hwnd, x, y, hold_ms);
            if down_ok != 1 || up_ok != 1 {
                return Err(DeviceError::CommandFailed {
                    message: format!(
                        "PostMessageW が Err を返しました (down={down_ok}, up={up_ok})"
                    ),
                });
            }
            Ok(())
        }
    }
}

fn swipe(
    process: &str,
    method: InputMethod,
    x1: i32,
    y1: i32,
    x2: i32,
    y2: i32,
    duration_ms: u64,
) -> Result<(), DeviceError> {
    let (pid, hwnd) = resolve_pid_and_hwnd(process)?;
    let steps = ((duration_ms / SWIPE_STEP_MS).max(1)) as i32;
    match method {
        InputMethod::SendInput => {
            if !bring_to_foreground(hwnd) {
                return Err(DeviceError::CommandFailed {
                    message: format!(
                        "前景化失敗 (process={process})。Swipe の SendInput には前景化が必須。"
                    ),
                });
            }
            std::thread::sleep(Duration::from_millis(FOREGROUND_SETTLE_MS));
            if !foreground_belongs_to(pid) {
                return Err(DeviceError::CommandFailed {
                    message: format!(
                        "SendInput 直前に前景が別ウィンドウへ奪われた (process={process})。Swipe 誤操作防止のため注入中止。"
                    ),
                });
            }
            let (origin_x, origin_y) = client_to_screen_abs(hwnd, 0, 0);

            let (start_x, start_y) = (origin_x + x1, origin_y + y1);
            let (end_x, end_y) = (origin_x + x2, origin_y + y2);
            if !point_in_window_rect(hwnd, start_x, start_y)
                || !point_in_window_rect(hwnd, end_x, end_y)
            {
                return Err(DeviceError::CommandFailed {
                    message: format!(
                        "Swipe 端点(start={start_x},{start_y} end={end_x},{end_y})が対象ウィンドウ画面矩形外 (process={process})。誤操作防止のため注入中止。"
                    ),
                });
            }

            let (vw, vh, ox, oy) = virtual_screen();
            let base = MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK | MOUSEEVENTF_MOVE;

            let (ax0, ay0) = to_absolute(origin_x + x1 - ox, origin_y + y1 - oy, vw, vh);
            let down = unsafe { send_mouse(ax0, ay0, base | MOUSEEVENTF_LEFTDOWN) };
            if down == 0 {
                return Err(DeviceError::CommandFailed {
                    message: "Swipe: SendInput(MOUSEEVENTF_LEFTDOWN) 挿入失敗".to_string(),
                });
            }

            for i in 1..=steps {
                let t = i as f64 / steps as f64;
                let cx = (origin_x as f64 + x1 as f64 + (x2 - x1) as f64 * t).round() as i32;
                let cy = (origin_y as f64 + y1 as f64 + (y2 - y1) as f64 * t).round() as i32;
                let (ax, ay) = to_absolute(cx - ox, cy - oy, vw, vh);
                let _ = unsafe { send_mouse(ax, ay, base | MOUSEEVENTF_MOVE) };
                std::thread::sleep(Duration::from_millis(SWIPE_STEP_MS));
            }

            let (ax1, ay1) = to_absolute(origin_x + x2 - ox, origin_y + y2 - oy, vw, vh);
            let up = unsafe { send_mouse(ax1, ay1, base | MOUSEEVENTF_LEFTUP) };
            if up == 0 {
                return Err(DeviceError::CommandFailed {
                    message: "Swipe: SendInput(MOUSEEVENTF_LEFTUP) 挿入失敗".to_string(),
                });
            }
            Ok(())
        }
        InputMethod::PostMessage => {
            let lp1 = make_lparam(x1, y1);
            let lp2 = make_lparam(x2, y2);
            let (sx1, sy1) = client_to_screen_abs(hwnd, x1, y1);
            unsafe {
                let _ = SetCursorPos(sx1, sy1);
                let _ = PostMessageW(Some(hwnd), WM_MOUSEMOVE, WPARAM(0), lp1);
                let down = PostMessageW(Some(hwnd), WM_LBUTTONDOWN, WPARAM(MK_LBUTTON), lp1);
                if down.is_err() {
                    return Err(DeviceError::CommandFailed {
                        message: "Swipe: PostMessage(WM_LBUTTONDOWN) 失敗".to_string(),
                    });
                }
                for i in 1..=steps {
                    let t = i as f64 / steps as f64;
                    let cx = (x1 as f64 + (x2 - x1) as f64 * t).round() as i32;
                    let cy = (y1 as f64 + (y2 - y1) as f64 * t).round() as i32;
                    let _ = PostMessageW(
                        Some(hwnd),
                        WM_MOUSEMOVE,
                        WPARAM(MK_LBUTTON),
                        make_lparam(cx, cy),
                    );
                    std::thread::sleep(Duration::from_millis(SWIPE_STEP_MS));
                }
                let up = PostMessageW(Some(hwnd), WM_LBUTTONUP, WPARAM(MK_LBUTTON), lp2);
                if up.is_err() {
                    return Err(DeviceError::CommandFailed {
                        message: "Swipe: PostMessage(WM_LBUTTONUP) 失敗".to_string(),
                    });
                }
            }
            Ok(())
        }
    }
}

pub(crate) fn find_pid_by_name(name: &str) -> Option<u32> {
    super::win32_proc::find_pid_by_name(name)
}

fn resolve_pid_and_hwnd(process: &str) -> Result<(u32, HWND), DeviceError> {
    let pid = find_pid_by_name(process).ok_or_else(|| DeviceError::CommandFailed {
        message: format!(
            "プロセスが見つかりません ({process})。ゲームを起動してから再実行してください。"
        ),
    })?;
    let hwnd = super::win32_proc::find_visible_top_level_window(pid)
        .ok_or_else(|| DeviceError::CommandFailed {
            message: format!(
                "PID {pid} に紐づく可視ウィンドウが見つかりません ({process})。ウィンドウが最小化/非表示の可能性。"
            ),
        })?;
    Ok((pid, hwnd))
}

fn rect_contains(rect: &RECT, x: i32, y: i32) -> bool {
    x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom
}

fn point_in_window_rect(hwnd: HWND, sx: i32, sy: i32) -> bool {
    unsafe {
        if hwnd.0.is_null() {
            return false;
        }
        let mut rect: RECT = mem::zeroed();
        if GetWindowRect(hwnd, &mut rect as *mut RECT).is_err() {
            return false;
        }
        rect_contains(&rect, sx, sy)
    }
}

fn foreground_belongs_to(target_pid: u32) -> bool {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return false;
        }
        let mut pid: u32 = 0;
        let _thread_id = GetWindowThreadProcessId(hwnd, Some(&mut pid as *mut u32));
        pid == target_pid
    }
}

pub(crate) fn enable_dpi_awareness() {
    unsafe {
        if SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2).is_err() {
            let _ = SetProcessDPIAware();
        }
    }
}

pub(crate) fn virtual_screen() -> (i32, i32, i32, i32) {
    unsafe {
        let vw = GetSystemMetrics(SM_CXVIRTUALSCREEN);
        let vh = GetSystemMetrics(SM_CYVIRTUALSCREEN);
        let ox = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let oy = GetSystemMetrics(SM_YVIRTUALSCREEN);
        (vw, vh, ox, oy)
    }
}

pub(crate) fn to_absolute(px: i32, py: i32, vw: i32, vh: i32) -> (i32, i32) {
    let vw1 = (vw as i64 - 1).max(1);
    let vh1 = (vh as i64 - 1).max(1);
    let dx = ((px as i64 * 65535) / vw1) as i32;
    let dy = ((py as i64 * 65535) / vh1) as i32;
    (dx, dy)
}

pub(crate) fn client_to_screen_abs(hwnd: HWND, client_x: i32, client_y: i32) -> (i32, i32) {
    unsafe {
        let mut pt = POINT {
            x: client_x,
            y: client_y,
        };
        if ClientToScreen(hwnd, &mut pt as *mut POINT).as_bool() {
            (pt.x, pt.y)
        } else {
            let mut r: RECT = mem::zeroed();
            let _ = GetWindowRect(hwnd, &mut r as *mut RECT);
            (r.left + client_x, r.top + client_y)
        }
    }
}

pub(crate) fn bring_to_foreground(hwnd: HWND) -> bool {
    unsafe {
        if hwnd.0.is_null() {
            return false;
        }
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        let cur_thread = GetCurrentThreadId();
        let fg = GetForegroundWindow();
        let fg_thread = if fg.0.is_null() {
            0
        } else {
            GetWindowThreadProcessId(fg, None)
        };
        let mut attached = false;
        if fg_thread != 0 && fg_thread != cur_thread {
            attached = AttachThreadInput(cur_thread, fg_thread, true).as_bool();
        }
        let ok = SetForegroundWindow(hwnd);
        if attached {
            let _ = AttachThreadInput(cur_thread, fg_thread, false);
        }
        ok.as_bool()
    }
}

unsafe fn send_mouse(dx: i32, dy: i32, flags: MOUSE_EVENT_FLAGS) -> u32 {
    unsafe {
        let mut inp: INPUT = mem::zeroed();
        inp.r#type = INPUT_MOUSE;
        inp.Anonymous.mi = MOUSEINPUT {
            dx,
            dy,
            mouseData: 0,
            dwFlags: flags,
            time: 0,
            dwExtraInfo: 0,
        };
        SendInput(&[inp], mem::size_of::<INPUT>() as i32)
    }
}

pub(crate) fn sendinput_click(screen_x: i32, screen_y: i32, hold_ms: u64) -> u32 {
    let (vw, vh, ox, oy) = virtual_screen();
    let (ax, ay) = to_absolute(screen_x - ox, screen_y - oy, vw, vh);
    let base = MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK | MOUSEEVENTF_MOVE;
    let down = unsafe { send_mouse(ax, ay, base | MOUSEEVENTF_LEFTDOWN) };
    std::thread::sleep(Duration::from_millis(hold_ms));
    let up = unsafe { send_mouse(ax, ay, base | MOUSEEVENTF_LEFTUP) };
    down + up
}

fn make_lparam(client_x: i32, client_y: i32) -> LPARAM {
    let v = (((client_y as u32) << 16) | ((client_x as u32) & 0xFFFF)) as isize;
    LPARAM(v)
}

pub(crate) fn postmessage_click(
    hwnd: HWND,
    client_x: i32,
    client_y: i32,
    hold_ms: u64,
) -> (u32, u32) {
    let lp = make_lparam(client_x, client_y);
    let (sx, sy) = client_to_screen_abs(hwnd, client_x, client_y);
    unsafe {
        let _ = SetCursorPos(sx, sy);
        let _ = PostMessageW(Some(hwnd), WM_MOUSEMOVE, WPARAM(0), lp);
        let down = PostMessageW(Some(hwnd), WM_LBUTTONDOWN, WPARAM(MK_LBUTTON), lp);
        std::thread::sleep(Duration::from_millis(hold_ms));
        let up = PostMessageW(Some(hwnd), WM_LBUTTONUP, WPARAM(MK_LBUTTON), lp);
        (down.is_ok() as u32, up.is_ok() as u32)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn make_lparam_packs_x_low_y_high() {
        let lp = make_lparam(100, 200);
        let raw = lp.0 as u32;
        assert_eq!(raw & 0xFFFF, 100);
        assert_eq!((raw >> 16) & 0xFFFF, 200);
    }

    #[test]
    fn to_absolute_uses_65535_formula() {
        let (ax, ay) = to_absolute(0, 0, 65536, 65536);
        assert_eq!((ax, ay), (0, 0));
        let (bx, by) = to_absolute(32767, 32767, 65536, 65536);
        assert_eq!((bx, by), (32767, 32767));
    }

    #[test]
    fn default_method_is_sendinput() {
        assert_eq!(InputMethod::default(), InputMethod::SendInput);
    }

    #[test]
    fn new_without_dpi_keeps_default_method() {
        let executor = Win32InputExecutor::new_without_dpi("AnotherEden.exe");
        assert_eq!(executor.method(), InputMethod::SendInput);
    }

    #[test]
    fn execute_blocking_wait_returns_ok_without_runtime() {
        let executor = Win32InputExecutor::new_without_dpi("AnotherEden.exe");
        executor
            .execute_blocking(&InputAction::Wait(Duration::from_millis(1)))
            .unwrap();
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "FFI 依存。foreground_belongs_to の実機手動検証用スタブ"]
    fn foreground_belongs_to_real_nonexistent_pid_is_false() {
        let bogus = 0xFFFF_FFFF;
        let result = foreground_belongs_to(bogus);
        assert!(!result, "存在しない PID では false(注入中止)になるべき");
    }

    #[cfg(windows)]
    #[test]
    fn resolve_pid_and_hwnd_missing_process_errors() {
        let err = resolve_pid_and_hwnd("__definitely_not_a_real_process__.exe").unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("プロセスが見つかりません"),
            "missing process must surface as CommandFailed, got: {msg}"
        );
    }

    #[test]
    fn rect_contains_inside_and_outside() {
        let rect = RECT {
            left: 100,
            top: 100,
            right: 800,
            bottom: 600,
        };
        assert!(rect_contains(&rect, 400, 350));
        assert!(!rect_contains(&rect, 50, 50));
        assert!(!rect_contains(&rect, 900, 700));
        assert!(!rect_contains(&rect, 50, 350));
        assert!(!rect_contains(&rect, 400, 50));
    }

    #[test]
    fn rect_contains_corners_and_edges_are_inclusive() {
        let rect = RECT {
            left: 100,
            top: 100,
            right: 800,
            bottom: 600,
        };
        assert!(rect_contains(&rect, 100, 100), "左上角");
        assert!(rect_contains(&rect, 800, 100), "右上角");
        assert!(rect_contains(&rect, 100, 600), "左下角");
        assert!(rect_contains(&rect, 800, 600), "右下角");
        assert!(rect_contains(&rect, 450, 100), "上辺");
        assert!(rect_contains(&rect, 800, 350), "右辺");
        assert!(!rect_contains(&rect, 99, 350), "左辺の1px外");
        assert!(!rect_contains(&rect, 801, 350), "右辺の1px外");
    }

    #[test]
    fn point_in_window_rect_null_hwnd_is_false() {
        let null_hwnd = HWND(std::ptr::null_mut());
        assert!(!point_in_window_rect(null_hwnd, 0, 0));
        assert!(!point_in_window_rect(null_hwnd, 500, 500));
    }
}
