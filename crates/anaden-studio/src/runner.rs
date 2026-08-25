//! Pipeline 実行ランナーGUI（Issue #83/#85 シャード2・3前半）。
//!
//! MAA/MDA 型 GUI の要素:
//! - ウィンドウ表示(eframe/egui)
//! - `anaden` CLI 子プロセスの起動/停止ボタン 1 組(childproc::ChildProcess)
//! - stdout/stderr のログ表示（log_view::SharedLogBuffer + egui スクロール
//!   ビューア。LogLevel 色分け・自動スクロール・クリアボタン）= シャード3前半
//! - `anaden` バイナリの明示パス解決（ANADEN_BIN → target/{debug,release} → PATH）
//!
//! 状態遷移・パス解決・ログ drain は egui に依存しないメソッドに切り出し、
//! ヘッドレスでユニットテスト可能にしている。

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, SyncSender};

use eframe::egui;

use crate::childproc::{ChildProcess, SpawnSpec};
use crate::log_view::{
    DEFAULT_MAX_LINES, LogBuffer, LogEntry, LogEvent, LogLevel, SharedLogBuffer,
};

/// `anaden` バイナリを解決する純関数（Issue #85 タスク2）。
///
/// 候補順:
/// 1. 環境変数 `ANADEN_BIN`（ファイルが存在すれば使用、無ければエラー）
/// 2. マニフェストディレクトリ基準 `target/<profile>/anaden[.exe]`
///    （debug → release の順）
/// 3. PATH 環境変数の各ディレクトリ
///
/// # Errors
/// いずれの候補でも実行可能ファイルが見つからない場合、または
/// `ANADEN_BIN` が存在しないパスを指している場合にエラーメッセージを返す。
pub fn pick_anaden_bin(
    env_val: Option<&str>,
    manifest_dir: Option<&Path>,
    path_var: Option<&str>,
) -> Result<PathBuf, String> {
    let exe = if cfg!(windows) {
        "anaden.exe"
    } else {
        "anaden"
    };
    if let Some(v) = env_val {
        let p = PathBuf::from(v);
        if p.is_file() {
            return Ok(p);
        }
        return Err(format!("ANADEN_BIN が指すファイルが存在しません: {v}"));
    }
    if let Some(dir) = manifest_dir {
        for profile in ["debug", "release"] {
            let p = dir.join("target").join(profile).join(exe);
            if p.is_file() {
                return Ok(p);
            }
        }
    }
    if let Some(path) = path_var {
        for dir in std::env::split_paths(path) {
            let p = dir.join(exe);
            if p.is_file() {
                return Ok(p);
            }
        }
    }
    Err(format!(
        "anaden バイナリ解決失敗: ANADEN_BIN / target/{{debug,release}}/{exe} / PATH のいずれにも見つかりません"
    ))
}

/// 実行時の `anaden` 解決（main.rs 用）。失敗時は PATH 起動を試みる
/// プログラム名 "anaden" へフォールバックし、解決エラーを UI 表示用に返す。
fn resolve_anaden_program() -> (String, Option<String>) {
    let env_val = std::env::var("ANADEN_BIN").ok();
    let manifest = Some(Path::new(env!("CARGO_MANIFEST_DIR")));
    let path_var = std::env::var("PATH").ok();
    match pick_anaden_bin(env_val.as_deref(), manifest, path_var.as_deref()) {
        Ok(p) => (p.to_string_lossy().into_owned(), None),
        Err(e) => ("anaden".to_string(), Some(e)),
    }
}

/// pipeline 子プロセスの起動指定を組み立てる。
pub fn build_spawn_spec(program: &str, args: &[String]) -> SpawnSpec {
    SpawnSpec::new(program, args.to_vec())
}

/// LogLevel の表示色（スクロールログビューアの色分け・純関数）。
fn level_color(level: LogLevel) -> egui::Color32 {
    match level {
        LogLevel::Error => egui::Color32::from_rgb(240, 80, 80),
        LogLevel::Warn => egui::Color32::from_rgb(230, 180, 50),
        LogLevel::Info => egui::Color32::from_gray(200),
    }
}

/// pipeline 実行GUI の状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunnerStatus {
    /// 停止中。
    Stopped,
    /// 実行中。
    Running,
}

/// 読み取りスレッド → UI 間の bounded channel 容量。
const LOG_CHANNEL_CAPACITY: usize = 1024;

/// pipeline ランナーアプリ。
pub struct PipelineRunnerApp {
    /// 子プロセス管理。
    child: ChildProcess,
    /// 起動するコマンド。
    program: String,
    /// バイナリ解決失敗等、起動前に確定しているエラー。
    resolution_error: Option<String>,
    /// 直近のエラー表示(UI 表示用)。
    last_error: Option<String>,
    /// ログイベント送信口(stdout/stderr reader 接続・再起動時に再利用)。
    log_tx: SyncSender<LogEvent>,
    /// ログイベント受信口(UI drain 用)。drain 後も再利用するため保持。
    log_rx: Receiver<LogEvent>,
    /// ログバッファ（reader → channel → drain で反映）。
    log: SharedLogBuffer,
    /// UI 描画用ログスナップショット（毎フレーム drain で更新）。
    log_snapshot: Vec<LogEntry>,
    /// ログの自動スクロール。
    auto_scroll: bool,
    /// 戦略選択パネル(シャード3スコープ、runner に統合)。
    strategy_panel: crate::strategy_ui::StrategyPanel,
}

impl PipelineRunnerApp {
    /// プログラム名を指定して生成する（テスト・明示指定用）。
    #[allow(dead_code)]
    pub fn new(program: impl Into<String>) -> Self {
        Self::with_channel(program, None)
    }

    /// `anaden` バイナリを明示パス解決して生成する（main.rs 用）。
    /// 解決失敗時は PATH の "anaden" にフォールバックし、エラーを保持する。
    pub fn with_resolved_anaden() -> Self {
        let (program, err) = resolve_anaden_program();
        Self::with_channel(program, err)
    }

    /// テスト用: 解決失敗状態をシミュレートする（起動を試みると即エラー）。
    #[allow(dead_code)]
    fn unresolved(message: &str) -> Self {
        Self::with_channel(
            "anaden",
            Some(format!("anaden バイナリ解決失敗: {message}")),
        )
    }

    fn with_channel(program: impl Into<String>, resolution_error: Option<String>) -> Self {
        // ログチャネル: reader スレッドが try_send で送り、UI が drain する。
        // UI が受信しなくても reader は行を破棄して継続する（best-effort）。
        let (log_tx, log_rx) = std::sync::mpsc::sync_channel::<LogEvent>(LOG_CHANNEL_CAPACITY);
        Self {
            child: ChildProcess::new(),
            program: program.into(),
            resolution_error,
            last_error: None,
            log_tx,
            log_rx,
            log: SharedLogBuffer::new(DEFAULT_MAX_LINES),
            log_snapshot: Vec::new(),
            auto_scroll: true,
            strategy_panel: crate::strategy_ui::StrategyPanel::default(),
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

    /// 開始ボタンのハンドラ。二重起動は childproc 層で防止され、エラーは
    /// UI 表示用に保持されるとともにログへ ERROR 行として記録される
    /// （UC-3: 起動失敗時にエラー行がログに表示される）。
    pub fn start_pipeline(&mut self, args: &[String]) {
        self.last_error = None;
        if let Some(e) = self.resolution_error.clone() {
            self.record_error_line(&e);
            return;
        }
        let spec = build_spawn_spec(&self.program, args);
        if let Err(e) = self.child.start(&spec, self.log_tx.clone()) {
            self.record_error_line(&e.to_string());
        }
    }

    /// 停止ボタンのハンドラ。エラーは UI 表示用に保持するとともにログへ記録。
    pub fn stop_pipeline(&mut self) {
        self.last_error = None;
        if let Err(e) = self.child.stop() {
            self.record_error_line(&e.to_string());
        }
    }

    /// エラーを last_error とログ（ERROR レベル行）の両方へ記録する。
    fn record_error_line(&mut self, message: &str) {
        self.last_error = Some(message.to_string());
        self.push_log_line(&format!("ERROR {message}"));
    }

    /// ログバッファへ直接 1 行 push しスナップショットを更新する
    /// （レベルは LogLevel::from_line で推定される）。
    fn push_log_line(&mut self, line: &str) {
        let line = line.to_string();
        self.log.with_buf(|b| b.push_line(&line));
        self.refresh_snapshot();
    }

    /// チャネルを drain してログスナップショットを更新する（UI 毎フレーム呼出）。
    pub fn drain_logs(&mut self) {
        self.log.drain(&self.log_rx);
        self.refresh_snapshot();
    }

    fn refresh_snapshot(&mut self) {
        self.log_snapshot = self
            .log
            .with_buf(|b| b.entries().cloned().collect())
            .unwrap_or_default();
    }

    /// ログをクリアする（クリアボタンのハンドラ）。
    pub fn clear_logs(&mut self) {
        self.log.with_buf(LogBuffer::clear);
        self.log_snapshot.clear();
    }

    /// 現在のログスナップショット（昇順・UI 描画とテストで使用）。
    #[allow(dead_code)]
    pub fn log_snapshot(&self) -> &[LogEntry] {
        &self.log_snapshot
    }

    /// 直近のエラーメッセージ(無ければ None)。
    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    /// ログビューア本体（シャード3前半: LogLevel 色分け・自動スクロール・クリア）。
    fn log_view_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("実行ログ");
            if ui.button("クリア").clicked() {
                self.clear_logs();
            }
            ui.checkbox(&mut self.auto_scroll, "自動スクロール");
        });
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .stick_to_bottom(self.auto_scroll)
            .show(ui, |ui| {
                if self.log_snapshot.is_empty() {
                    ui.weak("（ログなし）");
                }
                for entry in &self.log_snapshot {
                    ui.monospace(
                        egui::RichText::new(&entry.line)
                            .monospace()
                            .color(level_color(entry.level)),
                    );
                }
            });
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

            ui.separator();
            // 戦略選択パネル（シャード3）。
            self.strategy_panel.ui(ui);

            ui.separator();
            // ログビューア（シャード3前半・毎フレーム drain）。
            self.drain_logs();
            self.log_view_ui(ui);
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

    use std::path::PathBuf;

    fn exe_name() -> String {
        if cfg!(windows) {
            "anaden.exe".to_string()
        } else {
            "anaden".to_string()
        }
    }

    /// tempdir 直下に `target/<profile>/anaden[.exe]` を作る。
    fn make_target_binary(root: &Path, profile: &str) -> PathBuf {
        let dir = root.join("target").join(profile);
        std::fs::create_dir_all(&dir).unwrap();
        let bin = dir.join(exe_name());
        std::fs::write(&bin, b"dummy").unwrap();
        bin
    }

    #[test]
    fn test_pick_prefers_env_var_when_file_exists() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join(exe_name());
        std::fs::write(&bin, b"dummy").unwrap();
        let got = pick_anaden_bin(Some(bin.to_str().unwrap()), None, None).unwrap();
        assert_eq!(got, bin);
    }

    #[test]
    fn test_pick_env_var_missing_file_is_error() {
        let got = pick_anaden_bin(Some("/nonexistent/anaden-binary-zzz"), None, None);
        assert!(got.is_err());
        let err = got.unwrap_err();
        assert!(err.contains("ANADEN_BIN"), "unexpected: {err}");
    }

    #[test]
    fn test_pick_finds_target_debug_from_manifest_dir() {
        let dir = tempfile::tempdir().unwrap();
        let bin = make_target_binary(dir.path(), "debug");
        let got = pick_anaden_bin(None, Some(dir.path()), None).unwrap();
        assert_eq!(got, bin);
    }

    #[test]
    fn test_pick_finds_target_release_when_debug_absent() {
        let dir = tempfile::tempdir().unwrap();
        let bin = make_target_binary(dir.path(), "release");
        let got = pick_anaden_bin(None, Some(dir.path()), None).unwrap();
        assert_eq!(got, bin);
    }

    #[test]
    fn test_pick_falls_back_to_path_search() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join(exe_name());
        std::fs::write(&bin, b"dummy").unwrap();
        let path_var = dir.path().to_str().unwrap().to_string();
        let got = pick_anaden_bin(None, None, Some(&path_var)).unwrap();
        assert_eq!(got, bin);
    }

    #[test]
    fn test_pick_no_candidates_is_error() {
        let got = pick_anaden_bin(None, None, None);
        assert!(got.is_err());
    }

    /// UC-2: 解決失敗時にエラーが last_error に入り GUI がハングしない。
    #[test]
    fn test_unresolved_program_records_error_and_stays_stopped() {
        let mut app = PipelineRunnerApp::unresolved("anaden バイナリ解決失敗(テスト)");
        app.start_pipeline(&[]);
        assert_eq!(app.status(), RunnerStatus::Stopped);
        let err = app.last_error().expect("error should be recorded");
        assert!(err.contains("バイナリ解決失敗"), "unexpected error: {err}");
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
        assert!(app.log_snapshot().is_empty());
    }

    #[test]
    fn test_start_moves_to_running_and_stop_returns_to_stopped() {
        let mut app = PipelineRunnerApp::new(dummy_program());
        app.start_pipeline(&long_args());
        assert_eq!(app.status(), RunnerStatus::Running);
        app.stop_pipeline();
        assert_eq!(app.status(), RunnerStatus::Stopped);
    }

    /// UC-3: 起動失敗時にエラー行がログに表示される。
    #[test]
    fn test_spawn_failure_shows_error_line_in_log() {
        let mut app = PipelineRunnerApp::new("definitely-not-a-real-exe-xyz");
        app.start_pipeline(&[]);
        assert_eq!(app.status(), RunnerStatus::Stopped);
        let err = app.last_error().expect("error should be recorded");
        assert!(err.contains("起動に失敗"), "unexpected error: {err}");

        app.drain_logs();
        let snap = app.log_snapshot();
        assert!(!snap.is_empty(), "log should contain the error line");
        let last = snap.last().unwrap();
        assert!(
            last.line.contains("起動に失敗"),
            "unexpected log line: {}",
            last.line
        );
        assert_eq!(last.level, LogLevel::Error);
    }

    /// UC-3（解決失敗系）: 解決エラーもログへ ERROR 行として表示される。
    #[test]
    fn test_resolution_failure_shows_error_line_in_log() {
        let mut app = PipelineRunnerApp::unresolved("テスト用解決失敗");
        app.start_pipeline(&[]);
        app.drain_logs();
        let last = app.log_snapshot().last().expect("error line in log");
        assert_eq!(last.level, LogLevel::Error);
        assert!(last.line.contains("バイナリ解決失敗"));
    }

    /// 実行中の子の stdout が drain でログスナップショットに届く。
    #[test]
    fn test_running_child_stdout_appears_in_log_after_drain() {
        let mut app = PipelineRunnerApp::new(dummy_program());
        app.start_pipeline(&long_args());
        if app.status() == RunnerStatus::Running {
            // ping は即 stdout へ行を出す。数回 drain して届くまで待つ。
            for _ in 0..50 {
                app.drain_logs();
                if !app.log_snapshot().is_empty() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            let snap = app.log_snapshot();
            assert!(!snap.is_empty(), "stdout lines should be drained");
            app.stop_pipeline();
        }
        // ping が環境に無い場合は start が失敗するためスキップ相当。
    }

    /// クリアボタンのハンドラがログを空にする。
    #[test]
    fn test_clear_logs_empties_snapshot_and_buffer() {
        let mut app = PipelineRunnerApp::new(dummy_program());
        app.start_pipeline(&[]);
        app.drain_logs();
        app.clear_logs();
        assert!(app.log_snapshot().is_empty());
        let buf_empty = app.log.with_buf(|b| b.is_empty()).unwrap_or(false);
        assert!(buf_empty);
    }

    /// LogLevel → 表示色の対応（色分け描画の純ロジック部分）。
    #[test]
    fn test_level_color_varies_by_level() {
        assert_ne!(level_color(LogLevel::Error), level_color(LogLevel::Warn));
        assert_ne!(level_color(LogLevel::Warn), level_color(LogLevel::Info));
        assert_ne!(level_color(LogLevel::Error), level_color(LogLevel::Info));
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
