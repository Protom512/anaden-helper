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
    EnumWindows, GetForegroundWindow, GetSystemMetrics, GetWindowRect, GetWindowThreadProcessId,
    IsIconic, IsWindowVisible, PostMessageW, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN,
    SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SW_RESTORE, SetCursorPos, SetForegroundWindow,
    SetProcessDPIAware, ShowWindow, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE,
};
use windows::core::BOOL;

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

struct HwndSearchState {
    target_pid: u32,
    found: Option<HWND>,
}

pub(crate) fn find_main_window(pid: u32) -> Option<HWND> {
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

unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    unsafe {
        let state = &mut *(lparam.0 as *mut HwndSearchState);
        if state.found.is_some() {
            return BOOL(0);
        }
        if !IsWindowVisible(hwnd).as_bool() {
            return BOOL(1);
        }
        let mut wpid: u32 = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut wpid as *mut u32));
        if wpid == state.target_pid {
            state.found = Some(hwnd);
            return BOOL(0);
        }
        BOOL(1)
    }
}

fn resolve_pid_and_hwnd(process: &str) -> Result<(u32, HWND), DeviceError> {
    let pid = find_pid_by_name(process).ok_or_else(|| DeviceError::CommandFailed {
        message: format!(
            "プロセスが見つかりません ({process})。ゲームを起動してから再実行してください。"
        ),
    })?;
    let hwnd = find_main_window(pid).ok_or_else(|| DeviceError::CommandFailed {
        message: format!("PID {pid} に紐づく可視ウィンドウが見つかりません ({process})。ウィンドウが最小化/非表示の可能性。"),
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
        // MAKELPARAM: 下位16bit=x, 高位16bit=y。
        let lp = make_lparam(100, 200);
        let raw = lp.0 as u32;
        assert_eq!(raw & 0xFFFF, 100);
        assert_eq!((raw >> 16) & 0xFFFF, 200);
    }

    #[test]
    fn to_absolute_uses_65535_formula() {
        // extent=65536 の仮想デスクトップで (0,0) → (0,0)、(65535,65535) → (65535,65535)。
        let (ax, ay) = to_absolute(0, 0, 65536, 65536);
        assert_eq!((ax, ay), (0, 0));
        let (bx, by) = to_absolute(32767, 32767, 65536, 65536);
        // (32767 * 65535) / 65535 = 32767
        assert_eq!((bx, by), (32767, 32767));
    }

    #[test]
    fn default_method_is_sendinput() {
        assert_eq!(InputMethod::default(), InputMethod::SendInput);
    }

    /// `new_without_dpi`: DPI アウェア化を伴わない生成 (メソッドは既定 SendInput)。
    #[test]
    fn new_without_dpi_keeps_default_method() {
        let executor = Win32InputExecutor::new_without_dpi("AnotherEden.exe");
        assert_eq!(executor.method(), InputMethod::SendInput);
    }

    /// `execute_blocking`: Wait は FFI を伴わない (run_action_sync の Wait 分岐は
    /// 即 Ok) ので、ランタイム無し・実機入力無しで同期経路の疎通だけ検証できる。
    /// Tap 火球の実注入を伴うテストは書かない (実機 E2E は studio Shard 3)。
    #[test]
    fn execute_blocking_wait_returns_ok_without_runtime() {
        let executor = Win32InputExecutor::new_without_dpi("AnotherEden.exe");
        executor
            .execute_blocking(&InputAction::Wait(Duration::from_millis(1)))
            .unwrap();
    }

    /// ガード2: foreground_belongs_to の実機手動検証(#[ignore])。
    ///
    /// FFI(GetForegroundWindow / GetWindowThreadProcessId)に依存しモック不可のため
    /// 自動実行対象外。手順:
    /// 1. `cargo nextest run -p anaden-device foreground_belongs_to_real --run-ignored all`
    ///    を実行中のプロセス(PID を下記 CURRENT_PID に埋める)前景状態で確認 → true 期待。
    /// 2. テスト実行直後に別ウィンドウ(エクスプローラ等)をクリックして前景を奪うか、
    ///    存在しない PID(0xFFFF_FFFF 等)を渡して → false 期待。
    ///
    /// ここでは「存在しない PID を渡した場合は、たとえフォアグラウンドが何であれ
    /// 自プロセス以外なら false」という安全側の振る舞いを、確実に false になる
    /// 存在しない PID で検証する(前景が何であれ PID 0xFFFFFFFF と一致することは実質ない)。
    #[cfg(windows)]
    #[test]
    #[ignore = "FFI 依存。foreground_belongs_to の実機手動検証用スタブ"]
    fn foreground_belongs_to_real_nonexistent_pid_is_false() {
        // 存在しえない PID。GetWindowThreadProcessId が返す PID と一致しないため false。
        // (プロセステーブルの最大 PID 65535(0xFFFF) を超える値で安全側を確認。)
        let bogus = 0xFFFF_FFFF;
        let result = foreground_belongs_to(bogus);
        // 前景ウィンドウが存在する限り、その PID が 0xFFFFFFFF と一致することは実質ない。
        // 前景ウィンドウが取得できない(null)場合も false になるため、どちらにせよ false。
        assert!(!result, "存在しない PID では false(注入中止)になるべき");
    }

    /// ガード2: resolve_pid_and_hwnd がプロセス不存在時に CommandFailed を返すこと。
    ///
    /// FFI(CreateToolhelp32Snapshot)には依存するが、存在しえないプロセス名を与えれば
    /// スナップショット成功時は None → CommandFailed へ包まれる。これは resolve_hwnd の
    /// 分割後もエラー伝播経路が保たれていることを検証する(純粋ロジック寄りの部分)。
    #[cfg(windows)]
    #[test]
    fn resolve_pid_and_hwnd_missing_process_errors() {
        let err = resolve_pid_and_hwnd("__definitely_not_a_real_process__.exe").unwrap_err();
        let msg = format!("{err}");
        // プロセスが見つからないメッセージが返ること(CommandFailed への包み込み確認)。
        assert!(
            msg.contains("プロセスが見つかりません"),
            "missing process must surface as CommandFailed, got: {msg}"
        );
    }

    /// ガード3: rect_contains の境界内/外を検証する純粋関数テスト。
    #[test]
    fn rect_contains_inside_and_outside() {
        // 画面座標 (100,100)-(800,600) の矩形。
        let rect = RECT {
            left: 100,
            top: 100,
            right: 800,
            bottom: 600,
        };
        // 内部(中央)。
        assert!(rect_contains(&rect, 400, 350));
        // 左上より外。
        assert!(!rect_contains(&rect, 50, 50));
        // 右下より外。
        assert!(!rect_contains(&rect, 900, 700));
        // x だけ外。
        assert!(!rect_contains(&rect, 50, 350));
        // y だけ外。
        assert!(!rect_contains(&rect, 400, 50));
    }

    /// ガード3: rect_contains のコーナー/辺上は閉区間で許容されることを検証。
    #[test]
    fn rect_contains_corners_and_edges_are_inclusive() {
        let rect = RECT {
            left: 100,
            top: 100,
            right: 800,
            bottom: 600,
        };
        // 4隅(辺上 = 閉区間で true)。
        assert!(rect_contains(&rect, 100, 100), "左上角");
        assert!(rect_contains(&rect, 800, 100), "右上角");
        assert!(rect_contains(&rect, 100, 600), "左下角");
        assert!(rect_contains(&rect, 800, 600), "右下角");
        // 辺上の中央。
        assert!(rect_contains(&rect, 450, 100), "上辺");
        assert!(rect_contains(&rect, 800, 350), "右辺");
        // 辺の1px外側は false。
        assert!(!rect_contains(&rect, 99, 350), "左辺の1px外");
        assert!(!rect_contains(&rect, 801, 350), "右辺の1px外");
    }

    /// ガード3: point_in_window_rect は null HWND で安全側 false を返すこと。
    /// (FFI の GetWindowRect には到達せず、null 判定で落とす経路。)
    #[test]
    fn point_in_window_rect_null_hwnd_is_false() {
        let null_hwnd = HWND(std::ptr::null_mut());
        assert!(!point_in_window_rect(null_hwnd, 0, 0));
        assert!(!point_in_window_rect(null_hwnd, 500, 500));
    }
}
