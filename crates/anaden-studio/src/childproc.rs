use std::io;
use std::process::{Command, Stdio};
use std::sync::mpsc::SyncSender;
use std::thread::JoinHandle;

use crate::log_view::{self, LogEvent, SharedChild};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnSpec {
    pub program: String,
    pub args: Vec<String>,
}

impl SpawnSpec {
    pub fn new(program: impl Into<String>, args: impl IntoIterator<Item = String>) -> Self {
        Self {
            program: program.into(),
            args: args.into_iter().collect(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ChildProcessError {
    #[error("子プロセスは既に実行中です")]
    AlreadyRunning,
    #[error("子プロセスは実行中ではありません")]
    NotRunning,
    #[error("起動に失敗: {0}")]
    Spawn(#[source] io::Error),
    #[error("停止に失敗: {0}")]
    Kill(#[source] io::Error),
    #[error("終了待機に失敗: {0}")]
    Wait(#[source] io::Error),
}

pub struct ChildProcess {
    child: Option<SharedChild>,
    stdout_reader: Option<JoinHandle<()>>,
    stderr_reader: Option<JoinHandle<()>>,
}

impl Default for ChildProcess {
    fn default() -> Self {
        Self::new()
    }
}

impl ChildProcess {
    pub fn new() -> Self {
        Self {
            child: None,
            stdout_reader: None,
            stderr_reader: None,
        }
    }

    pub fn is_running(&mut self) -> bool {
        match &self.child {
            None => false,
            Some(child) => child.is_running(),
        }
    }

    pub fn start(
        &mut self,
        spec: &SpawnSpec,
        tx: SyncSender<LogEvent>,
    ) -> Result<(), ChildProcessError> {
        if self.is_running() {
            return Err(ChildProcessError::AlreadyRunning);
        }
        self.reap_readers();
        let mut command = Command::new(&spec.program);
        command
            .args(&spec.args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let (child, stdout_reader, stderr_reader) =
            log_view::spawn_output_readers(command, tx).map_err(ChildProcessError::Spawn)?;
        self.child = Some(child);
        self.stdout_reader = Some(stdout_reader);
        self.stderr_reader = Some(stderr_reader);
        Ok(())
    }

    pub fn stop(&mut self) -> Result<(), ChildProcessError> {
        let Some(child) = self.child.as_ref() else {
            return Err(ChildProcessError::NotRunning);
        };
        if !child.is_running() {
            self.child = None;
            self.reap_readers();
            return Ok(());
        }
        child.kill_and_wait();
        self.child = None;
        self.reap_readers();
        Ok(())
    }

    fn reap_readers(&mut self) {
        if let Some(h) = self.stdout_reader.take() {
            let _ = h.join();
        }
        if let Some(h) = self.stderr_reader.take() {
            let _ = h.join();
        }
    }
}

impl Drop for ChildProcess {
    fn drop(&mut self) {
        if self.is_running() {
            let _ = self.stop();
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use std::sync::mpsc::sync_channel;

    /// テスト用の bounded チャネル（容量大: テストは受信しないケースがあるため
    /// Full 破棄でもreaderは継続する）。
    fn channel() -> (
        std::sync::mpsc::SyncSender<crate::log_view::LogEvent>,
        std::sync::mpsc::Receiver<crate::log_view::LogEvent>,
    ) {
        sync_channel(1024)
    }

    /// 長時間(約30秒)生きる子プロセスの SpawnSpec。
    /// ping は Windows でも Linux でも存在し、`-n`/`-c` の差のみ。
    fn long_running_spec() -> SpawnSpec {
        if cfg!(windows) {
            SpawnSpec::new(
                "ping",
                ["-n".to_string(), "30".to_string(), "127.0.0.1".to_string()],
            )
        } else {
            SpawnSpec::new(
                "ping",
                ["-c".to_string(), "30".to_string(), "127.0.0.1".to_string()],
            )
        }
    }

    /// 即終了する子プロセスの SpawnSpec。
    fn short_spec() -> SpawnSpec {
        if cfg!(windows) {
            SpawnSpec::new(
                "cmd",
                ["/C".to_string(), "exit".to_string(), "0".to_string()],
            )
        } else {
            SpawnSpec::new("true", Vec::new())
        }
    }

    #[test]
    fn test_start_reports_running() {
        let mut cp = ChildProcess::new();
        assert!(!cp.is_running());
        cp.start(&long_running_spec(), channel().0).unwrap();
        assert!(cp.is_running());
        cp.stop().unwrap();
        assert!(!cp.is_running());
    }

    #[test]
    fn test_double_start_rejected() {
        let mut cp = ChildProcess::new();
        cp.start(&long_running_spec(), channel().0).unwrap();
        let err = cp.start(&short_spec(), channel().0).unwrap_err();
        assert!(matches!(err, ChildProcessError::AlreadyRunning));
        cp.stop().unwrap();
    }

    #[test]
    fn test_stop_when_not_running_is_error() {
        let mut cp = ChildProcess::new();
        let err = cp.stop().unwrap_err();
        assert!(matches!(err, ChildProcessError::NotRunning));
    }

    #[test]
    fn test_stop_after_natural_exit_is_ok() {
        let mut cp = ChildProcess::new();
        cp.start(&short_spec(), channel().0).unwrap();
        // 短命プロセスの自然終了を待つ。
        for _ in 0..50 {
            if !cp.is_running() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        assert!(!cp.is_running());
        // 自然終了後の stop は Ok。
        cp.stop().unwrap();
    }

    #[test]
    fn test_spawn_failure_reports_error() {
        let mut cp = ChildProcess::new();
        let spec = SpawnSpec::new("definitely-not-a-real-exe-xyz", Vec::new());
        let err = cp.start(&spec, channel().0).unwrap_err();
        assert!(matches!(err, ChildProcessError::Spawn(_)));
    }

    #[test]
    fn test_restart_after_stop_succeeds() {
        let mut cp = ChildProcess::new();
        cp.start(&short_spec(), channel().0).unwrap();
        cp.stop().unwrap();
        // 停止後なら再起動できる。
        cp.start(&short_spec(), channel().0).unwrap();
        cp.stop().unwrap();
    }

    #[test]
    fn test_stderr_is_drained_with_prefix() {
        // stderr に 2 行出力して終了する子。reader が drain しないと
        // 小容量でも保証がないため、接続を検証する。
        let spec = if cfg!(windows) {
            SpawnSpec::new(
                "cmd",
                [
                    "/C".to_string(),
                    "echo err1 1>&2 & echo err2 1>&2".to_string(),
                ],
            )
        } else {
            SpawnSpec::new(
                "sh",
                ["-c", "echo err1 >&2; echo err2 >&2"]
                    .iter()
                    .map(|s| s.to_string())
                    .collect::<Vec<String>>(),
            )
        };
        let (tx, rx) = channel();
        let mut cp = ChildProcess::new();
        cp.start(&spec, tx.clone()).unwrap();
        // 子の自然終了 + reader 回収を待つ。
        for _ in 0..100 {
            if !cp.is_running() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        cp.stop().unwrap();
        let mut stderr_lines = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            if let crate::log_view::LogEvent::Line(l) = ev
                && l.starts_with("[stderr] ")
            {
                stderr_lines.push(l);
            }
        }
        assert!(
            stderr_lines.iter().any(|l| l.contains("err1")),
            "{stderr_lines:?}"
        );
        assert!(
            stderr_lines.iter().any(|l| l.contains("err2")),
            "{stderr_lines:?}"
        );
    }

    /// UC-1: 大量出力でも子プロセスがブロックしない。
    ///
    /// 数 MB を stdout/stderr 両方に出力する子を、受信しない bounded チャネル
    /// （容量 1）で起動する。読み手不在なら OS パイプ容量（Windows 数十 KB）
    /// を超えた時点で子がブロックし、タイムアウト内に終了しない。
    /// reader が Full 時に行を破棄して読み続けるため、子は即座に書き切れる。
    ///
    /// CI の負荷変動を考慮してタイムアウトは 180 秒。それでも失敗する場合は
    /// `#[ignore]` 化を検討（Windows CI タイムアウトリスクは承認条件より
    /// 事前ポリシー: 通常 nextest で実行し、flaky 判定時に ignore へ昇格）。
    ///
    /// 2026-08-26: Windows 実機で 60 秒超過の flaky を確認したため
    /// 事前ポリシーに従い ignore へ昇格。検証時は
    /// `cargo nextest run -p anaden-studio -E 'test(high_volume)' --run-ignored ignored`。
    #[test]
    #[ignore = "Windows で cmd の for ループ出力が遅く 60s タイムアウトを超える flaky (Issue #85 UC-1 事前ポリシー)"]
    fn test_high_volume_output_does_not_block_child() {
        // 約 4MB: 各行 64B × 65536 行 × 2 パイプ。
        let spec = if cfg!(windows) {
            SpawnSpec::new(
                "cmd",
                ["/C".to_string(),
                 "for /L %i in (1,1,65536) do @echo aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa & echo bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb 1>&2".to_string()],
            )
        } else {
            SpawnSpec::new(
                "sh",
                ["-c".to_string(),
                 "i=0; while [ $i -lt 65536 ]; do echo aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa; echo bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb >&2; i=$((i+1)); done".to_string()].to_vec(),
            )
        };
        // 容量 1 の bounded チャネル: テストは受信しない → Full 連発。
        let (tx, rx) = sync_channel::<crate::log_view::LogEvent>(1);
        std::mem::forget(rx); // Drop でDisconnectedになってもreaderは継続するが、Full経路を主に検証
        let mut cp = ChildProcess::new();
        cp.start(&spec, tx.clone()).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(180);
        loop {
            if !cp.is_running() {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "子プロセスが180秒以内に終了しない = パイプ読み手不在でブロック"
            );
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        cp.stop().unwrap();
    }
}
