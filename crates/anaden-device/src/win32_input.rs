//! Win32 SendInput / PostMessage 入力注入バックエンド(Windows 専用)。
//!
//! `anaden-core::InputAction::{Tap, Swipe, LongPress, Wait}` を Win32 のマウスイベントへ
//! 翻訳して注入する。プローブ `examples/probe_windows_input.rs` で動作検証済みのロジック
//! (SendInput でアンチチート wfsdrv を突破確認)をベースに、`InputExecutor` と対称な
//! `Win32InputExecutor` を提供する。
//!
//! # 座標系
//! InputAction の (x, y) は `InputExecutor` と同様「画面左上原点の実ピクセル」
//! (pipeline_driver の rescale 後 device_width 座標)。プローブ phase2 と同じく
//! クライアント原点の画面座標 + (x,y) で SendInput 用画面絶対座標を作る。
//!
//! # メソッド
//! - 主軸: SendInput(INPUT_MOUSE) — 物理マウス同等。ゲストプロセスからはユーザ操作と区別不可。
//! - fallback: PostMessageW(WM_LBUTTONDOWN/UP) — 合成メッセージ。一部ゲームは synthetic を弾く。
//!
//! # Linux ビルド
//! モジュール全体を `#![cfg(windows)]` で囲み、Linux ではコンパイル対象外とする。
//! lib.rs 側の `#[cfg(windows)] mod win32_input;` と二重安全。

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

use crate::client::AdbError;

// wParam 区分(PostMessage 用)。
const MK_LBUTTON: usize = 0x0001;

/// スワイプの MOVE イベント送出間隔(ミリ秒)。Android `input swipe` の連続 down/move/up
/// を SendInput の MOVE 連打で近似するための刻み。小さすぎると SendInput 負荷上昇。
const SWIPE_STEP_MS: u64 = 10;

/// LongPress のデフォルト押下時間(action 側で明示指定がないときの安全弁)。
const DEFAULT_LONGPRESS_MS: u64 = 600;

/// 前景化後の安定 wait(プローブ phase2 の 150ms と同等)。
const FOREGROUND_SETTLE_MS: u64 = 150;

/// 入力注入メソッド。
///
/// `SendInput` は物理マウス同等(主軸)。`PostMessage` は合成ウィンドウメッセージで
/// 背面送信可能だが、一部ゲームは synthetic フラグで弾く(fallback 扱い)。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputMethod {
    /// SendInput(INPUT_MOUSE)。アンチチート wfsdrv を突破実証済みの主軸。
    SendInput,
    /// PostMessageW(WM_LBUTTONDOWN/UP)。背面送信可能な fallback。
    PostMessage,
}

impl Default for InputMethod {
    fn default() -> Self {
        Self::SendInput
    }
}

/// Win32 で InputAction を実行する入力 executor。
///
/// `process`(既定 "AnotherEden.exe")から都度 PID → HWND を解決し、SendInput/PostMessage で
/// マウスイベントを注入する。HWND はキャッシュせず毎回解決する(ウィンドウ再生成対策。
/// プローブと同じ挙動で、認識バグの温床を避ける)。
///
/// `execute` は SendInput の `std::thread::sleep`(hold 中)で async runtime を止めないよう
/// `spawn_blocking` でワーカスレッドへ逃す。`Wait` のみ `tokio::time::sleep` で非同期待機。
pub struct Win32InputExecutor {
    process: String,
    method: InputMethod,
}

impl Win32InputExecutor {
    /// 指定プロセス名(例: "AnotherEden.exe")に対する executor を作る。既定メソッド=SendInput。
    ///
    /// DPI アウェア化をここで1回呼ぶ(プロセス全体へ影響するが冪等。capture 側でも呼ばれて
    /// いても二重安全)。これをしないと高 DPI 環境で ClientToScreen/仮想画面メトリクスが
    /// 論理ピクセルを返し、座標がずれる。
    pub fn new(process: &str) -> Self {
        enable_dpi_awareness();
        Self {
            process: process.to_string(),
            method: InputMethod::default(),
        }
    }

    /// 注入メソッドを明示指定するコンストラクタ。
    pub fn with_method(process: &str, method: InputMethod) -> Self {
        enable_dpi_awareness();
        Self {
            process: process.to_string(),
            method,
        }
    }

    /// 現在の注入メソッドを返す。
    pub fn method(&self) -> InputMethod {
        self.method
    }

    /// `InputAction` を実行する。
    ///
    /// 座標は InputAction が持つ「画面左上原点の実ピクセル」(device_width 座標)。
    /// 内部でクライアント原点(client_origin = client_to_screen_abs(hwnd, 0, 0))を足して
    /// 画面絶対座標へ変換し、SendInput/PostMessage へ渡す(プローブ phase2 と同一方式)。
    pub async fn execute(&self, action: &InputAction) -> Result<(), AdbError> {
        // Wait は IO を伴わない非同期待機なので runtime スレッドで直接 sleep。
        if let InputAction::Wait(d) = action {
            tokio::time::sleep(*d).await;
            return Ok(());
        }

        let process = self.process.clone();
        let method = self.method;
        let action = action.clone();
        // SendInput/PostMessage は std::thread::sleep で hold する同期処理なので
        // spawn_blocking でワーカスレッドへ逃す(runtime 阻止回避)。
        tokio::task::spawn_blocking(move || run_action_sync(&process, method, &action))
            .await
            .map_err(|e| AdbError::CommandFailed {
                message: format!("入力ワーカがパニック/キャンセル: {e}"),
            })?
    }
}

/// SendInput/PostMessage の同期注入本体(spawn_blocking 内で実行)。
fn run_action_sync(
    process: &str,
    method: InputMethod,
    action: &InputAction,
) -> Result<(), AdbError> {
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
        // Wait は execute 側で非同期 sleep 済み。ここには来ない。
        InputAction::Wait(_) => Ok(()),
    }
}

/// Tap / LongPress 共通: 指定クライアント座標で DOWN → hold_ms → UP を注入。
fn click(process: &str, method: InputMethod, x: i32, y: i32, hold_ms: u64) -> Result<(), AdbError> {
    let hwnd = resolve_hwnd(process)?;
    match method {
        InputMethod::SendInput => {
            // SendInput は前景化が前提(物理マウス相当)。AttachThreadInput 併用で確実化。
            if !bring_to_foreground(hwnd) {
                return Err(AdbError::CommandFailed {
                    message: format!(
                        "前景化失敗 (process={process})。別アプリがフォアを握るか SetForegroundWindow 拒否。"
                    ),
                });
            }
            std::thread::sleep(Duration::from_millis(FOREGROUND_SETTLE_MS));
            let (origin_x, origin_y) = client_to_screen_abs(hwnd, 0, 0);
            let sent = sendinput_click(origin_x + x, origin_y + y, hold_ms);
            if sent != 2 {
                return Err(AdbError::CommandFailed {
                    message: format!(
                        "SendInput 戻り値<2 (got={sent})。UIPI/デスクトップ分離/管理者権限不足でブロックの疑い。"
                    ),
                });
            }
            Ok(())
        }
        InputMethod::PostMessage => {
            // 背面送信。前景化不要。lParam はクライアント座標(x,y)。
            let (down_ok, up_ok) = postmessage_click(hwnd, x, y, hold_ms);
            if down_ok != 1 || up_ok != 1 {
                return Err(AdbError::CommandFailed {
                    message: format!(
                        "PostMessageW が Err を返しました (down={down_ok}, up={up_ok})"
                    ),
                });
            }
            Ok(())
        }
    }
}

/// Swipe: from→to を SendInput の DOWN + MOVE連打 + UP で近似。
/// PostMessage の場合は WM_MOUSEMOVE を挟みつつ DOWN/UP。
fn swipe(
    process: &str,
    method: InputMethod,
    x1: i32,
    y1: i32,
    x2: i32,
    y2: i32,
    duration_ms: u64,
) -> Result<(), AdbError> {
    let hwnd = resolve_hwnd(process)?;
    let steps = ((duration_ms / SWIPE_STEP_MS).max(1)) as i32;
    match method {
        InputMethod::SendInput => {
            if !bring_to_foreground(hwnd) {
                return Err(AdbError::CommandFailed {
                    message: format!(
                        "前景化失敗 (process={process})。Swipe の SendInput には前景化が必須。"
                    ),
                });
            }
            std::thread::sleep(Duration::from_millis(FOREGROUND_SETTLE_MS));
            let (origin_x, origin_y) = client_to_screen_abs(hwnd, 0, 0);

            let (vw, vh, ox, oy) = virtual_screen();
            let base = MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK | MOUSEEVENTF_MOVE;

            // DOWN @ from
            let (ax0, ay0) = to_absolute(origin_x + x1 - ox, origin_y + y1 - oy, vw, vh);
            let down = unsafe { send_mouse(ax0, ay0, base | MOUSEEVENTF_LEFTDOWN) };
            if down == 0 {
                return Err(AdbError::CommandFailed {
                    message: "Swipe: SendInput(MOUSEEVENTF_LEFTDOWN) 挿入失敗".to_string(),
                });
            }

            // MOVE 連打で from→to を線形補間
            for i in 1..=steps {
                let t = i as f64 / steps as f64;
                let cx = (origin_x as f64 + x1 as f64 + (x2 - x1) as f64 * t).round() as i32;
                let cy = (origin_y as f64 + y1 as f64 + (y2 - y1) as f64 * t).round() as i32;
                let (ax, ay) = to_absolute(cx - ox, cy - oy, vw, vh);
                let _ = unsafe { send_mouse(ax, ay, base | MOUSEEVENTF_MOVE) };
                std::thread::sleep(Duration::from_millis(SWIPE_STEP_MS));
            }

            // UP @ to
            let (ax1, ay1) = to_absolute(origin_x + x2 - ox, origin_y + y2 - oy, vw, vh);
            let up = unsafe { send_mouse(ax1, ay1, base | MOUSEEVENTF_LEFTUP) };
            if up == 0 {
                return Err(AdbError::CommandFailed {
                    message: "Swipe: SendInput(MOUSEEVENTF_LEFTUP) 挿入失敗".to_string(),
                });
            }
            Ok(())
        }
        InputMethod::PostMessage => {
            // 背面送信。WM_MOUSEMOVE で経路を再現しつつ DOWN/UP。
            let lp1 = make_lparam(x1, y1);
            let lp2 = make_lparam(x2, y2);
            let (sx1, sy1) = client_to_screen_abs(hwnd, x1, y1);
            unsafe {
                let _ = SetCursorPos(sx1, sy1);
                let _ = PostMessageW(Some(hwnd), WM_MOUSEMOVE, WPARAM(0), lp1);
                let down = PostMessageW(Some(hwnd), WM_LBUTTONDOWN, WPARAM(MK_LBUTTON), lp1);
                if down.is_err() {
                    return Err(AdbError::CommandFailed {
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
                    return Err(AdbError::CommandFailed {
                        message: "Swipe: PostMessage(WM_LBUTTONUP) 失敗".to_string(),
                    });
                }
            }
            Ok(())
        }
    }
}

// ============================================================================
// 共通ヘルパ(pub(crate) — win32_capture/win32_launch と共有想定)
// ============================================================================

/// プロセス名(大文字小文字区別なし、.exe 含む)から PID を解決する。
/// 共通ヘルパ `win32_proc::find_pid_by_name` へ委譲(issue#2 で重複解消)。
pub(crate) fn find_pid_by_name(name: &str) -> Option<u32> {
    super::win32_proc::find_pid_by_name(name)
}

/// EnumWindows コールバック間で受け渡す状態(static mut を避け lparam 経由でスレッドセーフ化)。
struct HwndSearchState {
    target_pid: u32,
    found: Option<HWND>,
}

/// PID に属する可視トップレベルウィンドウの HWND を返す。
/// プローブ `find_main_window` と同等だが、static mut を排し lparam 経由で状態を受け渡す
/// (spawn_blocking で複数スレッドから呼ばれても競合しない)。
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

/// EnumWindows コールバック。指定 PID に属する最初の可視ウィンドウを記録して列挙を止める。
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

/// プロセス名 → PID → HWND を一括解決。失敗時は CommandFailed へ包む。
fn resolve_hwnd(process: &str) -> Result<HWND, AdbError> {
    let pid = find_pid_by_name(process).ok_or_else(|| AdbError::CommandFailed {
        message: format!(
            "プロセスが見つかりません ({process})。ゲームを起動してから再実行してください。"
        ),
    })?;
    find_main_window(pid).ok_or_else(|| AdbError::CommandFailed {
        message: format!("PID {pid} に紐づく可視ウィンドウが見つかりません ({process})。ウィンドウが最小化/非表示の可能性。"),
    })
}

/// DPI アウェア化(冪等)。Per-Monitor V2 → 失敗時 SetProcessDPIAware へフォールバック。
/// capture 側でも呼ばれる想定で、二重に呼んでも安全。
pub(crate) fn enable_dpi_awareness() {
    unsafe {
        if SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2).is_err() {
            let _ = SetProcessDPIAware();
        }
    }
}

// ============================================================================
// 画面メトリクス / 座標変換(プローブ検証済み)
// ============================================================================

/// 仮想デスクトップ全体(全モニタ結合)の (幅, 高さ, 左上x, 左上y) を返す。
/// MOUSEEVENTF_VIRTUALDESK と一貫させるためプライマリのみではなく仮想デスクトップ基準。
pub(crate) fn virtual_screen() -> (i32, i32, i32, i32) {
    unsafe {
        let vw = GetSystemMetrics(SM_CXVIRTUALSCREEN);
        let vh = GetSystemMetrics(SM_CYVIRTUALSCREEN);
        let ox = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let oy = GetSystemMetrics(SM_YVIRTUALSCREEN);
        (vw, vh, ox, oy)
    }
}

/// 画面絶対座標(px, 仮想デスクトップ原点基準) → 仮想デスクトップ絶対座標(0..65535)。
/// MSDN 公式: (px * 65535) / (extent - 1)。
pub(crate) fn to_absolute(px: i32, py: i32, vw: i32, vh: i32) -> (i32, i32) {
    let vw1 = (vw as i64 - 1).max(1);
    let vh1 = (vh as i64 - 1).max(1);
    let dx = ((px as i64 * 65535) / vw1) as i32;
    let dy = ((py as i64 * 65535) / vh1) as i32;
    (dx, dy)
}

/// クライアント座標 → 画面全体座標(物理px)。ClientToScreen 失敗時は GetWindowRect 左上で補完。
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

/// HWND を前景化(AttachThreadInput 併用で確実化)。成功/失敗を bool で返す。
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

// ============================================================================
// SendInput / PostMessage 注入(プローブ検証済み)
// ============================================================================

/// 指定 dwFlags でマウスイベント1つを SendInput。戻り値=挿入されたイベント数。
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

/// 画面絶対座標(px, 画面左上原点=仮想デスクトップ原点) で DOWN→hold→UP を送信。
/// DOWN/UP 各1の挿入成功で計2。片方失敗なら 0 or 1。
pub(crate) fn sendinput_click(screen_x: i32, screen_y: i32, hold_ms: u64) -> u32 {
    let (vw, vh, ox, oy) = virtual_screen();
    let (ax, ay) = to_absolute(screen_x - ox, screen_y - oy, vw, vh);
    let base = MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK | MOUSEEVENTF_MOVE;
    let down = unsafe { send_mouse(ax, ay, base | MOUSEEVENTF_LEFTDOWN) };
    std::thread::sleep(Duration::from_millis(hold_ms));
    let up = unsafe { send_mouse(ax, ay, base | MOUSEEVENTF_LEFTUP) };
    down + up
}

/// PostMessage 用 lParam 組立。MAKELPARAM(y, x) = (y << 16) | (x & 0xFFFF)。
fn make_lparam(client_x: i32, client_y: i32) -> LPARAM {
    let v = (((client_y as u32) << 16) | ((client_x as u32) & 0xFFFF)) as isize;
    LPARAM(v)
}

/// PostMessage で背面クリック(DOWN→hold→UP)。WM_MOUSEMOVE と SetCursorPos でホバー状態を確立。
/// 戻り値 (down_ok, up_ok)。各 Ok で 1、Err で 0。
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
}
