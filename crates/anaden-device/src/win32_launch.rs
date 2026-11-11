#![cfg(windows)]
#![allow(dead_code)]

use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use tracing::{info, warn};

use windows::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
use windows::Win32::System::Threading::{
    CREATE_NEW_PROCESS_GROUP, GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
};

use crate::ensure::EnsureOutcome;
use crate::error::DeviceError;
#[cfg(test)]
use crate::win32_terminate::terminate_pid;
use crate::win32_terminate::{DEFAULT_CLOSE_WAIT, TerminateStages, terminate_child_staged};

pub const DEFAULT_LAUNCHER: &str =
    r"C:\Program Files\Wright Flyer Studios\ANOTHER EDEN\Launcher\Launcher.exe";

pub const DEFAULT_WORKDIR: &str = r"C:\Program Files\Wright Flyer Studios\ANOTHER EDEN\Game";

pub const DEFAULT_CHILD: &str = "AnotherEden.exe";

pub const DEFAULT_WAIT: Duration = Duration::from_secs(30);

const POLL_INTERVAL: Duration = Duration::from_millis(500);

const STILL_ACTIVE_U32: u32 = STILL_ACTIVE.0 as u32;

#[derive(Clone)]
pub struct Win32Launch {
    launcher: String,
    workdir: String,
    child: String,
}

impl Win32Launch {
    pub fn new(launcher: &str, workdir: &str, child: &str) -> Self {
        Self {
            launcher: launcher.to_string(),
            workdir: workdir.to_string(),
            child: child.to_string(),
        }
    }

    pub fn default_paths() -> Self {
        Self::new(DEFAULT_LAUNCHER, DEFAULT_WORKDIR, DEFAULT_CHILD)
    }

    pub fn child(&self) -> &str {
        &self.child
    }

    pub async fn ensure_open(&self, wait: Duration) -> Result<EnsureOutcome, DeviceError> {
        let launcher = self.launcher.clone();
        let workdir = self.workdir.clone();
        let child = self.child.clone();
        tokio::task::spawn_blocking(move || run_blocking(&launcher, &workdir, &child, wait))
            .await
            .map_err(|e| DeviceError::CommandFailed {
                message: format!("ensure_open の blocking タスクがパニック/中止: {e}"),
            })?
    }

    pub async fn is_alive(&self) -> bool {
        let child = self.child.clone();
        tokio::task::spawn_blocking(move || child_exists(&child))
            .await
            .unwrap_or(false)
    }

    pub async fn launch_app(&self) -> Result<(), DeviceError> {
        let launcher = self.launcher.clone();
        let workdir = self.workdir.clone();
        tokio::task::spawn_blocking(move || spawn_launcher(&launcher, &workdir).map(|_| ()))
            .await
            .map_err(|e| DeviceError::CommandFailed {
                message: format!("launch_app の blocking タスクがパニック/中止: {e}"),
            })?
            .map_err(|e| DeviceError::CommandFailed { message: e })?;
        Ok(())
    }

    pub async fn restart_app(&self, exit_wait: Duration) -> Result<(), DeviceError> {
        let child = self.child.clone();
        tokio::task::spawn_blocking(move || {
            terminate_child_staged(
                &child,
                TerminateStages {
                    close_wait: DEFAULT_CLOSE_WAIT,
                    exit_wait,
                },
            )
        })
        .await
        .map_err(|e| DeviceError::CommandFailed {
            message: format!("restart_app の blocking タスクがパニック/中止: {e}"),
        })??;
        self.launch_app().await
    }
}

fn run_blocking(
    launcher: &str,
    workdir: &str,
    child: &str,
    wait: Duration,
) -> Result<EnsureOutcome, DeviceError> {
    if matches!(precheck_child_running(child)?, Precheck::AlreadyOpen) {
        return Ok(EnsureOutcome::AlreadyOpen);
    }

    let launcher_started = Instant::now();
    let launcher_child = match spawn_launcher(launcher, workdir) {
        Ok(pid) => {
            info!("Win32Launch: Launcher spawn OK (launcher_pid={pid})");
            pid
        }
        Err(msg) => {
            return Err(DeviceError::CommandFailed { message: msg });
        }
    };

    let overall_deadline = launcher_started + wait;

    let (child_pid, child_parent_pid) = match wait_for_child_discovery(child, overall_deadline) {
        ChildDiscovery::Seen(pid, parent_pid) => {
            if parent_pid == launcher_child {
                info!(
                    "Win32Launch: 子発見 pid={pid} parent={parent_pid} (Launcher({launcher_child}) → child, OK)"
                );
            } else {
                warn!(
                    "Win32Launch: 子発見 pid={pid} parent_pid={parent_pid} != launcher_pid={launcher_child} (別世代/既存の可能性)"
                );
            }
            (pid, parent_pid)
        }
        ChildDiscovery::SnapshotFailed => {
            return Err(DeviceError::CommandFailed {
                message:
                    "Win32Launch: CreateToolhelp32Snapshot 呼び出し失敗により子を監視できなかった"
                        .to_string(),
            });
        }
        ChildDiscovery::NotAppeared => {
            warn!(
                "Win32Launch: 子 {child} が {:.0}s 期間内に出現しなかった → Timeout",
                wait.as_secs_f64()
            );
            return Ok(EnsureOutcome::Timeout);
        }
    };

    let watch = monitor_child_alive(child_pid, overall_deadline)?;
    if watch.alive {
        info!(
            "Win32Launch: LAUNCH_OK child_pid={child_pid} survived={}s (STILL_ACTIVE)",
            wait.as_secs()
        );
        Ok(EnsureOutcome::Launched)
    } else {
        warn!(
            "Win32Launch: 子 pid={child_pid} parent={child_parent_pid} が即死: exit_code=0x{:08X} → Timeout",
            watch.last_exit_code
        );
        Ok(EnsureOutcome::Timeout)
    }
}

enum Precheck {
    AlreadyOpen,
    NotRunning,
}

fn precheck_child_running(child: &str) -> Result<Precheck, DeviceError> {
    match snapshot_processes() {
        Ok(entries) => {
            if entries.iter().any(|e| e.name.eq_ignore_ascii_case(child)) {
                info!("Win32Launch: {child} は既に起動中 → AlreadyOpen");
                Ok(Precheck::AlreadyOpen)
            } else {
                Ok(Precheck::NotRunning)
            }
        }
        Err(e) => Err(DeviceError::CommandFailed {
            message: format!("Win32Launch: 事前スナップショット失敗: {e}"),
        }),
    }
}

enum ChildDiscovery {
    Seen(u32, u32),
    NotAppeared,
    SnapshotFailed,
}

fn wait_for_child_discovery(child: &str, deadline: Instant) -> ChildDiscovery {
    while Instant::now() < deadline {
        match snapshot_processes() {
            Ok(entries) => {
                if let Some(entry) = entries.iter().find(|e| e.name.eq_ignore_ascii_case(child)) {
                    return ChildDiscovery::Seen(entry.pid, entry.parent_pid);
                }
            }
            Err(e) => {
                warn!("Win32Launch: CreateToolhelp32Snapshot 失敗: {e}");
                return ChildDiscovery::SnapshotFailed;
            }
        }
        std::thread::sleep(POLL_INTERVAL);
    }
    ChildDiscovery::NotAppeared
}

struct ChildWatch {
    alive: bool,
    last_exit_code: u32,
}

fn monitor_child_alive(child_pid: u32, deadline: Instant) -> Result<ChildWatch, DeviceError> {
    let mut last_exit_code: u32 = STILL_ACTIVE_U32;
    let mut alive = true;
    let mut ever_queried_ok = false;
    while Instant::now() < deadline {
        match query_exit_code(child_pid) {
            Ok(code) => {
                ever_queried_ok = true;
                last_exit_code = code;
                if code != STILL_ACTIVE_U32 {
                    alive = false;
                    break;
                }
            }
            Err(e) => {
                if ever_queried_ok {
                    warn!(
                        "Win32Launch: OpenProcess 失敗({e}) → 子消失 = 即死と判定 (pid={child_pid})"
                    );
                    alive = false;
                    break;
                }
                return Err(DeviceError::CommandFailed {
                    message: format!("Win32Launch: OpenProcess 呼び出し失敗(初回から不可): {e}"),
                });
            }
        }
        std::thread::sleep(POLL_INTERVAL);
    }
    Ok(ChildWatch {
        alive,
        last_exit_code,
    })
}

fn spawn_launcher(launcher: &str, workdir: &str) -> Result<u32, String> {
    let mut cmd = Command::new(launcher);
    cmd.current_dir(workdir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NEW_PROCESS_GROUP.0);

    cmd.spawn()
        .map(|child| {
            let pid = child.id();
            let _ = child;
            pid
        })
        .map_err(|e| format!("Launcher spawn 失敗: {e} (launcher={launcher}, workdir={workdir})"))
}

fn child_exists(child: &str) -> bool {
    match snapshot_processes() {
        Ok(entries) => entries.iter().any(|e| e.name.eq_ignore_ascii_case(child)),
        Err(_) => false,
    }
}

fn snapshot_processes() -> windows::core::Result<Vec<super::win32_proc::ProcEntry>> {
    super::win32_proc::snapshot_processes()
}

fn query_exit_code(pid: u32) -> windows::core::Result<u32> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid)?;
        let mut code: u32 = 0;
        let r = GetExitCodeProcess(handle, &mut code);
        let _ = CloseHandle(handle);
        r.map(|_| code)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use std::os::windows::process::CommandExt;

    #[tokio::test]
    async fn restart_app_terminates_existing_child_before_spawn() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let uniq = std::process::id();
        let child_name = format!("anaden_t210_child_{uniq}.exe");
        let launcher_name = format!("anaden_t210_launcher_{uniq}.exe");
        let system_root = std::env::var_os("SystemRoot").expect("SystemRoot");
        let ping = std::path::Path::new(&system_root)
            .join("System32")
            .join("ping.exe");
        let child_exe = tmp.path().join(&child_name);
        let launcher_exe = tmp.path().join(&launcher_name);
        std::fs::copy(&ping, &child_exe).expect("copy ping as dummy child");
        std::fs::copy(&ping, &launcher_exe).expect("copy ping as dummy launcher");

        let mut spawned = Command::new(&child_exe)
            .args(["-n", "61", "127.0.0.1"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NEW_PROCESS_GROUP.0)
            .spawn()
            .expect("spawn dummy child");
        let appear_deadline = Instant::now() + Duration::from_secs(10);
        while !child_exists(&child_name) {
            assert!(
                Instant::now() < appear_deadline,
                "dummy child did not appear in process list"
            );
            std::thread::sleep(Duration::from_millis(100));
        }

        let launch = Win32Launch::new(
            launcher_exe.to_str().expect("launcher path utf8"),
            tmp.path().to_str().expect("tempdir path utf8"),
            &child_name,
        );
        launch
            .restart_app(Duration::from_secs(15))
            .await
            .expect("restart_app must terminate child then spawn launcher");

        assert!(
            !child_exists(&child_name),
            "existing child must be terminated (and confirmed gone) before spawn"
        );
        let _ = spawned.wait();

        let gone_deadline = Instant::now() + Duration::from_secs(10);
        while child_exists(&launcher_name) {
            if Instant::now() >= gone_deadline {
                if let Ok(entries) = snapshot_processes() {
                    for e in entries
                        .into_iter()
                        .filter(|e| e.name.eq_ignore_ascii_case(&launcher_name))
                    {
                        let _ = terminate_pid(e.pid, &launcher_name);
                    }
                }
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(
            !child_exists(&launcher_name),
            "dummy launcher must not linger"
        );
    }
}
