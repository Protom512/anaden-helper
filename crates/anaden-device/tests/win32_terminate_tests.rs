//! `terminate_child_staged` の段階的終了の振る舞い検証 (Issue #266)。
//!
//! 実ゲームは使わない。ダミー子プロセス (`CARGO_BIN_EXE_graceful_child` を固有名へ
//! コピーしたもの) で検証する。判定の原理:
//!
//! - stage 2 の `TerminateProcess` は exit code **1** で殺す。
//! - ダミー子 (`mode = close`) は WM_CLOSE への応答として自ら exit **0** する。
//!
//! よって子の終了コードが、graceful 経路 (0) と強制終了経路 (1) を一意に識別する。
//! `graceful_child` は WM_CLOSE 以外の理由で終了しない (メッセージループが無限に
//! 続く) ため、終了コード 0 は即「WM_CLOSE が届いて処理された」ことの証明になる。
#![cfg(windows)]
#![allow(clippy::unwrap_used)]
#![allow(clippy::panic)]
#![allow(clippy::expect_used)]

use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anaden_device::{TerminateStages, terminate_child_staged};

/// std Command の creation_flags 用。コンソール信号の親への伝播を切る
/// (win32_launch の spawn_launcher と同じ分離)。
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

/// テストごとのダミー子の固有構成。コピー先 exe 名はプロセス名照合の対象なので
/// 並列実行や残存プロセスと衝突しない固有名にする。
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

/// `mode` / `close_delay_ms` を付けたダミー子を起動し、ウィンドウ作成完了
/// (ready ファイル出現) まで待つ。WM_CLOSE を送る前にウィンドウが存在しないと
/// graceful 経路の検証にならないため、準備完了を必ず待つ。
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

/// (a) WM_CLOSE に素直に退出する子: stage 1 で終わる。TerminateProcess 未到達の証明は
/// 終了コード 0 (強制終了なら 1)。
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

/// (b) WM_CLOSE を無視する子 (= フリーズゲーム代役): close_wait の猶予の後にのみ
/// stage 2 へ達する。終了コード 1 = TerminateProcess 到達、かつ猶予より早く
/// 強制終了していないことを経過時間で保証する。
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

/// (c) WM_CLOSE 受領後に退出まで時間がかかる子: stage 1 のポーリング待ちが途中の
/// 退出を取りこぼさず、猶予内なら強制終了に昇級しない。
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

/// 子が存在しなければ何もせず Ok (冪等)。kill 不要の再起動経路の前提。
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
