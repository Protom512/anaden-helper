use std::io;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

const HISTORY_FILE_NAME: &str = "history.jsonl";
const HISTORY_ENV_VAR: &str = "ANADEN_STUDIO_HISTORY";
pub const DEFAULT_MAX_RECORDS: usize = 50;
pub const LOG_TAIL_MAX_LINES: usize = 20;

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunOutcome {
    #[default]
    Success,
    Failed,
    Cancelled,
    BudgetExhausted,
    #[serde(other)]
    Unknown,
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

#[must_use]
pub fn load_run_report(path: &Path) -> Option<anaden_engine::ProgressReport> {
    std::fs::File::open(path)
        .ok()
        .and_then(|file| serde_json::from_reader(file).ok())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunRecord {
    #[serde(default)]
    pub started_at_unix: u64,
    #[serde(default)]
    pub strategy: String,
    #[serde(default)]
    pub outcome: RunOutcome,
    #[serde(default)]
    pub exit_code: Option<i32>,
    #[serde(default)]
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
    unparsed: Vec<String>,
}

impl RunHistory {
    pub fn open(path: impl Into<PathBuf>, max_records: usize) -> Self {
        let path = path.into();
        let (mut records, unparsed) = Self::read_file(&path);
        truncate_oldest(&mut records, max_records);
        Self {
            path,
            max_records,
            records,
            unparsed,
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
        self.persist()?;
        self.prune_report_files();
        Ok(())
    }

    fn prune_report_files(&self) {
        let Some(dir) = self.path.parent() else {
            return;
        };
        let entries = match std::fs::read_dir(dir.join("reports")) {
            Ok(entries) => entries,
            Err(_) => return,
        };
        let files: Vec<(u64, PathBuf)> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "json"))
            .map(|p| (report_mtime_rank(&p), p))
            .collect();
        for path in report_paths_to_prune(files, self.max_records) {
            let _ = std::fs::remove_file(path);
        }
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
            let line = serde_json::to_string(r).map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "record failed to serialize; history file left unwritten",
                )
            })?;
            buf.push_str(&line);
            buf.push('\n');
        }
        for raw in &self.unparsed {
            buf.push_str(raw);
            buf.push('\n');
        }
        std::fs::write(&self.path, buf)
    }

    fn read_file(path: &std::path::Path) -> (Vec<RunRecord>, Vec<String>) {
        let Ok(content) = std::fs::read_to_string(path) else {
            return (Vec::new(), Vec::new());
        };
        let mut records = Vec::new();
        let mut unparsed = Vec::new();
        for line in content.lines().filter(|l| !l.trim().is_empty()) {
            match serde_json::from_str::<RunRecord>(line) {
                Ok(r) => records.push(r),
                Err(_) => unparsed.push(line.to_string()),
            }
        }
        (records, unparsed)
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

#[must_use]
fn report_mtime_rank(path: &Path) -> u64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(u64::MAX)
}

#[must_use]
fn report_paths_to_prune(mut files: Vec<(u64, PathBuf)>, keep: usize) -> Vec<PathBuf> {
    files.sort_by(|a, b| b.cmp(a));
    files.into_iter().skip(keep).map(|(_, p)| p).collect()
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
        let orig = std::env::var(HISTORY_ENV_VAR).ok();
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

    #[test]
    fn record_without_evidence_omits_field_in_json() {
        let json = serde_json::to_string(&sample_record("fishing", 1)).unwrap();
        assert!(!json.contains("evidence_run_id"), "{json}");
    }

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

    #[test]
    fn legacy_record_without_field_deserializes_to_none() {
        let legacy = r#"{"started_at_unix":1,"strategy":"fishing","outcome":"Success","exit_code":0,"log_tail":["l"]}"#;
        let r: RunRecord = serde_json::from_str(legacy).unwrap();
        assert_eq!(r.evidence_run_id, None);
        assert_eq!(r.strategy, "fishing");
    }

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

    #[test]
    fn record_without_report_omits_fields_in_json() {
        let json = serde_json::to_string(&sample_record("fishing", 1)).unwrap();
        assert!(!json.contains("stop_reason"), "{json}");
        assert!(!json.contains("fired_count"), "{json}");
    }

    #[test]
    fn legacy_record_without_report_fields_deserializes_to_none() {
        let legacy = r#"{"started_at_unix":1,"strategy":"fishing","outcome":"Success","exit_code":0,"log_tail":["l"]}"#;
        let r: RunRecord = serde_json::from_str(legacy).unwrap();
        assert_eq!(r.stop_reason, None);
        assert_eq!(r.fired_count, None);
    }

    #[test]
    fn new_report_path_is_under_reports_dir_with_json_ext() {
        let p = new_report_path();
        assert_eq!(p.extension().and_then(|e| e.to_str()), Some("json"));
        assert!(
            p.to_string_lossy().contains("reports"),
            "unexpected path: {}",
            p.display()
        );
        assert_ne!(new_report_path(), p);
    }

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

    #[test]
    fn classify_outcome_max_iterations_zero_fired_is_budget_exhausted() {
        let r = report(Some(anaden_engine::LoopStopReason::MaxIterations), 0);
        assert_eq!(
            classify_outcome(Some(0), Some(&r)),
            RunOutcome::BudgetExhausted
        );
    }

    #[test]
    fn classify_outcome_max_iterations_with_fired_is_success() {
        let r = report(Some(anaden_engine::LoopStopReason::MaxIterations), 7);
        assert_eq!(classify_outcome(Some(0), Some(&r)), RunOutcome::Success);
    }

    #[test]
    fn classify_outcome_nonzero_exit_is_failed_even_with_report() {
        let r = report(Some(anaden_engine::LoopStopReason::MaxIterations), 7);
        assert_eq!(classify_outcome(Some(1), Some(&r)), RunOutcome::Failed);
        assert_eq!(classify_outcome(None, Some(&r)), RunOutcome::Failed);
    }

    #[test]
    fn classify_outcome_no_report_exit_zero_is_success() {
        assert_eq!(classify_outcome(Some(0), None), RunOutcome::Success);
    }

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

    #[test]
    fn budget_exhausted_outcome_serializes_as_distinct_label() {
        let json = serde_json::to_string(&RunOutcome::BudgetExhausted).unwrap();
        assert_eq!(json, "\"BudgetExhausted\"");
        let back: RunOutcome = serde_json::from_str(&json).unwrap();
        assert_eq!(back, RunOutcome::BudgetExhausted);
    }

    #[test]
    fn max_records_limit_drops_oldest() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.jsonl");
        let mut h = RunHistory::open(&path, 3);
        for i in 0..5u64 {
            h.append(sample_record("s", i)).unwrap();
        }
        assert_eq!(h.records().len(), 3);
        assert_eq!(
            h.records()
                .iter()
                .map(|r| r.started_at_unix)
                .collect::<Vec<_>>(),
            vec![4, 3, 2]
        );
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

    #[test]
    fn report_paths_to_prune_keeps_newest_and_never_drops_unknown_mtime() {
        let mk = PathBuf::from;
        let files = vec![
            (1_000, mk("1000.json")),
            (3_000, mk("3000.json")),
            (2_000, mk("2000.json")),
        ];
        assert_eq!(
            report_paths_to_prune(files, 2),
            vec![PathBuf::from("1000.json")]
        );
        let files = vec![(u64::MAX, mk("unknown.json")), (1_000, mk("1000.json"))];
        assert_eq!(
            report_paths_to_prune(files, 1),
            vec![PathBuf::from("1000.json")]
        );
    }

    #[test]
    fn append_prunes_report_files_beyond_history_cap() {
        let dir = tempfile::tempdir().unwrap();
        let reports = dir.path().join("reports");
        std::fs::create_dir_all(&reports).unwrap();
        for name in ["1.json", "2.json", "3.json", "4.json", "5.json"] {
            std::fs::write(reports.join(name), b"{}").unwrap();
        }
        let mut h = RunHistory::open(dir.path().join("history.jsonl"), 2);
        h.append(sample_record("s", 1)).unwrap();
        let mut remaining: Vec<String> = std::fs::read_dir(&reports)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        remaining.sort();
        assert_eq!(remaining, vec!["4.json", "5.json"]);
    }
}
