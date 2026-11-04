#![cfg(windows)]

use std::time::{Duration, Instant};

use tracing::{info, warn};
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, WPARAM};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowThreadProcessId, PostMessageW, WM_CLOSE,
};
use windows::core::BOOL;

use crate::error::DeviceError;

pub const DEFAULT_CLOSE_WAIT: Duration = Duration::from_secs(8);

const POLL_INTERVAL: Duration = Duration::from_millis(500);

const FORCE_EXIT_CODE: u32 = 1;

#[derive(Debug, Clone, Copy)]
pub struct TerminateStages {
    pub close_wait: Duration,
    pub exit_wait: Duration,
}

pub fn terminate_child_staged(child: &str, stages: TerminateStages) -> Result<(), DeviceError> {
    let pids = child_pids(child)?;
    if pids.is_empty() {
        info!("Win32Terminate: {child} は存在しない — 終了処理をスキップ (冪等)");
        return Ok(());
    }

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

    warn!(
        "Win32Terminate: stage 2/2 開始 — {child} が close_wait={:?} 以内に退出しないため \
         TerminateProcess (強制終了・LevelDB 破損リスクあり・Issue #266)",
        stages.close_wait
    );
    force_terminate_until_gone(child, stages.exit_wait)
}

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

pub(crate) fn terminate_pid(pid: u32) -> windows::core::Result<()> {
    unsafe {
        let handle = OpenProcess(PROCESS_TERMINATE, false, pid)?;
        let result = TerminateProcess(handle, FORCE_EXIT_CODE);
        let _ = CloseHandle(handle);
        result
    }
}

fn post_wm_close_to_pids(pids: &[u32]) -> usize {
    let mut posted = 0usize;
    for &pid in pids {
        let hwnds = top_level_windows_of(pid);
        for hwnd in &hwnds {
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

struct WindowsOfPid {
    pid: u32,
    hwnds: Vec<HWND>,
}

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
