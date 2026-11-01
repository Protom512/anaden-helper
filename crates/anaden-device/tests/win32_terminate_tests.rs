#![cfg(windows)]
#![allow(clippy::unwrap_used)]
#![allow(clippy::panic)]
#![allow(clippy::expect_used)]

use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anaden_device::{TerminateStages, terminate_child_staged};

const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

struct DummyChild {
    _dir: tempfile::TempDir,
    exe: PathBuf,
    ready: PathBuf,
    name: String,
}

fn dummy_child(tag: &str) -> DummyChild {
    let dir = tempfile::tempdir().expect("tempdir");
    let uniq = std::process::id();
    let name = format!("anaden_t266_{tag}_{uniq}.exe");
    let exe = dir.path().join(&name);
    std::fs::copy(env!("CARGO_BIN_EXE_graceful_child"), &exe).expect("copy graceful_child fixture");
    let ready = dir.path().join(format!("ready_{tag}_{uniq}.txt"));
    DummyChild {
        _dir: dir,
        exe,
        ready,
        name,
    }
}

fn spawn_ready(dummy: &DummyChild, mode: &str, close_delay_ms: u64) -> std::process::Child {
    let mut child = Command::new(&dummy.exe)
        .args([
            mode,
            dummy.ready.to_str().expect("ready path utf8"),
            &close_delay_ms.to_string(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NEW_PROCESS_GROUP)
        .spawn()
        .expect("spawn dummy child");
    let deadline = Instant::now() + Duration::from_secs(20);
    while !dummy.ready.exists() {
        assert!(
            Instant::now() < deadline,
            "dummy child did not become ready: {}",
            dummy.ready.display()
        );
        if let Some(status) = child.try_wait().expect("try_wait dummy child") {
            panic!("dummy child exited before ready: {status}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    child
}

#[test]
fn graceful_exit_never_reaches_terminate_process() {
    let dummy = dummy_child("graceful");
    let mut child = spawn_ready(&dummy, "close", 0);

    terminate_child_staged(
        &dummy.name,
        TerminateStages {
            close_wait: Duration::from_secs(5),
            exit_wait: Duration::from_secs(5),
        },
    )
    .expect("staged terminate must succeed");

    let status = child.wait().expect("wait dummy child");
    assert_eq!(
        status.code(),
        Some(0),
        "graceful 退出は exit 0。1 なら TerminateProcess が到達していた"
    );
}

#[test]
fn wm_close_ignored_escalates_to_terminate_process_after_grace() {
    let dummy = dummy_child("ignore");
    let mut child = spawn_ready(&dummy, "ignore", 0);
    let close_wait = Duration::from_millis(700);

    let started = Instant::now();
    terminate_child_staged(
        &dummy.name,
        TerminateStages {
            close_wait,
            exit_wait: Duration::from_secs(10),
        },
    )
    .expect("staged terminate must succeed via force");
    let elapsed = started.elapsed();

    let status = child.wait().expect("wait dummy child");
    assert_eq!(
        status.code(),
        Some(1),
        "WM_CLOSE 無視子は TerminateProcess (exit 1) で終わる必要がある"
    );
    assert!(
        elapsed >= close_wait,
        "graceful 猶予 ({close_wait:?}) より早く強制終了した: {elapsed:?}"
    );
}

#[test]
fn delayed_exit_during_grace_wait_is_detected_without_escalation() {
    let dummy = dummy_child("delayed");
    let close_delay = Duration::from_millis(1500);
    let mut child = spawn_ready(&dummy, "close", close_delay.as_millis() as u64);

    let started = Instant::now();
    terminate_child_staged(
        &dummy.name,
        TerminateStages {
            close_wait: Duration::from_secs(6),
            exit_wait: Duration::from_secs(2),
        },
    )
    .expect("staged terminate must succeed gracefully");
    let elapsed = started.elapsed();

    let status = child.wait().expect("wait dummy child");
    assert_eq!(
        status.code(),
        Some(0),
        "遅延退出でも猶予内なら graceful (exit 0)。1 なら猶予待ちが機能していない"
    );
    assert!(
        elapsed >= close_delay,
        "子は実際に {close_delay:?} 遅延したはず (測定 {elapsed:?})"
    );
}

#[test]
fn no_existing_child_returns_ok_immediately() {
    let absent = format!("anaden_t266_absent_{}.exe", std::process::id());
    terminate_child_staged(
        &absent,
        TerminateStages {
            close_wait: Duration::from_millis(50),
            exit_wait: Duration::from_millis(50),
        },
    )
    .expect("absent child must be a no-op Ok");
}
