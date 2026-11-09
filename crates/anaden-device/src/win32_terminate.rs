#![cfg(windows)]

use std::path::Path;
use std::time::{Duration, Instant};

use tracing::{info, warn};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM, WPARAM};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
    QueryFullProcessImageNameW, TerminateProcess,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowThreadProcessId, PostMessageW, WM_CLOSE,
};
use windows::core::{BOOL, PWSTR};

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
            if let Err(e) = terminate_pid(pid, child) {
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

pub(crate) fn terminate_pid(pid: u32, child: &str) -> Result<(), DeviceError> {
    let handle = unsafe {
        OpenProcess(
            PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION,
            false,
            pid,
        )
    }
    .map_err(|e| DeviceError::CommandFailed {
        message: format!("Win32Terminate: OpenProcess(pid={pid}) 失敗: {e}"),
    })?;
    let outcome = terminate_handle_if_matches(handle, pid, child);
    unsafe {
        let _ = CloseHandle(handle);
    }
    outcome
}

fn terminate_handle_if_matches(handle: HANDLE, pid: u32, child: &str) -> Result<(), DeviceError> {
    let Some(image) = process_image_name(handle) else {
        warn!(
            "Win32Terminate: pid={pid} のイメージ名取得に失敗 — 終了対象を特定できず terminate を見送り (fail-closed)"
        );
        return Ok(());
    };
    if !image_name_matches(child, &image) {
        warn!(
            "Win32Terminate: pid={pid} のイメージ名 {image} は {child} と不一致 — pid 再利用の可能性、terminate を見送り"
        );
        return Ok(());
    }
    unsafe { TerminateProcess(handle, FORCE_EXIT_CODE) }.map_err(|e| DeviceError::CommandFailed {
        message: format!("Win32Terminate: TerminateProcess(pid={pid}) 失敗: {e}"),
    })
}

fn process_image_name(handle: HANDLE) -> Option<String> {
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    let result = unsafe {
        QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
    };
    if result.is_err() {
        return None;
    }
    let len = (len as usize).min(buf.len());
    Some(String::from_utf16_lossy(&buf[..len]))
}

fn image_name_matches(child: &str, image_path: &str) -> bool {
    Path::new(image_path)
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case(child))
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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::expect_used)]
#[allow(clippy::panic)]
mod tests {
    use std::process::{Command, Stdio};

    use super::*;

    #[test]
    fn image_name_matches_bare_file_name_case_insensitively() {
        assert!(image_name_matches(
            "anaden.exe",
            r"C:\apps\Anaden\anaden.EXE"
        ));
        assert!(image_name_matches(
            "ANADEN.EXE",
            r"C:\apps\Anaden\anaden.exe"
        ));
    }

    #[test]
    fn image_name_matches_rejects_other_names_and_extensions() {
        assert!(!image_name_matches(
            "anaden.exe",
            r"C:\Windows\System32\cmd.exe"
        ));
        assert!(!image_name_matches("anaden.exe", r"C:\apps\anaden.exe.bak"));
    }

    #[test]
    fn terminate_pid_skips_pid_whose_image_does_not_match_child() {
        let mut probe = Command::new("powershell.exe")
            .args(["-NoProfile", "-Command", "Start-Sleep -Seconds 30"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn probe powershell");
        let pid = probe.id();

        let absent = format!("anaden_t111_absent_{}.exe", std::process::id());
        terminate_pid(pid, &absent).expect("image-name mismatch must skip, not error");
        assert!(
            probe.try_wait().expect("try_wait probe").is_none(),
            "mismatched image name must not terminate the probe"
        );

        terminate_pid(pid, "powershell.exe").expect("matching image name must terminate");
        let status = probe.wait().expect("wait probe after terminate");
        assert_eq!(status.code(), Some(FORCE_EXIT_CODE as i32));
    }
}
