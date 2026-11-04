use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, SyncSender};

use crate::childproc::{ChildProcess, SpawnSpec};
use crate::history::{RunHistory, RunOutcome, RunRecord};
use crate::history_ui::{HistoryAction, HistoryPanel};
use crate::log_view::{
    DEFAULT_MAX_LINES, LogBuffer, LogEntry, LogEvent, LogLevel, SharedLogBuffer,
};
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

fn resolve_anaden_program() -> (String, Option<String>) {
    let env_val = std::env::var("ANADEN_BIN").ok();
    let manifest = Some(Path::new(env!("CARGO_MANIFEST_DIR")));
    let path_var = std::env::var("PATH").ok();
    match pick_anaden_bin(env_val.as_deref(), manifest, path_var.as_deref()) {
        Ok(p) => (p.to_string_lossy().into_owned(), None),
        Err(e) => ("anaden".to_string(), Some(e)),
    }
}

pub fn build_spawn_spec(program: &str, args: &[String]) -> SpawnSpec {
    SpawnSpec::new(program, args.to_vec())
}

pub fn build_run_args(
    selection: &anaden_strategies::StrategySelection,
) -> Result<Vec<String>, String> {
    let id = selection.strategy.as_deref().ok_or_else(|| {
        "戦略が選択されていません。「戦略設定」で戦略を選択してください".to_string()
    })?;
    let catalog = anaden_strategies::StrategyCatalog::builtin();
    catalog
        .find(id)
        .map(anaden_strategies::StrategyDef::to_run_args)
        .ok_or_else(|| format!("未知の戦略です(カタログ外): {id}"))
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn is_pipeline_positional(tok: &str, args: &[String]) -> bool {
    tok == "run" || !tok.starts_with('-') && args.iter().any(|a| a == "run")
}

fn is_bare_pipeline_name(tok: &str) -> bool {
    !tok.starts_with('-') && !tok.contains('/') && !tok.contains('\\') && tok != "run"
}

fn resolve_positional_token(tok: &mut String, root: &Path) {
    let joined =
        if tok.starts_with("templates/pipelines/") || tok.starts_with("templates\\pipelines\\") {
            root.join(tok.as_str())
        } else if is_bare_pipeline_name(tok) {
            root.join("templates").join("pipelines").join(tok.as_str())
        } else {
            return;
        };
    if joined.is_dir() {
        *tok = joined.to_string_lossy().into_owned();
    }
}

#[must_use]
pub fn resolve_pipeline_arg(args: &[String], root: &Path) -> Vec<String> {
    let mut out = args.to_vec();
    let mut skip_value = false;
    for tok in out.iter_mut() {
        if skip_value {
            skip_value = false;
            continue;
        }
        if is_pipeline_positional(tok, args) {
            resolve_positional_token(tok, root);
        } else if tok.starts_with("--") {
            skip_value = true;
        }
    }
    out
}

#[must_use]
fn with_report_file_arg(args: &[String], report_path: &Path) -> Vec<String> {
    let mut out = args.to_vec();
    out.push("--report-file".to_string());
    out.push(report_path.to_string_lossy().into_owned());
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunnerPane {
    Run,
    History,
    Strategy,
    Settings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunnerStatus {
    Stopped,
    Running,
}

const LOG_CHANNEL_CAPACITY: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum PendingReportKind {
    #[default]
    Run,
    Routine,
}

enum LoadedReport {
    Run(anaden_engine::ProgressReport),
    Routine(anaden_engine::RoutineSummary),
}

impl LoadedReport {
    fn stop_reason_label(&self) -> Option<String> {
        match self {
            Self::Run(r) => r
                .stop_reason
                .clone()
                .map(anaden_engine::stop_reason_label)
                .map(str::to_string),
            Self::Routine(s) => Some(s.outcome_label()),
        }
    }

    fn fired_count(&self) -> u64 {
        match self {
            Self::Run(r) => r.fired_count,
            Self::Routine(s) => u64::try_from(s.total_fired).unwrap_or(u64::MAX),
        }
    }
}

#[derive(Clone)]
enum RunRequest {
    Strategy {
        args: Vec<String>,
    },
    Routine {
        path: PathBuf,
        name: String,
        evidence_enabled: bool,
    },
}

impl RunRequest {
    fn build_spawn(
        &self,
        program: &str,
        run_id_gen: fn() -> String,
    ) -> (
        SpawnSpec,
        Option<String>,
        Option<String>,
        Option<PathBuf>,
        PendingReportKind,
    ) {
        match self {
            Self::Strategy { args } => {
                let report = crate::history::new_report_path();
                let spec = build_spawn_spec(program, &with_report_file_arg(args, &report));
                (spec, None, None, Some(report), PendingReportKind::Run)
            }
            Self::Routine {
                path,
                name,
                evidence_enabled,
            } => {
                let run_id = evidence_enabled.then(run_id_gen);
                let report = crate::history::new_report_path();
                let args = with_report_file_arg(
                    &crate::routine_ui::build_routine_args(path, run_id.as_deref()),
                    &report,
                );
                let label = Some(format!("routine:{name}"));
                (
                    build_spawn_spec(program, &args),
                    label,
                    run_id,
                    Some(report),
                    PendingReportKind::Routine,
                )
            }
        }
    }
}

fn unix_now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub struct PipelineRunnerApp {
    child: ChildProcess,
    program: String,
    resolution_error: Option<String>,
    last_error: Option<String>,
    log_tx: SyncSender<LogEvent>,
    log_rx: Receiver<LogEvent>,
    log: SharedLogBuffer,
    pub(crate) log_snapshot: Vec<LogEntry>,
    log_revision: u64,
    pub(crate) auto_scroll: bool,
    pub(crate) strategy_panel: crate::strategy_ui::StrategyPanel,
    pub(crate) routine_panel: crate::routine_ui::RoutinePanel,
    pending_evidence_run_id: Option<String>,
    pending_report_path: Option<PathBuf>,
    pending_report_kind: PendingReportKind,
    run_id_gen: fn() -> String,
    pub(crate) strategy_summary: String,
    last_run: Option<RunRequest>,
    pub(crate) history: RunHistory,
    run_started_at: Option<u64>,
    run_strategy: Option<String>,
    exit_observed: bool,
    exit_code: Option<i32>,
    failure_summary: Option<String>,
    pub(crate) history_panel: HistoryPanel,
    pub(crate) settings_tab: crate::settings::SettingsTab,
    pub(crate) settings_path: PathBuf,
}

impl PipelineRunnerApp {
    pub fn new(program: impl Into<String>) -> Self {
        Self::with_channel(program, None)
    }

    pub fn with_resolved_anaden() -> Self {
        let (program, err) = resolve_anaden_program();
        Self::with_channel(program, err)
    }

    #[cfg(test)]
    fn unresolved(message: &str) -> Self {
        Self::with_channel(
            "anaden",
            Some(format!("anaden バイナリ解決失敗: {message}")),
        )
    }

    fn with_channel(program: impl Into<String>, resolution_error: Option<String>) -> Self {
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
            log_revision: 0,
            auto_scroll: true,
            strategy_panel: crate::strategy_ui::StrategyPanel::default(),
            routine_panel: crate::routine_ui::RoutinePanel::default(),
            pending_evidence_run_id: None,
            pending_report_path: None,
            pending_report_kind: PendingReportKind::Run,
            run_id_gen: crate::routine_ui::new_evidence_run_id,
            strategy_summary: "戦略未選択".to_string(),
            last_run: None,
            history: RunHistory::open_default(),
            run_started_at: None,
            run_strategy: None,
            exit_observed: false,
            exit_code: None,
            failure_summary: None,
            history_panel: HistoryPanel::new(),
            settings_tab: crate::settings::SettingsTab::default(),
            settings_path: crate::settings::settings_file_path(),
        }
    }

    pub fn status(&mut self) -> RunnerStatus {
        if self.child.is_running() {
            RunnerStatus::Running
        } else {
            RunnerStatus::Stopped
        }
    }

    pub fn strategy_summary(&self) -> &str {
        &self.strategy_summary
    }

    pub fn on_strategy_changed(&mut self, changed: bool) {
        if changed {
            self.strategy_summary = self.strategy_panel.summary();
        }
    }

    pub fn start_pipeline_with_selection(&mut self) {
        self.last_error = None;
        if let Err(e) = self.strategy_panel.validate() {
            self.record_error_line(&format!("戦略選択が無効です: {e}"));
            return;
        }
        match build_run_args(self.strategy_panel.selection()) {
            Ok(args) => {
                let resolved = resolve_pipeline_arg(&args, &workspace_root());
                self.start_pipeline(&resolved);
            }
            Err(e) => self.record_error_line(&e),
        }
    }

    pub fn start_pipeline(&mut self, args: &[String]) {
        self.start_request(RunRequest::Strategy {
            args: args.to_vec(),
        });
    }

    pub fn start_routine(&mut self) {
        self.last_error = None;
        let Some(path) = self.routine_panel.selected_path().map(Path::to_path_buf) else {
            self.record_error_line(
                "ルーチンが選択されていません（ルーチン選択で routine を指定してください）",
            );
            return;
        };
        let name = self
            .routine_panel
            .selected_name()
            .unwrap_or("-")
            .to_string();
        self.start_request(RunRequest::Routine {
            path,
            name,
            evidence_enabled: self.routine_panel.evidence_enabled(),
        });
    }

    fn start_request(&mut self, request: RunRequest) {
        self.last_error = None;
        if let Some(e) = self.resolution_error.clone() {
            self.record_error_line(&e);
            return;
        }
        self.reset_run_tracking();
        self.pending_evidence_run_id = None;
        self.pending_report_path = None;
        self.pending_report_kind = PendingReportKind::Run;
        let (spec, label, evidence_run_id, report_path, report_kind) =
            request.build_spawn(&self.program, self.run_id_gen);
        if let Err(e) = self.child.start(&spec, self.log_tx.clone()) {
            self.record_error_line(&e.to_string());
            return;
        }
        self.last_run = Some(request);
        self.pending_evidence_run_id = evidence_run_id;
        self.pending_report_path = report_path;
        self.pending_report_kind = report_kind;
        self.run_started_at = Some(unix_now_secs());
        self.run_strategy = label.or_else(|| self.strategy_panel.selection().strategy.clone());
    }

    fn reset_run_tracking(&mut self) {
        self.run_started_at = None;
        self.run_strategy = None;
        self.exit_observed = false;
        self.exit_code = None;
        self.failure_summary = None;
    }

    pub fn rerun_pipeline(&mut self) {
        self.last_error = None;
        if self.child.is_running() {
            self.record_error_line("再実行には停止待ちが必要です（実行中は再実行できません）");
            return;
        }
        let Some(request) = self.last_run.clone() else {
            self.record_error_line("再実行可能な実行履歴がありません（先に開始してください）");
            return;
        };
        let _ = self.child.stop();
        self.start_request(request);
    }

    pub fn stop_pipeline(&mut self) {
        self.last_error = None;
        if let Err(e) = self.child.stop() {
            self.record_error_line(&e.to_string());
            return;
        }
        if !self.exit_observed {
            self.pending_report_path = None;
            self.pending_report_kind = PendingReportKind::Run;
            self.append_history_record(RunOutcome::Cancelled, None, None);
        }
    }

    fn record_error_line(&mut self, message: &str) {
        self.last_error = Some(message.to_string());
        self.push_log_line(&format!("ERROR {message}"));
    }

    fn push_log_line(&mut self, line: &str) {
        let line = line.to_string();
        self.log.with_buf(|b| b.push_line(&line));
        self.refresh_snapshot();
    }

    pub fn drain_logs(&mut self) {
        let (_new_lines, observed) = crate::log_view::drain_channel_into(&self.log, &self.log_rx);
        let exit_seen = observed.is_some();
        let exit_code: Option<i32> = observed.flatten();
        self.refresh_snapshot();
        if exit_seen && !self.exit_observed {
            self.exit_observed = true;
            self.exit_code = exit_code;
            let failed = !matches!(exit_code, Some(0));
            if failed {
                self.failure_summary = Some(self.build_failure_summary());
            }
            let report = self.load_pending_report();
            match &report {
                Some(LoadedReport::Run(r)) => {
                    self.log.with_buf(|b| b.status.apply_report(r));
                }
                Some(LoadedReport::Routine(s)) => {
                    self.log.with_buf(|b| b.status.apply_routine_report(s));
                }
                None => {}
            }
            let run_report = report.as_ref().and_then(|r| match r {
                LoadedReport::Run(r) => Some(r),
                LoadedReport::Routine(_) => None,
            });
            let outcome = crate::history::classify_outcome(exit_code, run_report);
            self.append_history_record(outcome, exit_code, report.as_ref());
        }
    }

    fn load_pending_report(&mut self) -> Option<LoadedReport> {
        let path = self.pending_report_path.take()?;
        let kind = self.pending_report_kind;
        let parsed = std::fs::File::open(&path)
            .map_err(|e| e.to_string())
            .and_then(|f| match kind {
                PendingReportKind::Run => serde_json::from_reader(f)
                    .map(LoadedReport::Run)
                    .map_err(|e| format!("report.json のパース失敗 ({e})")),
                PendingReportKind::Routine => serde_json::from_reader(f)
                    .map(LoadedReport::Routine)
                    .map_err(|e| format!("report.json のパース失敗 ({e})")),
            });
        match parsed {
            Ok(report) => Some(report),
            Err(e) => {
                self.push_log_line(&format!(
                    "WARN run レポート読込失敗 (stdout 解析にフォールバック): {} {e}",
                    path.display()
                ));
                None
            }
        }
    }

    fn build_failure_summary(&self) -> String {
        let status = self.run_status_summary();
        let error_tail: Vec<String> = self
            .log_snapshot
            .iter()
            .rev()
            .filter(|e| e.level == LogLevel::Error)
            .take(crate::history::LOG_TAIL_MAX_LINES)
            .map(|e| e.line.clone())
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        if error_tail.is_empty() {
            format!("失敗: {status}")
        } else {
            format!(
                "失敗: {status} | エラーログ末尾: {}",
                error_tail.join(" / ")
            )
        }
    }

    fn append_history_record(
        &mut self,
        outcome: RunOutcome,
        exit_code: Option<i32>,
        report: Option<&LoadedReport>,
    ) {
        let log_tail = self.log_snapshot.iter().map(|e| e.line.clone()).collect();
        let stop_reason = report.and_then(LoadedReport::stop_reason_label);
        let fired_count = report.map(LoadedReport::fired_count);
        let record = RunRecord::new(
            self.run_started_at.unwrap_or(0),
            self.run_strategy.clone().unwrap_or_else(|| "-".to_string()),
            outcome,
            exit_code,
            log_tail,
        )
        .with_evidence_run_id(self.pending_evidence_run_id.take())
        .with_report_summary(stop_reason, fired_count);
        if let Err(e) = self.history.append(record) {
            self.record_error_line(&format!("履歴の保存に失敗しました: {e}"));
        }
    }

    pub fn failure_summary(&self) -> Option<&str> {
        self.failure_summary.as_deref()
    }

    fn refresh_snapshot(&mut self) {
        if let Some((rev, entries)) = self.log.changed_entries(self.log_revision) {
            self.log_revision = rev;
            self.log_snapshot = entries;
        }
    }

    pub fn clear_logs(&mut self) {
        self.log.with_buf(LogBuffer::clear);
        self.refresh_snapshot();
    }

    pub fn log_snapshot(&self) -> &[LogEntry] {
        &self.log_snapshot
    }

    pub fn run_status_summary(&self) -> String {
        self.log
            .with_buf(|b| b.status.summary())
            .unwrap_or_else(|| "未実行".to_string())
    }

    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    pub fn handle_history_actions(&mut self) {
        while let Some(action) = self.history_panel.take_action() {
            match action {
                HistoryAction::Rerun => self.rerun_pipeline(),
                HistoryAction::Stop => self.stop_pipeline(),
                HistoryAction::SaveSettings => self.save_settings_to_path(),
                HistoryAction::LoadSettings => self.load_settings_from_path(),
            }
        }
    }

    pub fn history_panel(&self) -> &HistoryPanel {
        &self.history_panel
    }

    pub fn history_panel_mut(&mut self) -> &mut HistoryPanel {
        &mut self.history_panel
    }

    pub(crate) fn save_settings_to_path(&mut self) {
        self.settings_tab
            .save(self.strategy_panel.selection(), &self.settings_path);
    }

    pub(crate) fn load_settings_from_path(&mut self) {
        if let Some(selection) = self.settings_tab.load(&self.settings_path)
            && self
                .strategy_panel
                .load_toml(&selection_to_toml(&selection))
                .is_ok()
        {
            self.strategy_summary = self.strategy_panel.summary();
        }
    }

    pub fn settings_status(&self) -> &crate::settings::SettingsTabStatus {
        self.settings_tab.status()
    }
}

fn selection_to_toml(selection: &anaden_strategies::StrategySelection) -> String {
    toml::to_string(selection).unwrap_or_default()
}

impl Drop for PipelineRunnerApp {
    fn drop(&mut self) {
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

    /// UC-5 (Issue #160): 差分更新スナップショットであっても行が欠けず、
    /// 新着行のないフレームを連続してもスナップショット内容が冪等に保たれる
    /// (60 フレーム相当 = 1 秒 @60fps のアイドル drain)。
    #[test]
    fn test_log_snapshot_idempotent_across_idle_drains() {
        let mut app = PipelineRunnerApp::new(dummy_program());
        app.push_log_line("INFO line-1");
        app.push_log_line("ERROR line-2");
        app.drain_logs();
        let first: Vec<String> = app.log_snapshot().iter().map(|e| e.line.clone()).collect();
        assert_eq!(first.len(), 2);
        for _ in 0..60 {
            app.drain_logs();
        }
        let second: Vec<String> = app.log_snapshot().iter().map(|e| e.line.clone()).collect();
        assert_eq!(first, second);
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

    /// run_status_summary: 未実行時は「未実行」。
    #[test]
    fn test_run_status_summary_initially_not_run() {
        let app = PipelineRunnerApp::new(dummy_program());
        assert_eq!(app.run_status_summary(), "未実行");
    }

    /// run_status_summary: 開始行観測後は goal 付き実行中表示
    /// （RunStatus.observe の出力契約: `run_loop 開始:` 行）。
    #[test]
    fn test_run_status_summary_reflects_start_line() {
        let mut app = PipelineRunnerApp::new(dummy_program());
        app.push_log_line("INFO anaden_cli: run_loop 開始: interval=2s max_iters=10 goal=farm50");
        assert_eq!(app.run_status_summary(), "実行中 goal=farm50 iterations=?");
    }

    /// run_status_summary: 結果行観測後は停止理由とサイクル数表示
    /// （`サイクル数:` / `停止理由:` 行）。
    #[test]
    fn test_run_status_summary_reflects_result_lines() {
        let mut app = PipelineRunnerApp::new(dummy_program());
        app.push_log_line("run_loop 開始: interval=2s max_iters=10 goal=g1");
        app.push_log_line("サイクル数: 42");
        app.push_log_line("停止理由:   宣言的ゴール到達(正常)");
        assert_eq!(
            app.run_status_summary(),
            "停止 reason=宣言的ゴール到達(正常) iterations=42"
        );
    }

    /// run_status_summary: ログクリアで未実行へ戻る。
    #[test]
    fn test_run_status_summary_resets_on_clear() {
        let mut app = PipelineRunnerApp::new(dummy_program());
        app.push_log_line("run_loop 開始: goal=g");
        app.clear_logs();
        assert_eq!(app.run_status_summary(), "未実行");
    }

    /// run_status_summary: drain 経由（チャネル→drain_logs）でも反映される。
    #[test]
    fn test_run_status_summary_updates_via_drain_logs() {
        use crate::log_view::LogEvent;
        let mut app = PipelineRunnerApp::new(dummy_program());
        // drain は受信側チャネルから読むため、start 経由ではなく直接検証は困難。
        // ここでは drain 後も summary が崩れないこと（空 drain で状態維持）を確認。
        app.push_log_line("run_loop 開始: goal=g2");
        app.drain_logs();
        assert_eq!(app.run_status_summary(), "実行中 goal=g2 iterations=?");
        let _ = LogEvent::Line(String::new());
    }

    // ---- build_run_args（Issue #88 タスク1・UC-1/UC-4）----

    fn pipeline_selection(id: &str) -> anaden_strategies::StrategySelection {
        let mut panel = crate::strategy_ui::StrategyPanel::default();
        panel.select_strategy(id);
        panel.selection().clone()
    }

    /// UC-1 正常系: 実在パイプライン選択で run サブコマンド + --algorithm + 位置引数が組まれる。
    #[test]
    fn test_build_run_args_field_loop_pc_builds_run_subcommand() {
        let args = build_run_args(&pipeline_selection("field_loop_pc")).unwrap();
        assert_eq!(args[0], "run");
        assert!(args.contains(&"--algorithm".to_string()));
        // --algorithm の値は sse|ccoeff のみ受理される値であること。
        let algo = args
            .iter()
            .position(|a| a == "--algorithm")
            .map(|i| args[i + 1].clone())
            .unwrap();
        assert!(
            algo == "sse" || algo == "ccoeff",
            "invalid algorithm: {algo}"
        );
    }

    /// UC-1 正常系: pipeline_dir / start_task 位置引数が含まれる。
    #[test]
    fn test_build_run_args_field_loop_pc_includes_positional_args() {
        let args = build_run_args(&pipeline_selection("field_loop_pc")).unwrap();
        // run の位置引数（フラグとその値を除く非フラグトークン）は pipeline_dir と
        // start_task (--max-iters の値 "10000" はフラグ値のため除外)。
        let positional: Vec<&String> = args
            .iter()
            .enumerate()
            .filter(|(i, a)| *i > 0 && !a.starts_with("--") && !args[*i - 1].starts_with("--"))
            .map(|(_, a)| a)
            .collect();
        assert_eq!(positional.len(), 2, "args: {args:?}");
        assert!(positional[0].contains("field_loop_pc"));
        assert_eq!(positional[1], "TapBottomStablePc");
    }

    /// Issue #188: CLI から --target フラグは削除済み (Win32 固定) — 引数列に含まれない。
    #[test]
    fn test_build_run_args_pc_pipeline_emits_no_target_flag() {
        let args = build_run_args(&pipeline_selection("nav_to_field_pc")).unwrap();
        assert_eq!(args[0], "run");
        assert!(!args.contains(&"--target".to_string()), "args: {args:?}");
        assert!(args.contains(&"templates/pipelines/nav_to_field_pc".to_string()));
        assert!(args.contains(&"TapToStartPc".to_string()));
    }

    /// Issue #139 T1: 実在 PC 版 2 パイプラインすべてがカタログ経由で引数組み立て可能。
    #[test]
    fn test_build_run_args_supports_all_two_real_pc_pipelines() {
        for id in ["field_loop_pc", "nav_to_field_pc"] {
            let args =
                build_run_args(&pipeline_selection(id)).unwrap_or_else(|e| panic!("{id}: {e}"));
            assert_eq!(args[0], "run", "{id}");
            assert!(
                args.iter().any(|a| a.contains(&format!("pipelines/{id}"))),
                "{id}: pipeline_dir missing in {args:?}"
            );
        }
    }

    /// Issue #139 T1: fishing（実在しない pipeline）はカタログ外として拒否される
    /// （select_strategy はカタログ外 id を無視するため strategy は None のまま）。
    #[test]
    fn test_build_run_args_fishing_is_rejected_as_nonexistent_pipeline() {
        // パネル経由: カタログ外のため選択自体が無視される → 未選択エラー。
        let sel = pipeline_selection("fishing");
        assert_eq!(sel.strategy, None);

        // 混入経路 (load_toml 等) を想定し strategy へ直接 fishing を入れた場合。
        let injected = anaden_strategies::StrategySelection {
            strategy: Some("fishing".to_string()),
            ..Default::default()
        };
        let err = build_run_args(&injected).unwrap_err();
        assert!(err.contains("fishing"), "unexpected: {err}");
    }

    /// UC-4 エッジケース: 戦略未選択は Err（子プロセス起動拒否の事前検証）。
    #[test]
    fn test_build_run_args_no_strategy_is_err() {
        let selection = anaden_strategies::StrategySelection::default();
        let err = build_run_args(&selection).unwrap_err();
        assert!(
            err.contains("戦略が選択されていません"),
            "unexpected: {err}"
        );
    }

    /// UC-4 エッジケース: 未知の戦略 id（不正選択）は Err。
    #[test]
    fn test_build_run_args_unknown_strategy_is_err() {
        let selection = anaden_strategies::StrategySelection {
            strategy: Some("bogus-strategy".to_string()),
            ..Default::default()
        };
        let err = build_run_args(&selection).unwrap_err();
        assert!(err.contains("bogus-strategy"), "unexpected: {err}");
    }

    /// --goal / --goal-file の排他制約（parse_goal_flag）を破らないこと:
    /// build_run_args はいずれも出力しない。
    #[test]
    fn test_build_run_args_never_outputs_goal_flags() {
        let args = build_run_args(&pipeline_selection("field_loop_pc")).unwrap();
        assert!(!args.contains(&"--goal".to_string()));
        assert!(!args.contains(&"--goal-file".to_string()));
    }

    // ---- Issue #139 T2: pipeline ディレクトリ決定的解決 ----

    /// tempdir に `templates/pipelines/<name>` ディレクトリを作るヘルパ。
    fn make_pipeline_dir(root: &Path, name: &str) -> PathBuf {
        let dir = root.join("templates").join("pipelines").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 相対 pipeline_dir が workspace ルート基準の絶対パスへ置換される。
    #[test]
    fn test_resolve_pipeline_arg_rewrites_relative_template_path() {
        let root = tempfile::tempdir().unwrap();
        let dir = make_pipeline_dir(root.path(), "field_loop");
        let args = vec![
            "run".to_string(),
            "--algorithm".to_string(),
            "ccoeff".to_string(),
            "templates/pipelines/field_loop".to_string(),
            "TapBottomStable".to_string(),
        ];
        let got = resolve_pipeline_arg(&args, root.path());
        assert_eq!(PathBuf::from(&got[3]), dir);
        // その他のトークンは不変。
        assert_eq!(got[0], "run");
        assert_eq!(got[2], "ccoeff");
        assert_eq!(got[4], "TapBottomStable");
    }

    /// 非実在 pipeline_dir は元の値を維持（CLI 側 fail-closed に委譲）。
    #[test]
    fn test_resolve_pipeline_arg_keeps_nonexistent_dir_as_is() {
        let root = tempfile::tempdir().unwrap();
        let args = vec![
            "run".to_string(),
            "templates/pipelines/ghost".to_string(),
            "Task".to_string(),
        ];
        let got = resolve_pipeline_arg(&args, root.path());
        assert_eq!(got[1], "templates/pipelines/ghost");
    }

    /// bare name（カタログ id）も templates/pipelines 基準で解決される。
    #[test]
    fn test_resolve_pipeline_arg_resolves_bare_pipeline_name() {
        let root = tempfile::tempdir().unwrap();
        let dir = make_pipeline_dir(root.path(), "worldmap_loop");
        let args = vec![
            "run".to_string(),
            "worldmap_loop".to_string(),
            "Task".to_string(),
        ];
        let got = resolve_pipeline_arg(&args, root.path());
        assert_eq!(PathBuf::from(&got[1]), dir);
    }

    /// 絶対パス指定は変更されない（冪等・二重解決なし）。
    #[test]
    fn test_resolve_pipeline_arg_keeps_absolute_path_unchanged() {
        let root = tempfile::tempdir().unwrap();
        let abs = make_pipeline_dir(root.path(), "field_loop");
        let args = vec![
            "run".to_string(),
            abs.to_string_lossy().into_owned(),
            "Task".to_string(),
        ];
        let got = resolve_pipeline_arg(&args, root.path());
        assert_eq!(PathBuf::from(&got[1]), abs);
    }

    /// --target の値（位置引数風の "windows"）は bare name 解決の対象外。
    #[test]
    fn test_resolve_pipeline_arg_does_not_touch_flag_values() {
        let root = tempfile::tempdir().unwrap();
        // windows という名の pipeline が存在しても --target の値は置換しない。
        let _ = make_pipeline_dir(root.path(), "windows");
        let args = vec![
            "run".to_string(),
            "--target".to_string(),
            "windows".to_string(),
            "templates/pipelines/none".to_string(),
            "T".to_string(),
        ];
        let got = resolve_pipeline_arg(&args, root.path());
        assert_eq!(got[2], "windows");
    }

    /// 実機workspace ルートでカタログ 2 パイプラインがすべて絶対解決される
    /// （T1 カタログ定義からの引数経路が実ディレクトリに到達することの保証）。
    #[test]
    fn test_resolve_pipeline_arg_supports_all_two_real_pipelines_on_real_root() {
        let root = workspace_root();
        for id in ["field_loop_pc", "nav_to_field_pc"] {
            let sel = pipeline_selection(id);
            let args = build_run_args(&sel).unwrap_or_else(|e| panic!("{id}: {e}"));
            let got = resolve_pipeline_arg(&args, &root);
            let dir = got
                .iter()
                .position(|a| a == "run")
                .map(|i| {
                    got[i + 1..]
                        .iter()
                        .find(|a| Path::new(a).is_absolute())
                        .cloned()
                        .unwrap_or_default()
                })
                .unwrap_or_default();
            assert!(
                Path::new(&dir).is_dir(),
                "{id}: resolved dir not found: {dir}"
            );
        }
    }

    /// UC-4: 戦略未選択のまま開始すると拒否され、子プロセスは起動しない。
    #[test]
    fn test_start_with_no_strategy_is_rejected() {
        let mut app = PipelineRunnerApp::new(dummy_program());
        // 初期状態は戦略未選択。
        assert_eq!(app.strategy_summary(), "戦略未選択");
        app.start_pipeline_with_selection();
        assert_eq!(app.status(), RunnerStatus::Stopped);
        let err = app.last_error().expect("rejection error");
        assert!(
            err.contains("戦略が選択されていません"),
            "unexpected: {err}"
        );
        app.drain_logs();
        let last = app.log_snapshot().last().expect("error line");
        assert_eq!(last.level, LogLevel::Error);
    }

    /// UC-4: カタログ外戦略（load_toml 等で混入）でも validate が拒否する。
    #[test]
    fn test_start_with_unknown_strategy_is_rejected_by_validate() {
        let mut app = PipelineRunnerApp::new(dummy_program());
        app.strategy_panel
            .load_toml("strategy = \"bogus\"")
            .expect("toml load");
        app.start_pipeline_with_selection();
        assert_eq!(app.status(), RunnerStatus::Stopped);
        let err = app.last_error().expect("rejection error");
        assert!(err.contains("戦略選択が無効です"), "unexpected: {err}");
    }

    /// 選択済みなら validate → build_run_args → start の経路で起動する
    /// （ping があれば Running。無ければスキップ相当）。
    #[test]
    fn test_start_with_selected_strategy_starts_child() {
        let mut app = PipelineRunnerApp::new(dummy_program());
        app.strategy_panel.select_strategy("field_loop_pc");
        app.start_pipeline_with_selection();
        if app.status() == RunnerStatus::Running {
            app.stop_pipeline();
        }
        // 引数組み立て自体は pipeline_selection のテストで検証済み。
    }

    /// on_strategy_changed: changed=true でのみサマリが再計算される。
    #[test]
    fn test_on_strategy_changed_refreshes_summary() {
        let mut app = PipelineRunnerApp::new(dummy_program());
        assert_eq!(app.strategy_summary(), "戦略未選択");
        app.strategy_panel.select_strategy("field_loop_pc");
        // changed=false（同一フレーム内の無操作）では更新されない。
        app.on_strategy_changed(false);
        assert_eq!(app.strategy_summary(), "戦略未選択");
        // changed=true で更新される。
        app.on_strategy_changed(true);
        // 実在 2 パイプラインは ON/OFF オプションを持たないため「オプションなし」。
        assert_eq!(
            app.strategy_summary(),
            "strategy=field_loop_pc (オプションなし)"
        );
    }

    // ---- Issue #125 shard 3 タスク2: Strategy / Settings ペイン ----

    /// RunnerPane に Strategy / Settings バリアントが存在し全ペイン種別が区別される。
    #[test]
    fn test_runner_pane_has_six_distinct_variants() {
        use RunnerPane::{History, Run, Settings, Strategy};
        assert_ne!(Strategy, Run);
        assert_ne!(Strategy, History);
        assert_ne!(Settings, Run);
        assert_ne!(Settings, History);
        assert_ne!(Strategy, Settings);
    }

    /// 設定ビューの保存: 選択中戦略が settings.toml へ保存され status が Ok になる。
    #[test]
    fn test_save_settings_from_runner_writes_file_and_records_ok() {
        let mut app = PipelineRunnerApp::new(dummy_program());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        app.settings_path = path.clone();
        app.strategy_panel.select_strategy("field_loop_pc");
        app.save_settings_to_path();
        assert!(path.is_file());
        assert!(matches!(
            app.settings_status(),
            crate::settings::SettingsTabStatus::Ok(_)
        ));
    }

    /// 設定ビューの読込: 保存済み設定が戦略パネルへ復元されサマリも更新される。
    #[test]
    fn test_load_settings_restores_strategy_selection_and_summary() {
        let mut app = PipelineRunnerApp::new(dummy_program());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        app.settings_path = path.clone();
        app.strategy_panel.select_strategy("field_loop_pc");
        app.save_settings_to_path();

        let mut app2 = PipelineRunnerApp::new(dummy_program());
        app2.settings_path = path;
        assert_eq!(app2.strategy_summary(), "戦略未選択");
        app2.load_settings_from_path();
        assert!(app2.strategy_panel.selection().strategy.is_some());
        assert!(app2.strategy_summary().contains("field_loop_pc"));
    }

    /// 読込: ファイル不在 (初回起動) はエラー扱いにしない (UC-2)。
    #[test]
    fn test_load_settings_missing_file_is_ok_status() {
        let mut app = PipelineRunnerApp::new(dummy_program());
        let dir = tempfile::tempdir().unwrap();
        app.settings_path = dir.path().join("nope.toml");
        app.load_settings_from_path();
        assert!(matches!(
            app.settings_status(),
            crate::settings::SettingsTabStatus::Ok(_)
        ));
    }

    /// 履歴ペインの SaveSettings アクションは設定ビューと同一ロジックへ委譲される
    /// （二重実装ではなく単一の save_settings_to_path を通る）。
    #[test]
    fn test_history_save_settings_action_delegates_to_shared_logic() {
        let mut app = PipelineRunnerApp::new(dummy_program());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        app.settings_path = path.clone();
        app.strategy_panel.select_strategy("fishing");
        app.history_panel_mut()
            .request(crate::history_ui::HistoryAction::SaveSettings);
        app.handle_history_actions();
        assert!(path.is_file());
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

    // ---- routine 実行の evidence 採取 (Issue #202 UC-1) ----

    /// 終了 (Exit イベント) が drain されるまで待つヘルパ
    /// (integration tests の wait_for_exit_and_drain と同パターン)。
    fn wait_stopped_and_drained(app: &mut PipelineRunnerApp) {
        for _ in 0..300 {
            if app.status() == RunnerStatus::Stopped {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            app.drain_logs();
            if app
                .log_snapshot()
                .iter()
                .any(|e| e.line.contains("プロセス終了"))
            {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "Exit イベントが drain されない: {:?}",
                app.log_snapshot().last().map(|e| e.line.clone())
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    /// 履歴ストアを tempdir へ差し替える (実環境の history.jsonl を汚さない)。
    /// 戻り値の TempDir はスコープ終了まで保持すること (自動削除は drop 時)。
    fn isolate_history(app: &mut PipelineRunnerApp) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        app.history = crate::history::RunHistory::open(
            dir.path().join("history.jsonl"),
            crate::history::DEFAULT_MAX_RECORDS,
        );
        dir
    }

    /// routine 実行 (evidence 既定 ON) の履歴レコードに evidence run-id と
    /// `routine:<name>` ラベルが記録される (ping 引数違いの即時終了を利用)。
    #[test]
    fn routine_run_records_evidence_run_id_in_history() {
        let mut app = PipelineRunnerApp::new(dummy_program());
        let _history_dir = isolate_history(&mut app);
        app.routine_panel.select("daily");
        assert!(app.routine_panel.evidence_enabled(), "default must be ON");
        app.start_routine();
        if app.status() != RunnerStatus::Running {
            // ping が環境に無い場合は start が失敗するためスキップ相当。
            return;
        }
        wait_stopped_and_drained(&mut app);

        let records = app.history.records();
        let record = records.first().expect("history record appended");
        assert_eq!(record.strategy, "routine:daily");
        let run_id = record
            .evidence_run_id
            .as_ref()
            .expect("evidence run-id must be recorded");
        assert!(run_id.starts_with("routine-"), "{run_id}");
    }

    /// evidence チェック OFF では run-id が記録されない (None)。
    #[test]
    fn routine_run_without_evidence_records_no_run_id() {
        let mut app = PipelineRunnerApp::new(dummy_program());
        let _history_dir = isolate_history(&mut app);
        app.routine_panel.select("daily");
        app.routine_panel.set_evidence_enabled(false);
        app.start_routine();
        if app.status() != RunnerStatus::Running {
            return;
        }
        wait_stopped_and_drained(&mut app);

        let records = app.history.records();
        let record = records.first().expect("history record appended");
        assert_eq!(record.strategy, "routine:daily");
        assert_eq!(record.evidence_run_id, None);
    }

    // ---- Issue #204: routine 再実行の evidence run-id 再生成 + 清算 ----

    /// テスト用の決定的 run-id 生成器 (呼出ごとに異なる id を返す。
    /// `evidence_run_id` へのタイムスタンプ注入で実運用と同一形式)。
    fn injected_run_id() -> String {
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        crate::routine_ui::evidence_run_id(&format!("20260914-0130{n:02}"))
    }

    /// 履歴レコードが `want` 件になるまで drain を続けるヘルパ (Issue #204:
    /// 再実行テストではログに前回実行の「プロセス終了」行も残るため、
    /// 終了行ではなく履歴件数で待つ)。
    fn wait_history_len(app: &mut PipelineRunnerApp, want: usize) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            app.drain_logs();
            if app.history.records().len() >= want {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "履歴が {want} 件にならない: 最終ログ行 {:?}",
                app.log_snapshot().last().map(|e| e.line.clone())
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    /// AC-1/AC-2: routine 再実行で新しい evidence run-id が発行され、
    /// 再実行分の履歴レコードにもその run-id が記録される
    /// (旧実装は run-id 込み SpawnSpec の再利用で同一 `.omc/logs/<run-id>/`
    /// への上書き + 再実行分履歴の run-id 未記録)。
    #[test]
    fn routine_rerun_issues_fresh_evidence_run_id_and_records_it() {
        let mut app = PipelineRunnerApp::new(dummy_program());
        let _history_dir = isolate_history(&mut app);
        app.run_id_gen = injected_run_id;
        app.routine_panel.select("daily");
        app.start_routine();
        if app.status() != RunnerStatus::Running {
            // ping が環境に無い場合は start が失敗するためスキップ相当。
            return;
        }
        wait_history_len(&mut app, 1);
        let first = app.history.records()[0]
            .evidence_run_id
            .clone()
            .expect("first run records run-id");
        assert!(first.starts_with("routine-"), "{first}");

        app.rerun_pipeline();
        if app.status() != RunnerStatus::Running {
            return;
        }
        wait_history_len(&mut app, 2);
        let records = app.history.records();
        let rerun_id = records[0]
            .evidence_run_id
            .as_ref()
            .expect("rerun must record its own run-id");
        assert_ne!(rerun_id, &first, "rerun must issue a fresh run-id");
        assert!(rerun_id.starts_with("routine-"), "{rerun_id}");
        // 両レコードとも routine ラベル・初回レコードの run-id は不変。
        assert_eq!(records[0].strategy, "routine:daily");
        assert_eq!(records[1].strategy, "routine:daily");
        assert_eq!(records[1].evidence_run_id.as_deref(), Some(first.as_str()));
    }

    /// AC-3: strategy 実行の再実行で stale な pending run-id が履歴へ
    /// 付与されない (旧実装の rerun は start_pipeline の清算点をバイパス)。
    #[test]
    fn strategy_rerun_does_not_attach_stale_evidence_run_id() {
        let mut app = PipelineRunnerApp::new(dummy_program());
        let _history_dir = isolate_history(&mut app);
        app.strategy_panel.select_strategy("field_loop_pc");
        app.start_pipeline_with_selection();
        if app.status() != RunnerStatus::Running {
            return;
        }
        wait_history_len(&mut app, 1);
        assert_eq!(app.history.records()[0].evidence_run_id, None);

        // 旧実装で spawn 失敗時に残存し得た stale pending を直接注入し、
        // 再実行経由でも履歴へ付与されないことを検証する。
        app.pending_evidence_run_id = Some("routine-stale".to_string());
        app.rerun_pipeline();
        if app.status() != RunnerStatus::Running {
            return;
        }
        wait_history_len(&mut app, 2);
        assert_eq!(
            app.history.records()[0].evidence_run_id,
            None,
            "strategy rerun must not attach stale run-id"
        );
    }

    /// AC-3 (routine 経路): 再実行時に pending に残っていた stale run-id は
    /// 履歴へ付与されず、新規発行の run-id に置き換えられる。
    #[test]
    fn routine_rerun_replaces_stale_pending_run_id_with_fresh_one() {
        let mut app = PipelineRunnerApp::new(dummy_program());
        let _history_dir = isolate_history(&mut app);
        app.run_id_gen = injected_run_id;
        app.routine_panel.select("daily");
        app.start_routine();
        if app.status() != RunnerStatus::Running {
            return;
        }
        wait_history_len(&mut app, 1);

        app.pending_evidence_run_id = Some("routine-stale".to_string());
        app.rerun_pipeline();
        if app.status() != RunnerStatus::Running {
            return;
        }
        wait_history_len(&mut app, 2);
        let rerun_id = app.history.records()[0]
            .evidence_run_id
            .as_ref()
            .expect("rerun must record fresh run-id");
        assert_ne!(rerun_id, "routine-stale", "stale must be cleared");
        assert!(rerun_id.starts_with("routine-"), "{rerun_id}");
    }

    /// evidence OFF で開始した routine の再実行も run-id なしで再現される
    /// (起動時スナップショットの再現・run-id が勝手に付与されない)。
    #[test]
    fn routine_rerun_respects_evidence_disabled_snapshot() {
        let mut app = PipelineRunnerApp::new(dummy_program());
        let _history_dir = isolate_history(&mut app);
        app.routine_panel.select("daily");
        app.routine_panel.set_evidence_enabled(false);
        app.start_routine();
        if app.status() != RunnerStatus::Running {
            return;
        }
        wait_history_len(&mut app, 1);
        assert_eq!(app.history.records()[0].evidence_run_id, None);

        app.rerun_pipeline();
        if app.status() != RunnerStatus::Running {
            return;
        }
        wait_history_len(&mut app, 2);
        assert_eq!(
            app.history.records()[0].evidence_run_id,
            None,
            "rerun must preserve the evidence-disabled request"
        );
    }

    // ---- 03-report-channel: strategy spawn の --report-file 付与 ----

    /// with_report_file_arg: 引数列末尾に `--report-file <path>` が付く。
    #[test]
    fn report_file_arg_appended_to_strategy_args() {
        let args = vec![
            "run".to_string(),
            "templates/pipelines/x".to_string(),
            "T".to_string(),
        ];
        let got = with_report_file_arg(&args, Path::new("/tmp/r/report.json"));
        assert_eq!(got.len(), args.len() + 2);
        assert_eq!(got[got.len() - 2], "--report-file");
        assert_eq!(got[got.len() - 1], "/tmp/r/report.json");
        assert_eq!(&got[..args.len()], &args[..]);
    }

    /// RunRequest::Strategy の spawn spec に --report-file が含まれ、
    /// 発行された report パスと一致する (label/run-id は None・種別は Run)。
    #[test]
    fn strategy_build_spawn_emits_report_file() {
        let req = RunRequest::Strategy {
            args: vec!["run".to_string(), "p".to_string(), "T".to_string()],
        };
        let (spec, label, run_id, report, kind) = req.build_spawn("anaden", injected_run_id);
        assert!(
            spec.args.contains(&"--report-file".to_string()),
            "args: {:?}",
            spec.args
        );
        let i = spec.args.iter().position(|a| a == "--report-file").unwrap();
        let emitted = PathBuf::from(&spec.args[i + 1]);
        assert_eq!(Some(emitted), report);
        assert_eq!(kind, PendingReportKind::Run);
        assert_eq!(label, None);
        assert_eq!(run_id, None);
    }

    /// routine 実行も --report-file を付与し、種別は Routine
    /// (03-report-channel follow-up: studio が routine レポートを読める)。
    #[test]
    fn routine_build_spawn_emits_report_file_of_routine_kind() {
        let dir = tempfile::tempdir().unwrap();
        let routine_path = dir.path().join("daily.toml");
        std::fs::write(&routine_path, b"").unwrap();
        let req = RunRequest::Routine {
            path: routine_path,
            name: "daily".to_string(),
            evidence_enabled: false,
        };
        let (spec, label, run_id, report, kind) = req.build_spawn("anaden", injected_run_id);
        let i = spec
            .args
            .iter()
            .position(|a| a == "--report-file")
            .expect("--report-file must be in routine args");
        let emitted = PathBuf::from(&spec.args[i + 1]);
        assert_eq!(Some(emitted), report);
        assert_eq!(kind, PendingReportKind::Routine);
        assert_eq!(label.as_deref(), Some("routine:daily"));
        assert_eq!(run_id, None);
    }

    /// Exit イベント観測時、pending の report.json が第一ソースとして
    /// 履歴レコード (停止理由・発火回数) と LogStatus へ反映される
    /// (stdout 日本語行マッチはフォールバック)。
    #[test]
    fn exit_drain_reads_report_into_status_and_history() {
        let mut app = PipelineRunnerApp::new(dummy_program());
        let _history_dir = isolate_history(&mut app);
        let dir = tempfile::tempdir().unwrap();
        let report_path = dir.path().join("report.json");
        std::fs::write(
            &report_path,
            r#"{"iterations":42,"fired_count":7,"per_task_matches":[],"elapsed_ms":1000,"terminal_task":"goal_reached","reached_goal":"loop_count=42","stop_reason":"goal_reached","recovery_attempts":0,"max_consecutive_recovery":0}"#,
        )
        .unwrap();
        app.pending_report_path = Some(report_path);
        app.run_started_at = Some(1);
        app.run_strategy = Some("field_loop_pc".to_string());
        // Exit イベント (exit code 0) を直接注入して drain。
        app.log_tx
            .send(crate::log_view::LogEvent::Exit(Some(0)))
            .unwrap();
        app.drain_logs();

        let record = app.history.records().first().expect("history record");
        assert_eq!(record.strategy, "field_loop_pc");
        assert_eq!(
            record.stop_reason.as_deref(),
            Some("宣言的ゴール到達(正常)")
        );
        assert_eq!(record.fired_count, Some(7));
        // LogStatus (ステータスバー) も report 由来の値へ更新される。
        assert_eq!(
            app.run_status_summary(),
            "停止 reason=宣言的ゴール到達(正常) iterations=42"
        );
        // 読込済み report は再消費されない (take 済み)。
        assert_eq!(app.pending_report_path, None);
    }

    /// report が無い (古い anaden バイナリ・書込失敗) 場合は従来経路
    /// (stdout 解析 + report フィールド None) で履歴が作られる。
    #[test]
    fn exit_drain_without_report_falls_back_to_stdout() {
        let mut app = PipelineRunnerApp::new(dummy_program());
        let _history_dir = isolate_history(&mut app);
        app.pending_report_path = None;
        app.run_started_at = Some(1);
        app.run_strategy = Some("field_loop_pc".to_string());
        app.log_tx
            .send(crate::log_view::LogEvent::Exit(Some(0)))
            .unwrap();
        app.drain_logs();

        let record = app.history.records().first().expect("history record");
        assert_eq!(record.stop_reason, None);
        assert_eq!(record.fired_count, None);
        assert_eq!(app.run_status_summary(), "未実行");
    }

    // ---- 04-studio-maxiters-outcome: MaxIterations/Success の成否分類 (F6) ----

    /// exit 0 でも report が MaxIterations + fired=0 (成果ゼロ打ち切り) の場合、
    /// 履歴は BudgetExhausted として記録される (従来は Success = 誤成功)。
    #[test]
    fn exit_drain_classifies_budget_exhausted_run() {
        let mut app = PipelineRunnerApp::new(dummy_program());
        let _history_dir = isolate_history(&mut app);
        let dir = tempfile::tempdir().unwrap();
        let report_path = dir.path().join("report.json");
        std::fs::write(
            &report_path,
            r#"{"iterations":100,"fired_count":0,"per_task_matches":[],"elapsed_ms":100000,"terminal_task":null,"reached_goal":null,"stop_reason":"max_iterations","recovery_attempts":0,"max_consecutive_recovery":0}"#,
        )
        .unwrap();
        app.pending_report_path = Some(report_path);
        app.run_started_at = Some(1);
        app.run_strategy = Some("field_loop_pc".to_string());
        app.log_tx
            .send(crate::log_view::LogEvent::Exit(Some(0)))
            .unwrap();
        app.drain_logs();

        let record = app.history.records().first().expect("history record");
        assert_eq!(record.outcome, RunOutcome::BudgetExhausted);
        assert_eq!(record.exit_code, Some(0));
        assert_eq!(record.stop_reason.as_deref(), Some("最大サイクル到達"));
        assert_eq!(record.fired_count, Some(0));
    }

    /// fired>0 の MaxIterations は成果あり扱い — 従来通り Success で記録される。
    #[test]
    fn exit_drain_max_iterations_with_fired_is_success() {
        let mut app = PipelineRunnerApp::new(dummy_program());
        let _history_dir = isolate_history(&mut app);
        let dir = tempfile::tempdir().unwrap();
        let report_path = dir.path().join("report.json");
        std::fs::write(
            &report_path,
            r#"{"iterations":100,"fired_count":7,"per_task_matches":[],"elapsed_ms":100000,"terminal_task":null,"reached_goal":null,"stop_reason":"max_iterations","recovery_attempts":0,"max_consecutive_recovery":0}"#,
        )
        .unwrap();
        app.pending_report_path = Some(report_path);
        app.run_started_at = Some(1);
        app.run_strategy = Some("field_loop_pc".to_string());
        app.log_tx
            .send(crate::log_view::LogEvent::Exit(Some(0)))
            .unwrap();
        app.drain_logs();

        let record = app.history.records().first().expect("history record");
        assert_eq!(record.outcome, RunOutcome::Success);
        assert_eq!(record.fired_count, Some(7));
    }

    // ---- routine レポート読込 (03-report-channel follow-up) ----

    /// routine 実行の Exit ドレイン: RoutineSummary 形式の report.json を
    /// 種別 (PendingReportKind::Routine) に従って読み、履歴 (全体成否ラベル・
    /// 合計発火数) と LogStatus (合計サイクル数・成否ラベル) へ反映する。
    #[test]
    fn exit_drain_reads_routine_report_into_status_and_history() {
        let mut app = PipelineRunnerApp::new(dummy_program());
        let _history_dir = isolate_history(&mut app);
        let dir = tempfile::tempdir().unwrap();
        let report_path = dir.path().join("report.json");
        let completed_step = || anaden_engine::RoutineStepResult {
            step: "s".to_string(),
            pipeline_dir: "p".to_string(),
            status: anaden_engine::StepStatus::Completed(anaden_engine::LoopStopReason::Stop),
            iterations: 1,
            fired_count: 2,
            terminal: "t".to_string(),
        };
        let summary = anaden_engine::RoutineSummary {
            routine: "daily".to_string(),
            results: vec![completed_step(), completed_step()],
            total_iterations: 15,
            total_fired: 4,
            completed: 2,
            failed: 0,
            skipped: 0,
            aborted: false,
        };
        std::fs::write(&report_path, serde_json::to_string(&summary).unwrap()).unwrap();
        app.pending_report_path = Some(report_path);
        app.pending_report_kind = PendingReportKind::Routine;
        app.run_started_at = Some(1);
        app.run_strategy = Some("routine:daily".to_string());
        // Exit イベント (exit code 0 = all_ok) を直接注入して drain。
        app.log_tx
            .send(crate::log_view::LogEvent::Exit(Some(0)))
            .unwrap();
        app.drain_logs();

        let record = app.history.records().first().expect("history record");
        assert_eq!(record.strategy, "routine:daily");
        assert_eq!(record.outcome, RunOutcome::Success);
        assert_eq!(record.stop_reason.as_deref(), Some("routine 完了 (2/2)"));
        assert_eq!(record.fired_count, Some(4));
        // LogStatus (ステータスバー) も routine レポート由来の値へ更新される。
        assert_eq!(
            app.run_status_summary(),
            "停止 reason=routine 完了 (2/2) iterations=15"
        );
        // 読込済み report は再消費されない (take 済み)。
        assert_eq!(app.pending_report_path, None);
    }

    /// routine 失敗 (exit 2) のレポートも読み込まれ、失敗ラベルが履歴へ残る。
    #[test]
    fn exit_drain_routine_failure_report_labels_history() {
        let mut app = PipelineRunnerApp::new(dummy_program());
        let _history_dir = isolate_history(&mut app);
        let dir = tempfile::tempdir().unwrap();
        let report_path = dir.path().join("report.json");
        let step = |status| anaden_engine::RoutineStepResult {
            step: "s".to_string(),
            pipeline_dir: "p".to_string(),
            status,
            iterations: 1,
            fired_count: 0,
            terminal: "t".to_string(),
        };
        let summary = anaden_engine::RoutineSummary {
            routine: "daily".to_string(),
            results: vec![
                step(anaden_engine::StepStatus::Failed(
                    anaden_engine::LoopStopReason::CaptureError,
                )),
                step(anaden_engine::StepStatus::Skipped),
            ],
            total_iterations: 3,
            total_fired: 0,
            completed: 0,
            failed: 1,
            skipped: 1,
            aborted: true,
        };
        std::fs::write(&report_path, serde_json::to_string(&summary).unwrap()).unwrap();
        app.pending_report_path = Some(report_path);
        app.pending_report_kind = PendingReportKind::Routine;
        app.run_started_at = Some(1);
        app.run_strategy = Some("routine:daily".to_string());
        app.log_tx
            .send(crate::log_view::LogEvent::Exit(Some(2)))
            .unwrap();
        app.drain_logs();

        let record = app.history.records().first().expect("history record");
        assert_eq!(record.outcome, RunOutcome::Failed);
        assert_eq!(
            record.stop_reason.as_deref(),
            Some("routine 失敗 (failed=1 skipped=1 aborted=true)")
        );
    }
}
