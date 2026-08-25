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
use std::process::{Child, Command, Stdio};

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
    /// プロセス停止に失敗。
    #[error("停止に失敗: {0}")]
    Kill(#[source] io::Error),
    /// 停止後の終了待機に失敗。
    #[error("終了待機に失敗: {0}")]
    Wait(#[source] io::Error),
}

/// 子プロセス 1 本のライフサイクル管理。
///
/// GUI 停止ボタン → [`ChildProcess::stop`]、GUI 終了 → Drop で kill。
pub struct ChildProcess {
    child: Option<Child>,
}

impl Default for ChildProcess {
    fn default() -> Self {
        Self::new()
    }
}

impl ChildProcess {
    /// 空の状態で生成する。
    pub fn new() -> Self {
        Self { child: None }
    }

    /// 実行中かどうか。終了済みなら内部状態を掃除して false を返す。
    pub fn is_running(&mut self) -> bool {
        match &mut self.child {
            None => false,
            Some(child) => matches!(child.try_wait(), Ok(None)),
        }
    }

    /// 子プロセスを起動する。実行中なら [`ChildProcessError::AlreadyRunning`]。
    pub fn start(&mut self, spec: &SpawnSpec) -> Result<(), ChildProcessError> {
        if self.is_running() {
            return Err(ChildProcessError::AlreadyRunning);
        }
        let child = Command::new(&spec.program)
            .args(&spec.args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(ChildProcessError::Spawn)?;
        self.child = Some(child);
        Ok(())
    }

    /// 子プロセスを停止( kill )し終了を待つ。実行中でなければ
    /// [`ChildProcessError::NotRunning`]。
    pub fn stop(&mut self) -> Result<(), ChildProcessError> {
        let Some(child) = self.child.as_mut() else {
            return Err(ChildProcessError::NotRunning);
        };
        if child.try_wait().map_err(ChildProcessError::Wait)?.is_some() {
            // 既に終了済み。
            self.child = None;
            return Ok(());
        }
        child.kill().map_err(ChildProcessError::Kill)?;
        child.wait().map_err(ChildProcessError::Wait)?;
        self.child = None;
        Ok(())
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
        cp.start(&long_running_spec()).unwrap();
        assert!(cp.is_running());
        cp.stop().unwrap();
        assert!(!cp.is_running());
    }

    #[test]
    fn test_double_start_rejected() {
        let mut cp = ChildProcess::new();
        cp.start(&long_running_spec()).unwrap();
        let err = cp.start(&short_spec()).unwrap_err();
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
        cp.start(&short_spec()).unwrap();
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
        let err = cp.start(&spec).unwrap_err();
        assert!(matches!(err, ChildProcessError::Spawn(_)));
    }

    #[test]
    fn test_restart_after_stop_succeeds() {
        let mut cp = ChildProcess::new();
        cp.start(&short_spec()).unwrap();
        cp.stop().unwrap();
        // 停止後なら再起動できる。
        cp.start(&short_spec()).unwrap();
        cp.stop().unwrap();
    }
}
