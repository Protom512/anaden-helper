//! `anaden` CLI 子プロセスの起動/停止管理。
//!
//! Issue #83 シャード1 スケルトン: GUI から pipeline を 1 本だけ起動し、
//! 停止ボタンで終了させるための最小ラッパ。std::process::Command ベースで、
//! 二重起動防止・GUI 落下時の孤立防止(kill_on_drop)を担う。
//!
//! 注意: Drop による kill は「GUI が正常終了した場合」のみ機能する。
//! GUI がクラッシュした場合の孤立防止(Windows Job Object)はシャード2で
//! IPC アーキテクチャと併せて決定する(設計比較は GUI-Stack-Selection.md 参照)。

use std::io;
use std::process::{Command, Stdio};
use std::sync::mpsc::SyncSender;
use std::thread::JoinHandle;

use crate::log_view::{self, LogEvent, SharedChild};

/// 起動する子プロセスの指定。
#[derive(Debug, Clone)]
pub struct SpawnSpec {
    /// 実行ファイル名またはパス。
    pub program: String,
    /// 引数。
    pub args: Vec<String>,
}

impl SpawnSpec {
    /// program と args から生成する。
    pub fn new(program: impl Into<String>, args: impl IntoIterator<Item = String>) -> Self {
        Self {
            program: program.into(),
            args: args.into_iter().collect(),
        }
    }
}

/// 子プロセス管理エラー。
#[derive(Debug, thiserror::Error)]
pub enum ChildProcessError {
    /// 実行中に再起動を試みた(二重起動防止)。
    #[error("子プロセスは既に実行中です")]
    AlreadyRunning,
    /// 停止対象が実行中でない。
    #[error("子プロセスは実行中ではありません")]
    NotRunning,
    /// プロセス起動に失敗。
    #[error("起動に失敗: {0}")]
    Spawn(#[source] io::Error),
    /// プロセス停止に失敗（SharedChild 化に伴い現在未使用だが、stop の
    /// 失敗報告契約として将来の再利用に備えて保持する）。
    #[error("停止に失敗: {0}")]
    Kill(#[source] io::Error),
    /// 停止後の終了待機に失敗（同上）。
    #[error("終了待機に失敗: {0}")]
    Wait(#[source] io::Error),
}

/// 子プロセス 1 本のライフサイクル管理。
///
/// GUI 停止ボタン → [`ChildProcess::stop`]、GUI 終了 → Drop で kill。
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
    /// 空の状態で生成する。
    pub fn new() -> Self {
        Self {
            child: None,
            stdout_reader: None,
            stderr_reader: None,
        }
    }

    /// 実行中かどうか。終了済みなら false。
    pub fn is_running(&mut self) -> bool {
        match &self.child {
            None => false,
            Some(child) => child.is_running(),
        }
    }

    /// 子プロセスを起動する。実行中なら [`ChildProcessError::AlreadyRunning`]。
    ///
    /// stdout と stderr は別々の読み取りスレッド（log_view の reader）に
    /// 接続され、行は `tx` へ送られる（stderr 行は `[stderr] ` 接頭辞付き）。
    /// 読み手不在によるパイプ詰まり（子が大量出力時にブロック）は発生しない。
    /// Exit イベントは stdout 側の reader が 1 回だけ送る。
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

    /// 子プロセスを停止( kill )し終了を待つ。実行中でなければ
    /// [`ChildProcessError::NotRunning`]。
    ///
    /// kill 後は stdout/stderr パイプが EOF になり、各読み取りスレッドが
    /// 完了するのを回収（join）する。
    pub fn stop(&mut self) -> Result<(), ChildProcessError> {
        let Some(child) = self.child.as_ref() else {
            return Err(ChildProcessError::NotRunning);
        };
        if !child.is_running() {
            // 既に終了済み。
            self.child = None;
            self.reap_readers();
            return Ok(());
        }
        child.kill_and_wait();
        self.child = None;
        self.reap_readers();
        Ok(())
    }

    /// 読み取りスレッドの JoinHandle を回収する（パイプ EOF 後に完了済み）。
    /// join に失敗（reader パニック）や完了前でもエラー化しない（ログは
    /// best-effort）。未完了の場合は Drop 時にスレッドを放流する。
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
