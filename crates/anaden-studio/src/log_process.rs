use std::io::{BufRead, BufReader};
use std::process::{Child, Stdio};
use std::sync::mpsc::SyncSender;
use std::sync::mpsc::TrySendError;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use crate::log_buffer::LogEvent;

#[allow(dead_code)]
pub fn spawn_stdout_reader(
    mut cmd: std::process::Command,
    tx: SyncSender<LogEvent>,
) -> std::io::Result<(SharedChild, JoinHandle<()>)> {
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::null());
    let mut child = cmd.spawn()?;
    let stdout = child.stdout.take();
    let shared = SharedChild::new(child);
    let reader = spawn_exit_reporting_reader(stdout, shared.clone(), tx);
    Ok((shared, reader))
}

fn spawn_exit_reporting_reader(
    stdout: Option<std::process::ChildStdout>,
    child: SharedChild,
    tx: SyncSender<LogEvent>,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        drain_pipe(stdout, &tx, "");
        let code = child.wait_for_exit();
        let _ = tx.try_send(LogEvent::Exit(code));
    })
}

#[derive(Clone)]
pub struct SharedChild(Arc<Mutex<Child>>);

impl SharedChild {
    pub fn new(child: Child) -> Self {
        Self(Arc::new(Mutex::new(child)))
    }

    pub fn is_running(&self) -> bool {
        let Ok(mut child) = self.0.lock() else {
            return false;
        };
        matches!(child.try_wait(), Ok(None))
    }

    pub fn kill_and_wait(&self) {
        let Ok(mut child) = self.0.lock() else {
            return;
        };
        let _ = child.kill();
        let _ = child.wait();
    }

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
        ) {}
    }
}

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
    let shared = SharedChild::new(child);
    let stdout_reader = spawn_exit_reporting_reader(stdout, shared.clone(), tx.clone());
    let stderr_reader = {
        let tx = tx.clone();
        std::thread::spawn(move || drain_pipe(stderr, &tx, "[stderr] "))
    };
    Ok((shared, stdout_reader, stderr_reader))
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
