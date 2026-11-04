use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

const HISTORY_FILE_NAME: &str = "history.jsonl";
const HISTORY_ENV_VAR: &str = "ANADEN_STUDIO_HISTORY";
pub const DEFAULT_MAX_RECORDS: usize = 50;
pub const LOG_TAIL_MAX_LINES: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunOutcome {
    Success,
    Failed,
    Cancelled,
    BudgetExhausted,
}

#[must_use]
pub fn classify_outcome(
    exit_code: Option<i32>,
    report: Option<&anaden_engine::ProgressReport>,
) -> RunOutcome {
    if !matches!(exit_code, Some(0)) {
        return RunOutcome::Failed;
    }
    if let Some(r) = report
        && r.stop_reason == Some(anaden_engine::LoopStopReason::MaxIterations)
        && r.fired_count == 0
    {
        return RunOutcome::BudgetExhausted;
    }
    RunOutcome::Success
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunRecord {
    pub started_at_unix: u64,
    pub strategy: String,
    pub outcome: RunOutcome,
    pub exit_code: Option<i32>,
    pub log_tail: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fired_count: Option<u64>,
}

impl RunRecord {
    pub fn new(
        started_at_unix: u64,
        strategy: impl Into<String>,
        outcome: RunOutcome,
        exit_code: Option<i32>,
        log_tail: Vec<String>,
    ) -> Self {
        Self {
            started_at_unix,
            strategy: strategy.into(),
            outcome,
            exit_code,
            log_tail: truncate_tail(log_tail),
            evidence_run_id: None,
            stop_reason: None,
            fired_count: None,
        }
    }

    #[must_use]
    pub fn with_evidence_run_id(mut self, run_id: Option<String>) -> Self {
        self.evidence_run_id = run_id;
        self
    }

    #[must_use]
    pub fn with_report_summary(
        mut self,
        stop_reason: Option<String>,
        fired_count: Option<u64>,
    ) -> Self {
        self.stop_reason = stop_reason;
        self.fired_count = fired_count;
        self
    }
}

fn truncate_tail(mut lines: Vec<String>) -> Vec<String> {
    if lines.len() > LOG_TAIL_MAX_LINES {
        let start = lines.len() - LOG_TAIL_MAX_LINES;
        lines.drain(..start);
    }
    lines
}

#[derive(Debug)]
pub struct RunHistory {
    path: PathBuf,
    max_records: usize,
    records: Vec<RunRecord>,
}

impl RunHistory {
    pub fn open(path: impl Into<PathBuf>, max_records: usize) -> Self {
        let path = path.into();
        let mut records = Self::read_file(&path);
        truncate_oldest(&mut records, max_records);
        Self {
            path,
            max_records,
            records,
        }
    }

    pub fn open_default() -> Self {
        Self::open(history_path(), DEFAULT_MAX_RECORDS)
    }

    pub fn history_path() -> PathBuf {
        history_path()
    }

    pub fn append(&mut self, record: RunRecord) -> io::Result<()> {
        self.records.insert(0, record);
        truncate_oldest(&mut self.records, self.max_records);
        self.persist()
    }

    pub fn records(&self) -> &[RunRecord] {
        &self.records
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    fn persist(&self) -> io::Result<()> {
        if let Some(parent) = self.path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }

        let mut buf = String::new();
        for r in &self.records {
            match serde_json::to_string(r) {
                Ok(line) => {
                    buf.push_str(&line);
                    buf.push('\n');
                }
                Err(_) => continue,
            }
        }
        std::fs::write(&self.path, buf)
    }

    fn read_file(path: &std::path::Path) -> Vec<RunRecord> {
        let Ok(content) = std::fs::read_to_string(path) else {
            return Vec::new();
        };
        content
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }
}

pub fn history_path() -> PathBuf {
    if let Ok(explicit) = std::env::var(HISTORY_ENV_VAR)
        && !explicit.trim().is_empty()
    {
        return PathBuf::from(explicit);
    }
    config_dir().join("anaden-studio").join(HISTORY_FILE_NAME)
}

#[must_use]
pub fn new_report_path() -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let base = history_path()
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("reports").join(format!("{nanos}.json"))
}

fn config_dir() -> PathBuf {
    if let Ok(appdata) = std::env::var("APPDATA")
        && !appdata.trim().is_empty()
    {
        return PathBuf::from(appdata);
    }
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME")
        && !xdg.trim().is_empty()
    {
        return PathBuf::from(xdg);
    }
    if let Ok(home) = std::env::var("HOME")
        && !home.trim().is_empty()
    {
        return PathBuf::from(home).join(".config");
    }
    PathBuf::from(".")
}

fn truncate_oldest(records: &mut Vec<RunRecord>, max_records: usize) {
    if records.len() > max_records {
        records.truncate(max_records);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    fn sample_record(strategy: &str, started: u64) -> RunRecord {
        RunRecord::new(
            started,
            strategy,
            RunOutcome::Success,
            Some(0),
            vec!["line1".to_string(), "line2".to_string()],
        )
    }

    // --- 正常系 ---

    #[test]
    fn append_persists_and_reload_returns_record() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.jsonl");
        {
            let mut h = RunHistory::open(&path, DEFAULT_MAX_RECORDS);
            h.append(sample_record("fishing", 1000)).unwrap();
            h.append(RunRecord::new(
                2000,
                "main_story",
                RunOutcome::Failed,
                Some(1),
                vec![],
            ))
            .unwrap();
        }
        let h = RunHistory::open(&path, DEFAULT_MAX_RECORDS);
        let rs = h.records();
        assert_eq!(rs.len(), 2);
        // 新しい順
        assert_eq!(rs[0].strategy, "main_story");
        assert_eq!(rs[0].outcome, RunOutcome::Failed);
        assert_eq!(rs[0].exit_code, Some(1));
        assert_eq!(rs[1].strategy, "fishing");
        assert_eq!(
            rs[1].log_tail,
            vec!["line1".to_string(), "line2".to_string()]
        );
    }

    #[test]
    fn history_path_respects_env_override() {
        // env-var テストは並列実行と競合しうるため一時的に排他は諦め、
        // 値を設定して即検証・復元する (他テストは open(path) 直指定で環境非依存)。
        let orig = std::env::var(HISTORY_ENV_VAR).ok();
        // safety: テストバイナリ内で他スレッドが環境変数を読み書きしていない
        // (当テストモジュールの他テストは環境非依存)。単一テスト内で完結。
        unsafe {
            std::env::set_var(HISTORY_ENV_VAR, "/tmp/custom-history.jsonl");
        }
        let p = history_path();
        unsafe {
            std::env::remove_var(HISTORY_ENV_VAR);
        }
        if let Some(o) = orig {
            unsafe {
                std::env::set_var(HISTORY_ENV_VAR, o);
            }
        }
        assert_eq!(p, PathBuf::from("/tmp/custom-history.jsonl"));
    }

    #[test]
    fn record_serializes_to_jsonl_line() {
        let r = sample_record("fishing", 42);
        let json = serde_json::to_string(&r).unwrap();
        let back: RunRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(back, r);
    }

    // --- evidence_run_id (Issue #202 UC-1) ---

    /// 未採取 (None) の record は evidence_run_id を JSON に出力しない
    /// (通常の pipeline 実行履歴の互換性: フィールド追加前と同一 JSON)。
    #[test]
    fn record_without_evidence_omits_field_in_json() {
        let json = serde_json::to_string(&sample_record("fishing", 1)).unwrap();
        assert!(!json.contains("evidence_run_id"), "{json}");
    }

    /// 付与した record は round-trip で run-id が復元される。
    #[test]
    fn record_with_evidence_round_trips_run_id() {
        let r = sample_record("routine:daily", 2)
            .with_evidence_run_id(Some("routine-20260914-013000".to_string()));
        let json = serde_json::to_string(&r).unwrap();
        assert!(
            json.contains("\"evidence_run_id\":\"routine-20260914-013000\""),
            "{json}"
        );
        let back: RunRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(back, r);
    }

    /// 本フィールド追加前の旧履歴ファイル行 (evidence_run_id なし) は
    /// None で復元される (history.jsonl 後方互換)。
    #[test]
    fn legacy_record_without_field_deserializes_to_none() {
        let legacy = r#"{"started_at_unix":1,"strategy":"fishing","outcome":"Success","exit_code":0,"log_tail":["l"]}"#;
        let r: RunRecord = serde_json::from_str(legacy).unwrap();
        assert_eq!(r.evidence_run_id, None);
        assert_eq!(r.strategy, "fishing");
    }

    // --- report 由来フィールド (03-report-channel) ---

    /// 付与した record は report サマリ (停止理由・発火回数) を round-trip する。
    #[test]
    fn record_with_report_summary_round_trips() {
        let r = sample_record("field_loop_pc", 1)
            .with_report_summary(Some("最大サイクル到達".to_string()), Some(7));
        let json = serde_json::to_string(&r).unwrap();
        assert!(
            json.contains("\"stop_reason\":\"最大サイクル到達\""),
            "{json}"
        );
        assert!(json.contains("\"fired_count\":7"), "{json}");
        let back: RunRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(back, r);
    }

    /// 未付与 (None) の record は report フィールドを JSON に出力しない
    /// (フィールド追加前と同一 JSON = 互換)。
    #[test]
    fn record_without_report_omits_fields_in_json() {
        let json = serde_json::to_string(&sample_record("fishing", 1)).unwrap();
        assert!(!json.contains("stop_reason"), "{json}");
        assert!(!json.contains("fired_count"), "{json}");
    }

    /// 本フィールド追加前の旧履歴ファイル行は None で復元される。
    #[test]
    fn legacy_record_without_report_fields_deserializes_to_none() {
        let legacy = r#"{"started_at_unix":1,"strategy":"fishing","outcome":"Success","exit_code":0,"log_tail":["l"]}"#;
        let r: RunRecord = serde_json::from_str(legacy).unwrap();
        assert_eq!(r.stop_reason, None);
        assert_eq!(r.fired_count, None);
    }

    /// new_report_path は app data dir 配下の reports/ に一意な JSON パスを返す。
    #[test]
    fn new_report_path_is_under_reports_dir_with_json_ext() {
        let p = new_report_path();
        assert_eq!(p.extension().and_then(|e| e.to_str()), Some("json"));
        assert!(
            p.to_string_lossy().contains("reports"),
            "unexpected path: {}",
            p.display()
        );
        // 連続発行で異なるパス (再実行で上書きしない)。
        assert_ne!(new_report_path(), p);
    }

    // --- 成否分類 classify_outcome (04-studio-maxiters-outcome・F6) ---

    /// テスト用の最小 ProgressReport (停止理由・発火回数のみ可変)。
    fn report(
        reason: Option<anaden_engine::LoopStopReason>,
        fired: u64,
    ) -> anaden_engine::ProgressReport {
        anaden_engine::ProgressReport {
            jitter_seed: None,
            verify: Default::default(),
            iterations: 10,
            fired_count: fired,
            per_task_matches: vec![],
            elapsed_ms: 1000,
            terminal_task: None,
            reached_goal: None,
            stop_reason: reason,
            recovery_attempts: 0,
            max_consecutive_recovery: 0,
        }
    }

    /// exit 0 + MaxIterations + fired=0 → BudgetExhausted (誤成功の排除)。
    #[test]
    fn classify_outcome_max_iterations_zero_fired_is_budget_exhausted() {
        let r = report(Some(anaden_engine::LoopStopReason::MaxIterations), 0);
        assert_eq!(
            classify_outcome(Some(0), Some(&r)),
            RunOutcome::BudgetExhausted
        );
    }

    /// exit 0 + MaxIterations + fired>0 → Success (成果を伴う打ち切りは成果あり)。
    #[test]
    fn classify_outcome_max_iterations_with_fired_is_success() {
        let r = report(Some(anaden_engine::LoopStopReason::MaxIterations), 7);
        assert_eq!(classify_outcome(Some(0), Some(&r)), RunOutcome::Success);
    }

    /// exit 非零は report より優先 — MaxIterations 表示でも Failed。
    #[test]
    fn classify_outcome_nonzero_exit_is_failed_even_with_report() {
        let r = report(Some(anaden_engine::LoopStopReason::MaxIterations), 7);
        assert_eq!(classify_outcome(Some(1), Some(&r)), RunOutcome::Failed);
        assert_eq!(classify_outcome(None, Some(&r)), RunOutcome::Failed);
    }

    /// report 無し (古い anaden バイナリ・routine 実行) の exit 0 は従来通り Success。
    #[test]
    fn classify_outcome_no_report_exit_zero_is_success() {
        assert_eq!(classify_outcome(Some(0), None), RunOutcome::Success);
    }

    /// 他の正常停止理由 (GoalReached / Stop / TerminalTask) は Success のまま。
    #[test]
    fn classify_outcome_goal_reached_and_stop_are_success() {
        for reason in [
            anaden_engine::LoopStopReason::GoalReached,
            anaden_engine::LoopStopReason::Stop,
            anaden_engine::LoopStopReason::TerminalTask,
        ] {
            let r = report(Some(reason.clone()), 0);
            assert_eq!(
                classify_outcome(Some(0), Some(&r)),
                RunOutcome::Success,
                "reason: {reason:?}"
            );
        }
    }

    /// BudgetExhausted は独自の serde ラベルで往復する (history.jsonl 互換)。
    #[test]
    fn budget_exhausted_outcome_serializes_as_distinct_label() {
        let json = serde_json::to_string(&RunOutcome::BudgetExhausted).unwrap();
        assert_eq!(json, "\"BudgetExhausted\"");
        let back: RunOutcome = serde_json::from_str(&json).unwrap();
        assert_eq!(back, RunOutcome::BudgetExhausted);
    }

    // --- エッジケース ---

    #[test]
    fn max_records_limit_drops_oldest() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.jsonl");
        let mut h = RunHistory::open(&path, 3);
        for i in 0..5u64 {
            h.append(sample_record("s", i)).unwrap();
        }
        assert_eq!(h.records().len(), 3);
        // 最新 3 件 (4, 3, 2) が残る
        assert_eq!(
            h.records()
                .iter()
                .map(|r| r.started_at_unix)
                .collect::<Vec<_>>(),
            vec![4, 3, 2]
        );
        // 再読み込み後も件数維持
        let h2 = RunHistory::open(&path, 3);
        assert_eq!(h2.records().len(), 3);
    }

    #[test]
    fn log_tail_truncated_to_limit() {
        let lines: Vec<String> = (0..100).map(|i| format!("l{i}")).collect();
        let r = RunRecord::new(1, "s", RunOutcome::Cancelled, None, lines.clone());
        assert_eq!(r.log_tail.len(), LOG_TAIL_MAX_LINES);
        assert_eq!(r.log_tail.first().unwrap(), "l80");
        assert_eq!(r.log_tail.last().unwrap(), "l99");
    }

    #[test]
    fn corrupt_lines_are_skipped_on_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.jsonl");
        let good = serde_json::to_string(&sample_record("ok", 1)).unwrap();
        let content = format!("{{broken json\n{good}\n\nnot-a-record\n");
        std::fs::write(&path, content).unwrap();
        let h = RunHistory::open(&path, DEFAULT_MAX_RECORDS);
        assert_eq!(h.records().len(), 1);
        assert_eq!(h.records()[0].strategy, "ok");
    }

    #[test]
    fn open_missing_file_yields_empty_history() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nonexistent.jsonl");
        let h = RunHistory::open(&path, DEFAULT_MAX_RECORDS);
        assert!(h.records().is_empty());
    }
}
