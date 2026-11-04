#![cfg(windows)]

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{CloseHandle, HANDLE, STILL_ACTIVE};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{
    CREATE_NEW_PROCESS_GROUP, GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
};

const DEFAULT_LAUNCHER: &str =
    r"C:\Program Files\Wright Flyer Studios\ANOTHER EDEN\Launcher\Launcher.exe";
const DEFAULT_WORKDIR: &str = r"C:\Program Files\Wright Flyer Studios\ANOTHER EDEN\Game";
const DEFAULT_CHILD: &str = "AnotherEden.exe";
const DEFAULT_WAIT_SECS: u64 = 30;
const POLL_INTERVAL: Duration = Duration::from_millis(500);

const PASS_TAG: &str = "[PASS]";
const FAIL_TAG: &str = "[FAIL]";
const MANUAL_TAG: &str = "[MANUAL]";

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut wait_secs: u64 = DEFAULT_WAIT_SECS;
    let mut child_name: String = DEFAULT_CHILD.to_string();
    let mut launcher: String = DEFAULT_LAUNCHER.to_string();
    let mut workdir: String = DEFAULT_WORKDIR.to_string();

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--wait" => {
                if let Some(v) = args.get(i + 1)
                    && let Ok(n) = v.parse()
                {
                    wait_secs = n;
                }
                i += 2;
            }
            "--child" => {
                if let Some(v) = args.get(i + 1) {
                    child_name = v.clone();
                }
                i += 2;
            }
            "--launcher" => {
                if let Some(v) = args.get(i + 1) {
                    launcher = v.clone();
                }
                i += 2;
            }
            "--workdir" => {
                if let Some(v) = args.get(i + 1) {
                    workdir = v.clone();
                }
                i += 2;
            }
            other => {
                eprintln!("warn: 不明な引数を無視: {other}");
                i += 1;
            }
        }
    }

    let wait = Duration::from_secs(wait_secs);
    println!("== probe_windows_launch ==");
    println!("launcher : {launcher}");
    println!("workdir  : {workdir}");
    println!("child    : {child_name}");
    println!("wait     : {}s", wait_secs);

    let code = run(&launcher, &workdir, &child_name, wait);
    std::process::exit(code);
}

fn run(launcher: &str, workdir: &str, child_name: &str, wait: Duration) -> i32 {
    if let Err(code) = precheck_existing_child(child_name) {
        return code;
    }

    let launcher_started = Instant::now();
    let launcher_child = match spawn_launcher_probe(launcher, workdir) {
        Ok(pid) => pid,
        Err(code) => return code,
    };

    let overall_deadline = launcher_started + wait;
    let (child_pid, child_parent_pid) = match wait_discovery_probe(
        child_name,
        overall_deadline,
        launcher_started,
        launcher_child,
    ) {
        Ok(v) => v,
        Err(code) => return code,
    };

    let (alive, last_exit_code) = match monitor_survival(child_pid, overall_deadline) {
        Ok(v) => v,
        Err(code) => return code,
    };

    probe_verdict(
        alive,
        child_pid,
        child_parent_pid,
        launcher_child,
        last_exit_code,
        wait,
        launcher_started,
        child_name,
    )
}

fn precheck_existing_child(child_name: &str) -> Result<(), i32> {
    match snapshot_processes() {
        Ok(entries) => {
            let preexisting = entries.iter().find(|e| eq_ignore_case(&e.name, child_name));
            if let Some(e) = preexisting {
                eprintln!(
                    "{FAIL_TAG} 既存プロセス検出: {child_name} が既に起動中 (pid={} parent_pid={})",
                    e.pid, e.parent_pid
                );
                eprintln!("     → exit 50: ゲーム既存プロセス検出。検証前に全て終了すること。");
                return Err(50);
            }
            Ok(())
        }
        Err(e) => {
            eprintln!("{FAIL_TAG} 事前スナップショット失敗: {e}");
            eprintln!("     → exit 40: windows-rs feature / 昇格を確認。");
            Err(40)
        }
    }
}

fn spawn_launcher_probe(launcher: &str, workdir: &str) -> Result<u32, i32> {
    let spawn_result = Command::new(launcher)
        .current_dir(workdir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NEW_PROCESS_GROUP.0)
        .spawn();

    match spawn_result {
        Ok(c) => {
            let pid = c.id();
            println!("[1] Launcher spawn OK (launcher_pid={pid})");
            let _ = c;
            Ok(pid)
        }
        Err(e) => {
            eprintln!(
                "{FAIL_TAG} Launcher spawn 失敗: {e} (launcher={launcher}, workdir={workdir})"
            );
            eprintln!("     → exit 10: パス/権限を確認してください。");
            Err(10)
        }
    }
}

fn wait_discovery_probe(
    child_name: &str,
    deadline: Instant,
    launcher_started: Instant,
    launcher_child: u32,
) -> Result<(u32, u32), i32> {
    let mut first_seen: Option<(u32, u32)> = None;
    let mut snapshot_failed = false;

    while Instant::now() < deadline {
        match snapshot_processes() {
            Ok(entries) => {
                for entry in &entries {
                    if eq_ignore_case(&entry.name, child_name) {
                        first_seen = Some((entry.pid, entry.parent_pid));
                        break;
                    }
                }
                if first_seen.is_some() {
                    break;
                }
            }
            Err(e) => {
                eprintln!("warn: CreateToolhelp32Snapshot 失敗: {e}");
                snapshot_failed = true;
                break;
            }
        }
        std::thread::sleep(POLL_INTERVAL);
    }

    match first_seen {
        Some(v) => {
            let discover_ms = launcher_started.elapsed().as_millis();
            println!(
                "[2] 子プロセス発見: pid={} parent_pid={} (発見まで {}ms)",
                v.0, v.1, discover_ms
            );
            if v.1 == launcher_child {
                println!("    親子関係: Launcher({launcher_child}) → child (OK)");
            } else {
                println!(
                    "    親子関係メモ: child parent_pid={} != launcher_pid={launcher_child} (別Launcher世代/既存プロセスの可能性)",
                    v.1
                );
            }
            Ok(v)
        }
        None if snapshot_failed => {
            eprintln!("{FAIL_TAG} CreateToolhelp32Snapshot 呼び出し失敗により子を監視できなかった");
            eprintln!("     → exit 40: windows-rs feature / 昇格を確認。");
            Err(40)
        }
        None => {
            let wait_secs = (deadline - launcher_started).as_secs_f64();
            eprintln!(
                "{FAIL_TAG} 子プロセス {child_name} が --wait({wait_secs:.0}s) 期間内に出現しなかった"
            );
            eprintln!(
                "     → exit 20: Launcher が子を起動していない/即座に失敗。KB5094126 回帰疑い。"
            );
            Err(20)
        }
    }
}

fn monitor_survival(child_pid: u32, deadline: Instant) -> Result<(bool, u32), i32> {
    let mut last_exit_code: u32 = STILL_ACTIVE_0;
    let mut alive = true;
    let mut ever_queried_ok = false;

    while Instant::now() < deadline {
        match query_exit_code(child_pid) {
            Ok(code) => {
                ever_queried_ok = true;
                last_exit_code = code;
                if code != STILL_ACTIVE_0 {
                    alive = false;
                    break;
                }
            }
            Err(e) => {
                if ever_queried_ok {
                    eprintln!(
                        "warn: OpenProcess 失敗: {e} → 直前まで観測可能だった子が消失 = 即死と判定"
                    );
                    alive = false;
                    break;
                }
                eprintln!("{FAIL_TAG} OpenProcess 呼び出し失敗(初回から不可): {e}");
                eprintln!("     → exit 40: 権限/feature を確認。");
                return Err(40);
            }
        }
        std::thread::sleep(POLL_INTERVAL);
    }
    Ok((alive, last_exit_code))
}

#[allow(clippy::too_many_arguments)]
fn probe_verdict(
    alive: bool,
    child_pid: u32,
    child_parent_pid: u32,
    launcher_child: u32,
    last_exit_code: u32,
    wait: Duration,
    launcher_started: Instant,
    child_name: &str,
) -> i32 {
    let survived_secs: f64 = if alive {
        wait.as_secs_f64()
    } else {
        launcher_started.elapsed().as_secs_f64()
    };

    if alive {
        println!(
            "{PASS_TAG} LAUNCH_OK child_pid={child_pid} survived={}s (exit_code=STILL_ACTIVE 0x103)",
            wait.as_secs()
        );
        print_summary(
            child_pid,
            child_parent_pid,
            launcher_child,
            0x103,
            survived_secs,
            true,
        );
        println!(
            "{MANUAL_TAG} ゲーム画面が実際に描画されタイトル/プレイ可能状態に到達したかを目視してください(プロセスが生きていても黒画面やロード停止の可能性あり)。これは probe_windows_capture に委ねる。"
        );
        return 0;
    }

    eprintln!(
        "{FAIL_TAG} 子プロセス {child_name}(pid={child_pid}) が即死: exit_code=0x{:08X}",
        last_exit_code
    );
    eprintln!(
        "     → exit 30: 0xC0000005 等の NT STATUS。launcher-opengl-fix.md の WoW64 クラッシュ回帰疑い。"
    );
    print_summary(
        child_pid,
        child_parent_pid,
        launcher_child,
        last_exit_code,
        survived_secs,
        false,
    );
    30
}

const STILL_ACTIVE_0: u32 = STILL_ACTIVE.0 as u32;

fn print_summary(
    child_pid: u32,
    child_parent_pid: u32,
    launcher_pid: u32,
    exit_code: u32,
    survived_secs: f64,
    alive: bool,
) {
    let status = if alive { "ALIVE(STILL_ACTIVE)" } else { "DEAD" };
    println!(
        "SUMMARY child_pid={child_pid} child_parent_pid={child_parent_pid} launcher_pid={launcher_pid} exit_code=0x{:08X} survived={:.2}s status={status}",
        exit_code, survived_secs
    );
}

struct ProcEntry {
    pid: u32,
    parent_pid: u32,
    name: String,
}

fn snapshot_processes() -> windows::core::Result<Vec<ProcEntry>> {
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)?;
        let result = enum_inner(snapshot);
        let _ = CloseHandle(snapshot);
        result
    }
}

unsafe fn enum_inner(snapshot: HANDLE) -> windows::core::Result<Vec<ProcEntry>> {
    unsafe {
        let mut entries: Vec<ProcEntry> = Vec::new();
        let mut pe: PROCESSENTRY32W = std::mem::zeroed();
        pe.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;

        if Process32FirstW(snapshot, &mut pe).is_err() {
            return Err(windows::core::Error::from_thread());
        }

        loop {
            let name = wsz_to_string(&pe.szExeFile);
            let pid = pe.th32ProcessID;
            let parent_pid = pe.th32ParentProcessID;
            entries.push(ProcEntry {
                pid,
                parent_pid,
                name,
            });

            if Process32NextW(snapshot, &mut pe).is_err() {
                break;
            }
        }

        Ok(entries)
    }
}

#[allow(dead_code)]
#[inline]
fn read_pids(pe: &PROCESSENTRY32W) -> (u32, u32) {
    (pe.th32ProcessID, pe.th32ParentProcessID)
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

fn wsz_to_string(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

fn eq_ignore_case(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

#[allow(dead_code)]
fn to_wide<S: AsRef<OsStr>>(s: S) -> Vec<u16> {
    s.as_ref().encode_wide().chain(std::iter::once(0)).collect()
}
