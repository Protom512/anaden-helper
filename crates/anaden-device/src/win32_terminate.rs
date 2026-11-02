#![cfg(windows)]
//! 既存ゲームプロセスの段階的終了 (Issue #266)。
//!
//! リカバリー経路 (`--recover-launch`) で従来いきなり `TerminateProcess` を使った結果、
//! ゲーム (ANOTHER EDEN PC) の LevelDB が異常終了状態で残り 0xc0000409 クラッシュループ
//! (brick) に陥った (2026-11-03 live セッション。証跡は issue #260 のコメント)。
//! 本モジュールは終了を 2 段階に分離し、強制終了を最終手段へ退ける:
//!
//! 1. **stage 1 (graceful)**: 対象 PID の全トップレベルウィンドウへ `WM_CLOSE` を投稿し、
//!    `close_wait` 以内の退出をポーリングで待つ。ゲームが自ら保存を済ませて終了する経路。
//! 2. **stage 2 (forced)**: stage 1 の猶予内に退出しない場合 (フリーズ・メッセージポンプ
//!    停止) のみ `TerminateProcess` を発行し、`exit_wait` 以内の消失を確認する。
//!
//! 段階の遷移はすべて tracing ログで明示する。stage 2 到達は「強制終了した = LevelDB 破損の
//! 可能性が残る」ことを意味するため warn で記録する。
//!
//! ポーリングは `std::thread::sleep` を使う blocking 関数群で、`Win32Launch::restart_app`
//! 経由では `spawn_blocking` 上でのみ実行される。テストからは猶予を [`TerminateStages`]
//! で注入し、ダミー子プロセスの終了コード (graceful=0 / 強制=1) で到達段階を検証する
//! (`tests/win32_terminate_tests.rs`)。

use std::time::{Duration, Instant};

use tracing::{info, warn};
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, WPARAM};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowThreadProcessId, PostMessageW, WM_CLOSE,
};
use windows::core::BOOL;

use crate::error::DeviceError;

/// stage 1 (WM_CLOSE) の猶予既定値。issue #266 の指示 (5〜10 秒目安) の中間。
pub const DEFAULT_CLOSE_WAIT: Duration = Duration::from_secs(8);

/// 消失確認ポーリングの間隔 (win32_launch の監視ポーリングと同じ値)。
const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// stage 2 の `TerminateProcess` に渡す exit code。
/// テストはこれ (強制=1) と graceful 退出 (0) の違いで到達した段階を判別する。
const FORCE_EXIT_CODE: u32 = 1;

/// 段階的終了の猶予設定。`close_wait` と `exit_wait` は隣り合う Duration 引数として
/// 渡すと取り違えが起きやすいため、名前付きフィールドで受け渡す。
#[derive(Debug, Clone, Copy)]
pub struct TerminateStages {
    /// stage 1: `WM_CLOSE` 投稿後の graceful 退出待ち。
    pub close_wait: Duration,
    /// stage 2: `TerminateProcess` 発行後の消失確認待ち。
    pub exit_wait: Duration,
}

/// プロセス名 (`child`・大文字小文字無視) で既存の子プロセスを段階的に終了させる。
///
/// - 子が存在しなければ何もせず `Ok`(冪等)。
/// - stage 1 で全インスタンスが退出すれば `Ok`(この場合 `TerminateProcess` は未使用)。
/// - stage 2 で消失を確認すれば `Ok`(強制終了したことを warn ログに残す)。
/// - `close_wait` + `exit_wait` を経過しても残存すれば `Err`(fail-closed。呼び出し元の
///   `restart_app` は spawn を中止し、二重起動を防止する)。
///
/// スナップショット取得に失敗した場合は既存実装と同じく即座に `Err` を返す。
pub fn terminate_child_staged(child: &str, stages: TerminateStages) -> Result<(), DeviceError> {
    let pids = child_pids(child)?;
    if pids.is_empty() {
        info!("Win32Terminate: {child} は存在しない — 終了処理をスキップ (冪等)");
        return Ok(());
    }

    // stage 1: graceful。WM_CLOSE を届けられるウィンドウが無いプロセスは応じようが
    // ないため、投稿 0 件なら猶予待ちせず stage 2 へ進む (純粋な遅延になるだけ)。
    info!("Win32Terminate: stage 1/2 開始 — {child} (pids={pids:?}) へ WM_CLOSE (graceful) を投稿");
    let posted = post_wm_close_to_pids(&pids);
    if posted > 0 && wait_until_gone(child, stages.close_wait)? {
        info!(
            "Win32Terminate: stage 1/2 完了 — {child} は WM_CLOSE で graceful 退出 \
             (TerminateProcess 未使用)"
        );
        return Ok(());
    }
    if posted == 0 {
        warn!(
            "Win32Terminate: {child} (pids={pids:?}) にはトップレベルウィンドウが無く \
             WM_CLOSE を送れなかった — stage 2/2 へ直行"
        );
    }

    // stage 2: 最終手段。
    warn!(
        "Win32Terminate: stage 2/2 開始 — {child} が close_wait={:?} 以内に退出しないため \
         TerminateProcess (強制終了・LevelDB 破損リスクあり・Issue #266)",
        stages.close_wait
    );
    force_terminate_until_gone(child, stages.exit_wait)
}

/// `child` 名 (大文字小文字無視) に一致する全 PID を返す。
/// スナップショット失敗は [`DeviceError::CommandFailed`] で伝播 (fail-closed)。
fn child_pids(child: &str) -> Result<Vec<u32>, DeviceError> {
    let entries =
        super::win32_proc::snapshot_processes().map_err(|e| DeviceError::CommandFailed {
            message: format!("Win32Terminate: プロセススナップショット失敗: {e}"),
        })?;
    Ok(entries
        .iter()
        .filter(|e| e.name.eq_ignore_ascii_case(child))
        .map(|e| e.pid)
        .collect())
}

/// `wait` の期限まで `child` 名の消失を `POLL_INTERVAL` でポーリングする。
/// 消失を確認できれば `Ok(true)`、期限切れなら `Ok(false)`。
fn wait_until_gone(child: &str, wait: Duration) -> Result<bool, DeviceError> {
    let deadline = Instant::now() + wait;
    loop {
        if child_pids(child)?.is_empty() {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// stage 2 本体。旧 `terminate_child_blocking` の強制終了ループと同一 semantics:
/// ループ毎に再スナップショットし、残存 PID へ `TerminateProcess` を発行し続ける
/// (権限エラー等の一時失敗は warn のみで再試行)。消失すれば `Ok`、`exit_wait` を
/// 経過しても残存すれば `Err`(fail-closed)。
fn force_terminate_until_gone(child: &str, exit_wait: Duration) -> Result<(), DeviceError> {
    let deadline = Instant::now() + exit_wait;
    loop {
        let pids = child_pids(child)?;
        if pids.is_empty() {
            info!("Win32Terminate: stage 2/2 完了 — 強制終了により {child} の消失を確認");
            return Ok(());
        }
        info!(
            "Win32Terminate: stage 2/2 — {child} へ TerminateProcess (pids={pids:?}, exit_code={FORCE_EXIT_CODE})"
        );
        for pid in pids {
            if let Err(e) = terminate_pid(pid) {
                warn!("Win32Terminate: TerminateProcess({pid}) 失敗 (次ポーリングで再試行): {e}");
            }
        }
        if Instant::now() >= deadline {
            return Err(DeviceError::CommandFailed {
                message: format!(
                    "Win32Terminate: {child} が強制終了後も exit_wait={exit_wait:?} 以内に\
                     消失しない (二重起動防止のため spawn 中止 — fail-closed)"
                ),
            });
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// 指定 PID へ終了要求 (`TerminateProcess`・exit code [`FORCE_EXIT_CODE`]) を出す。
pub(crate) fn terminate_pid(pid: u32) -> windows::core::Result<()> {
    unsafe {
        let handle = OpenProcess(PROCESS_TERMINATE, false, pid)?;
        let result = TerminateProcess(handle, FORCE_EXIT_CODE);
        let _ = CloseHandle(handle);
        result
    }
}

/// `pids` 各々の全トップレベルウィンドウへ `WM_CLOSE` を投稿し、投稿に成功した
/// ウィンドウ数の合計を返す (0 = 誰にも届けられなかった)。
fn post_wm_close_to_pids(pids: &[u32]) -> usize {
    let mut posted = 0usize;
    for &pid in pids {
        let hwnds = top_level_windows_of(pid);
        for hwnd in &hwnds {
            // Result を握りつぶさず失敗は warn で残す。graceful できるかは退出観測で判定。
            match unsafe { PostMessageW(Some(*hwnd), WM_CLOSE, WPARAM(0), LPARAM(0)) } {
                Ok(()) => posted += 1,
                Err(e) => warn!("Win32Terminate: WM_CLOSE 投稿失敗 (pid={pid}): {e}"),
            }
        }
        info!(
            "Win32Terminate: WM_CLOSE → pid={pid} のウィンドウ {} 件",
            hwnds.len()
        );
    }
    posted
}

/// EnumWindows コールバック間で受け渡す状態 (win32_input の enum_proc と同じ
/// lparam 経由パターン。static mut を使わない)。
struct WindowsOfPid {
    pid: u32,
    hwnds: Vec<HWND>,
}

/// PID に属する全トップレベルウィンドウを列挙する (可視・不可視を問わない。
/// 凍結ゲームの不可視ウィンドウにも WM_CLOSE を届けるため)。
fn top_level_windows_of(pid: u32) -> Vec<HWND> {
    let mut state = WindowsOfPid {
        pid,
        hwnds: Vec::new(),
    };
    let lparam = LPARAM(&mut state as *mut WindowsOfPid as isize);
    unsafe {
        let _ = EnumWindows(Some(collect_windows_proc), lparam);
    }
    state.hwnds
}

/// EnumWindows コールバック。指定 PID のウィンドウをすべて収集する。
unsafe extern "system" fn collect_windows_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    unsafe {
        let state = &mut *(lparam.0 as *mut WindowsOfPid);
        let mut window_pid: u32 = 0;
        let _ = GetWindowThreadProcessId(hwnd, Some(&mut window_pid as *mut u32));
        if window_pid == state.pid {
            state.hwnds.push(hwnd);
        }
        BOOL(1)
    }
}
