//! 子プロセス stdout/stderr 読み取りスレッドの起動と共有子プロセスハンドル
//! (Issue #172: log_view.rs 分割で旧モジュールから移動)。
//!
//! IO（子プロセス stdout/stderr パイプ読み取り）は本モジュールの
//! `spawn_stdout_reader` / `spawn_output_readers` のみで、`std::sync::mpsc`
//! で `LogEvent` を UI 側へ非ブロッキング配送する（行解析・バッファリングは
//! [`crate::log_buffer`]、状態トラッカは [`crate::log_status`]）。

use std::io::{BufRead, BufReader};
use std::process::{Child, Stdio};
use std::sync::mpsc::SyncSender;
use std::sync::mpsc::TrySendError;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use crate::log_buffer::LogEvent;

/// 子プロセスの stdout を行単位で読み取り `tx` へ送るスレッドを起動する。
///
/// 読み取りスレッドは行を `SyncSender::try_send` で送る（bounded）。UI 側が
/// 受信を止めてもスレッドがブロックしないよう、`Full/Disconnected` 時は
/// 該当行を破棄して読み取りを継続する（ログは best-effort）。
///
/// 戻り値:
/// - `Ok((Child, JoinHandle))`: 起動成功。`Child` の stdout は本スレッドが
///   消費するため UI 側は wait のみ行うこと。JoinHandle は EOF 後に子の
///   exit code を待ち `LogEvent::Exit` を送って完了する。
/// - `Err(spawn 失敗)`: 子プロセス未起動。
///
/// # Errors
/// `std::process::Command::spawn` の失敗（実行ファイル不在等）をそのまま返す。
#[allow(dead_code)]
pub fn spawn_stdout_reader(
    mut cmd: std::process::Command,
    tx: SyncSender<LogEvent>,
) -> std::io::Result<(SharedChild, JoinHandle<()>)> {
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::null()); // shard-4 では stdout のみ（tracing は stdout 出力）
    let mut child = cmd.spawn()?;
    let stdout = child.stdout.take();
    let child = SharedChild::new(child);
    let poll_child = child.clone();
    let reader = std::thread::spawn(move || {
        if let Some(out) = stdout {
            for line in BufReader::new(out).lines() {
                let Ok(line) = line else { break };
                if matches!(
                    tx.try_send(LogEvent::Line(line)),
                    Err(TrySendError::Full(_) | TrySendError::Disconnected(_))
                ) {
                    // UI が受信しなくても読み取りは続行（EOF 検出のため）。
                }
            }
        }
        let code = poll_child.wait_for_exit();
        let _ = tx.try_send(LogEvent::Exit(code));
    });
    Ok((child, reader))
}

/// 複数スレッド（UI 側 kill/wait と reader スレッドの終了検出）で共有する子プロセス。
///
/// `Child::wait` は blocking かつ排他のため共有できない。本型では
/// `SharedChild::wait_for_exit` が `try_wait` をポーリングし、UI 側の
/// kill/wait と競合しない（lock は各呼び出し毎に短時間のみ保持）。
#[derive(Clone)]
pub struct SharedChild(Arc<Mutex<Child>>);

impl SharedChild {
    /// ラップする。
    pub fn new(child: Child) -> Self {
        Self(Arc::new(Mutex::new(child)))
    }

    /// 非ブロッキングの生存確認。終了済みなら false。
    pub fn is_running(&self) -> bool {
        let Ok(mut child) = self.0.lock() else {
            return false;
        };
        matches!(child.try_wait(), Ok(None))
    }

    /// kill + wait（停止ボタン用）。ロック中毒時は何もしない。
    pub fn kill_and_wait(&self) {
        let Ok(mut child) = self.0.lock() else {
            return;
        };
        let _ = child.kill();
        let _ = child.wait();
    }

    /// 子の終了を 50ms 間隔の try_wait ポーリングで待ち、exit code を返す。
    ///
    /// UI 側が kill_and_wait で先に終了させた場合も即座に検出できる。
    fn wait_for_exit(&self) -> Option<i32> {
        loop {
            let status = {
                let Ok(mut child) = self.0.lock() else {
                    return None;
                };
                match child.try_wait() {
                    Ok(Some(status)) => Some(status),
                    _ => None,
                }
            };
            if let Some(status) = status {
                return status.code();
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
}

/// パイプ 1 本（`R`）を非ブロッキング送信で drain し、`prefix` 付きの行を
/// `tx` へ送る。Full/Disconnected 時は行を破棄して読み取りを継続
/// （EOF 検出のため。ログは best-effort）。
fn drain_pipe<R: std::io::Read>(pipe: Option<R>, tx: &SyncSender<LogEvent>, prefix: &str) {
    let Some(pipe) = pipe else { return };
    for line in BufReader::new(pipe).lines() {
        let Ok(line) = line else { break };
        let line = if prefix.is_empty() {
            line
        } else {
            format!("{prefix}{line}")
        };
        if matches!(
            tx.try_send(LogEvent::Line(line)),
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_))
        ) {
            // UI が受信しなくても読み取りは続行（EOF 検出のため）。
        }
    }
}

/// stdout と stderr の**両方**を別々のスレッドで読み取る子プロセスを起動する。
///
/// Issue #85 (Issue #83 shard 2): 読み手不在によるパイプ容量超過ブロックを
/// 解消するため、ChildProcess から利用される。stdout と stderr は独立した
/// 2 スレッドで並行 drain する（逐次読み取りは片パイプだけ書く子で
/// デッドロックするため）。子の終了待機と `LogEvent::Exit` は stdout 側の
/// reader が 1 回だけ行う。
///
/// # Errors
/// `std::process::Command::spawn` の失敗（実行ファイル不在等）をそのまま返す。
pub fn spawn_output_readers(
    mut cmd: std::process::Command,
    tx: SyncSender<LogEvent>,
) -> std::io::Result<(SharedChild, JoinHandle<()>, JoinHandle<()>)> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn()?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let child = SharedChild::new(child);
    let poll_child = child.clone();
    let stdout_reader = {
        let tx = tx.clone();
        std::thread::spawn(move || {
            drain_pipe(stdout, &tx, "");
            let code = poll_child.wait_for_exit();
            let _ = tx.try_send(LogEvent::Exit(code));
        })
    };
    let stderr_reader = {
        let tx = tx.clone();
        std::thread::spawn(move || drain_pipe(stderr, &tx, "[stderr] "))
    };
    Ok((child, stdout_reader, stderr_reader))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    // ---- spawn_output_readers (stdout/stderr 並行 drain) ----

    #[test]
    fn output_readers_stream_both_stdout_and_stderr() {
        // stdout と stderr の両方へ書く子。逐次読み取りなら stderr が
        // パイプ容量で詰まる可能性があるが、並行 drain で両方届く。
        let cmd = if cfg!(windows) {
            let mut c = std::process::Command::new("cmd");
            c.args(["/C", "echo out-line & echo err-line 1>&2"]);
            c
        } else {
            let mut c = std::process::Command::new("sh");
            c.args(["-c", "echo out-line; echo err-line >&2"]);
            c
        };
        let (tx, rx) = std::sync::mpsc::sync_channel::<LogEvent>(256);
        let (_child, out_h, err_h) = spawn_output_readers(cmd, tx).expect("spawn failed");
        out_h.join().expect("stdout reader panicked");
        err_h.join().expect("stderr reader panicked");
        let mut events: Vec<LogEvent> = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            events.push(ev);
        }
        let lines: Vec<String> = events
            .iter()
            .filter_map(|e| match e {
                LogEvent::Line(l) => Some(l.trim().to_string()),
                _ => None,
            })
            .collect();
        assert!(
            lines.iter().any(|l| l.contains("out-line")),
            "lines={lines:?}"
        );
        assert!(
            lines.iter().any(|l| l.contains("err-line")),
            "lines={lines:?}"
        );
        assert!(
            matches!(events.last(), Some(LogEvent::Exit(Some(0)))),
            "events={events:?}"
        );
    }

    // ---- spawn_stdout_reader (echo プロセスで統合確認) ----

    #[test]
    fn stdout_reader_streams_lines_and_exit() {
        // Windows は cmd /c、それ以外は sh。どちらも2行出力して終了する。
        let cmd = if cfg!(windows) {
            let mut c = std::process::Command::new("cmd");
            c.args(["/C", "echo one & echo two"]);
            c
        } else {
            let mut c = std::process::Command::new("sh");
            c.args(["-c", "echo one; echo two"]);
            c
        };
        let (tx, rx) = std::sync::mpsc::sync_channel::<LogEvent>(256);
        let (_child, handle) = spawn_stdout_reader(cmd, tx).expect("spawn failed");
        handle.join().expect("reader thread panicked");
        let mut events: Vec<LogEvent> = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            events.push(ev);
        }
        let lines: Vec<&str> = events
            .iter()
            .filter_map(|e| match e {
                LogEvent::Line(l) => Some(l.as_str()),
                _ => None,
            })
            .map(str::trim)
            .collect();
        assert!(lines.contains(&"one"), "lines={lines:?}");
        assert!(lines.contains(&"two"), "lines={lines:?}");
        assert!(
            matches!(events.last(), Some(LogEvent::Exit(Some(0)))),
            "events={events:?}"
        );
    }
}
