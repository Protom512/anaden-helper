//! Pipeline 実行ランナーGUI（Issue #83 シャード1 スケルトン）。
//!
//! MAA/MDA 型 GUI の最小要素のみ:
//! - ウィンドウ表示(eframe/egui)
//! - `anaden` CLI 子プロセスの起動/停止ボタン 1 組(childproc::ChildProcess)
//!
//! 状態遷移とプロセス操作は egui に依存しないメソッドに切り出し、
//! ヘッドレスでユニットテスト可能にしている。ログ/戦略選択はシャード3/4。

use eframe::egui;

use crate::childproc::{ChildProcess, SpawnSpec};

/// pipeline 子プロセスの起動指定を組み立てる。
///
/// `anaden` CLI のバイナリ名は環境 PATH 解決に任せる(ビルド成果物を
/// 同一レポジトリの target 配下に置く運用を想定。シャード2で IPC 整備時に
/// 明示パス解決へ切り替える)。
pub fn build_spawn_spec(program: &str, args: &[String]) -> SpawnSpec {
    SpawnSpec::new(program, args.to_vec())
}

/// pipeline 実行GUI の状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunnerStatus {
    /// 停止中。
    Stopped,
    /// 実行中。
    Running,
}

/// pipeline ランナーアプリ。
pub struct PipelineRunnerApp {
    /// 子プロセス管理。
    child: ChildProcess,
    /// 起動するコマンド。
    program: String,
    /// 直近のエラー表示(UI 表示用)。
    last_error: Option<String>,
}

impl PipelineRunnerApp {
    /// プログラム名を指定して生成する。
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            child: ChildProcess::new(),
            program: program.into(),
            last_error: None,
        }
    }

    /// 現在の状態。
    pub fn status(&mut self) -> RunnerStatus {
        if self.child.is_running() {
            RunnerStatus::Running
        } else {
            RunnerStatus::Stopped
        }
    }

    /// 開始ボタンのハンドラ。二重起動は childproc 層で防止され、
    /// エラーは UI 表示用に保持される。
    pub fn start_pipeline(&mut self, args: &[String]) {
        self.last_error = None;
        let spec = build_spawn_spec(&self.program, args);
        if let Err(e) = self.child.start(&spec) {
            self.last_error = Some(e.to_string());
        }
    }

    /// 停止ボタンのハンドラ。
    pub fn stop_pipeline(&mut self) {
        self.last_error = None;
        if let Err(e) = self.child.stop() {
            self.last_error = Some(e.to_string());
        }
    }

    /// 直近のエラーメッセージ(無ければ None)。
    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }
}

impl eframe::App for PipelineRunnerApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show_inside(ui, |ui| {
            ui.heading("anaden pipeline runner");

            let running = self.status() == RunnerStatus::Running;
            ui.add_enabled_ui(!running, |ui| {
                if ui.button("開始").clicked() {
                    self.start_pipeline(&["--version".to_string()]);
                }
            });
            ui.add_enabled_ui(running, |ui| {
                if ui.button("停止").clicked() {
                    self.stop_pipeline();
                }
            });

            ui.label(if running {
                "状態: 実行中"
            } else {
                "状態: 停止"
            });

            if let Some(err) = self.last_error() {
                ui.colored_label(egui::Color32::RED, format!("エラー: {err}"));
            }
        });
    }
}

impl Drop for PipelineRunnerApp {
    fn drop(&mut self) {
        // ChildProcess の kill_on_drop に加え、明示的に停止を試みる。
        self.stop_pipeline();
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    /// 実行可能なダミープログラムの名前(ping は Windows/Linux 両方にある)。
    fn dummy_program() -> &'static str {
        "ping"
    }

    fn long_args() -> Vec<String> {
        if cfg!(windows) {
            vec!["-n".to_string(), "30".to_string(), "127.0.0.1".to_string()]
        } else {
            vec!["-c".to_string(), "30".to_string(), "127.0.0.1".to_string()]
        }
    }

    #[test]
    fn test_build_spawn_spec_copies_program_and_args() {
        let args = vec!["run".to_string()];
        let spec = build_spawn_spec("anaden", &args);
        assert_eq!(spec.program, "anaden");
        assert_eq!(spec.args, args);
    }

    #[test]
    fn test_initial_status_is_stopped() {
        let mut app = PipelineRunnerApp::new(dummy_program());
        assert_eq!(app.status(), RunnerStatus::Stopped);
        assert!(app.last_error().is_none());
    }

    #[test]
    fn test_start_moves_to_running_and_stop_returns_to_stopped() {
        let mut app = PipelineRunnerApp::new(dummy_program());
        app.start_pipeline(&long_args());
        assert_eq!(app.status(), RunnerStatus::Running);
        app.stop_pipeline();
        assert_eq!(app.status(), RunnerStatus::Stopped);
    }

    #[test]
    fn test_spawn_failure_sets_last_error_and_stays_stopped() {
        let mut app = PipelineRunnerApp::new("definitely-not-a-real-exe-xyz");
        app.start_pipeline(&[]);
        assert_eq!(app.status(), RunnerStatus::Stopped);
        let err = app.last_error().expect("error should be recorded");
        assert!(err.contains("起動に失敗"), "unexpected error: {err}");
    }

    #[test]
    fn test_stop_when_stopped_sets_error() {
        let mut app = PipelineRunnerApp::new(dummy_program());
        app.stop_pipeline();
        let err = app.last_error().expect("error should be recorded");
        assert!(
            err.contains("実行中ではありません"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_double_start_records_already_running_error() {
        let mut app = PipelineRunnerApp::new(dummy_program());
        app.start_pipeline(&long_args());
        let running = app.status() == RunnerStatus::Running;
        if running {
            app.start_pipeline(&[]);
            let err = app.last_error().expect("error should be recorded");
            assert!(err.contains("既に実行中"), "unexpected error: {err}");
            app.stop_pipeline();
        }
        // ping が環境に無い場合は start が失敗するためスキップ相当。
    }
}
