#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoScrollFollow {
    enabled: bool,
    pending: usize,
}

impl Default for AutoScrollFollow {
    fn default() -> Self {
        Self {
            enabled: true,
            pending: 0,
        }
    }
}

impl AutoScrollFollow {
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.pending = 0;
        }
    }

    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    #[must_use]
    pub fn should_stick_to_bottom(&self) -> bool {
        self.enabled
    }

    pub fn observe_new_lines(&mut self, n: usize) {
        if self.enabled {
            self.pending = self.pending.saturating_add(n);
        }
    }

    pub fn on_scrolled_to_bottom(&mut self) {
        self.pending = 0;
    }

    #[must_use]
    pub fn pending_lines(&self) -> usize {
        self.pending
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RunStatus {
    pub running: bool,
    pub goal: Option<String>,
    pub iterations: Option<u64>,
    pub stop_reason: Option<String>,
}

impl RunStatus {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn observe(&mut self, line: &str) {
        if line.contains("run_loop 開始") {
            self.running = true;
            self.iterations = None;
            self.stop_reason = None;
            self.goal = line.split("goal=").nth(1).and_then(|rest| {
                let g = rest.trim();
                (g != "(none)").then(|| g.to_string())
            });
        } else if let Some((_, rest)) = line.split_once("サイクル数:") {
            self.iterations = rest.trim().parse::<u64>().ok();
        } else if let Some((_, rest)) = line.split_once("停止理由:") {
            self.stop_reason = Some(rest.trim().to_string());
            self.running = false;
        }
    }

    #[allow(dead_code)]
    pub fn summary(&self) -> String {
        if self.running {
            format!(
                "実行中 goal={} iterations={}",
                self.goal.as_deref().unwrap_or("(none)"),
                self.iterations
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "?".into())
            )
        } else if let Some(reason) = &self.stop_reason {
            format!(
                "停止 reason={} iterations={}",
                reason,
                self.iterations
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "?".into())
            )
        } else {
            "未実行".to_string()
        }
    }

    pub fn apply_report(&mut self, report: &anaden_engine::ProgressReport) {
        self.iterations = Some(report.iterations);
        if let Some(reason) = report.stop_reason.clone() {
            self.stop_reason = Some(anaden_engine::stop_reason_label(reason).to_string());
        }
        self.running = false;
    }

    pub fn apply_routine_report(&mut self, report: &anaden_engine::RoutineSummary) {
        self.iterations = Some(report.total_iterations);
        self.stop_reason = Some(report.outcome_label());
        self.running = false;
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    // ---- RunStatus ----

    #[test]
    fn status_start_line_sets_running_and_goal() {
        let mut s = RunStatus::new();
        s.observe("INFO anaden_cli: run_loop 開始: interval=2s max_iters=10 goal=farm50");
        assert!(s.running);
        assert_eq!(s.goal.as_deref(), Some("farm50"));
        assert_eq!(s.iterations, None);
        assert_eq!(s.stop_reason, None);
    }

    #[test]
    fn status_start_line_without_goal_is_none() {
        let mut s = RunStatus::new();
        s.observe("INFO anaden_cli: run_loop 開始: interval=2s max_iters=10 goal=(none)");
        assert!(s.running);
        assert_eq!(s.goal, None);
    }

    #[test]
    fn status_result_lines_set_iterations_and_stop() {
        let mut s = RunStatus::new();
        s.observe("run_loop 開始: interval=2s max_iters=10 goal=g1");
        s.observe("サイクル数: 42");
        s.observe("停止理由:   宣言的ゴール到達(正常)");
        assert!(!s.running);
        assert_eq!(s.iterations, Some(42));
        assert_eq!(s.stop_reason.as_deref(), Some("宣言的ゴール到達(正常)"));
    }

    #[test]
    fn status_summary_varies_by_phase() {
        let mut s = RunStatus::new();
        assert_eq!(s.summary(), "未実行");
        s.observe("run_loop 開始: interval=2s goal=g1");
        assert_eq!(s.summary(), "実行中 goal=g1 iterations=?");
        s.observe("サイクル数: 3");
        s.observe("停止理由: 最大サイクル到達");
        assert_eq!(s.summary(), "停止 reason=最大サイクル到達 iterations=3");
    }

    #[test]
    fn status_restart_resets_previous_result() {
        let mut s = RunStatus::new();
        s.observe("サイクル数: 5");
        s.observe("停止理由: 最大サイクル到達");
        s.observe("run_loop 開始: interval=2s goal=g2");
        assert!(s.running);
        assert_eq!(s.iterations, None);
        assert_eq!(s.stop_reason, None);
        assert_eq!(s.goal.as_deref(), Some("g2"));
    }

    // ---- apply_report (03-report-channel: report を第一ソース化) ----

    /// report.json fixture から終端状態 (サイクル数・停止理由) が構築できる。
    /// stdout の開始行由来の goal は report に無い情報のためフォールバック保持。
    #[test]
    fn status_builds_from_report_fixture() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("report.json");
        std::fs::write(
            &path,
            r#"{"iterations":42,"fired_count":7,"per_task_matches":[],"elapsed_ms":1000,"terminal_task":"goal_reached","reached_goal":"loop_count=42","stop_reason":"goal_reached","recovery_attempts":0,"max_consecutive_recovery":0}"#,
        )
        .expect("write fixture");
        let report: anaden_engine::ProgressReport =
            serde_json::from_reader(std::fs::File::open(&path).expect("open")).expect("parse");

        let mut s = RunStatus::new();
        s.observe("INFO anaden_cli: run_loop 開始: interval=2s max_iters=10 goal=farm50");
        s.apply_report(&report);

        assert!(!s.running, "report は終端成果物 = 実行中でない");
        assert_eq!(s.iterations, Some(42));
        assert_eq!(s.stop_reason.as_deref(), Some("宣言的ゴール到達(正常)"));
        assert_eq!(s.goal.as_deref(), Some("farm50"));
    }

    /// report の値は stdout 行解析の結果を上書きする (report が第一ソース)。
    #[test]
    fn status_report_overrides_stdout_parsed_values() {
        let report = anaden_engine::ProgressReport {
            jitter_seed: None,
            verify: Default::default(),
            iterations: 9,
            fired_count: 2,
            per_task_matches: vec![],
            elapsed_ms: 10,
            terminal_task: Some("max_iterations".to_string()),
            reached_goal: None,
            stop_reason: Some(anaden_engine::LoopStopReason::MaxIterations),
            recovery_attempts: 0,
            max_consecutive_recovery: 0,
        };
        let mut s = RunStatus::new();
        s.observe("run_loop 開始: interval=1s goal=g");
        // stdout 解析で誤った値が入った状態をシミュレート。
        s.observe("サイクル数: 999");
        s.observe("停止理由:   宣言的ゴール到達(正常)");
        s.apply_report(&report);
        assert_eq!(s.iterations, Some(9), "report のサイクル数が優先");
        assert_eq!(s.stop_reason.as_deref(), Some("最大サイクル到達"));
        assert!(!s.running);
    }

    /// 旧成果物由来 (stop_reason 無し) の report は stdout 解析結果を維持。
    #[test]
    fn status_report_without_stop_reason_keeps_stdout_value() {
        let mut s = RunStatus::new();
        s.observe("run_loop 開始: goal=g");
        s.observe("停止理由:   最大サイクル到達");
        s.observe("サイクル数: 3");
        let legacy = anaden_engine::ProgressReport {
            iterations: 3,
            ..anaden_engine::ProgressReport::default()
        };
        s.apply_report(&legacy);
        assert_eq!(s.stop_reason.as_deref(), Some("最大サイクル到達"));
        assert_eq!(s.iterations, Some(3));
    }

    /// routine レポート (RoutineSummary) も終端状態を上書きする:
    /// iterations = 全ステップ合計・停止理由 = 全体成否ラベル (routine 版契約)。
    #[test]
    fn status_routine_report_overrides_with_totals_and_outcome_label() {
        let step = || anaden_engine::RoutineStepResult {
            step: "s".to_string(),
            pipeline_dir: "p".to_string(),
            status: anaden_engine::StepStatus::Completed(anaden_engine::LoopStopReason::Stop),
            iterations: 1,
            fired_count: 2,
            terminal: "t".to_string(),
        };
        let report = anaden_engine::RoutineSummary {
            routine: "daily".to_string(),
            results: vec![step(), step()],
            total_iterations: 15,
            total_fired: 4,
            completed: 2,
            failed: 0,
            skipped: 0,
            aborted: false,
        };
        let mut s = RunStatus::new();
        s.observe("run_loop 開始: goal=g");
        s.observe("サイクル数: 999");
        s.apply_routine_report(&report);
        assert!(!s.running);
        assert_eq!(s.iterations, Some(15));
        assert_eq!(s.stop_reason.as_deref(), Some("routine 完了 (2/2)"));
    }

    // ---- AutoScrollFollow (T4: 自動スクロール追従の純ロジック) ----

    #[test]
    fn follow_defaults_enabled_with_no_pending() {
        let f = AutoScrollFollow::default();
        assert!(f.should_stick_to_bottom());
        assert_eq!(f.pending_lines(), 0);
    }

    #[test]
    fn follow_accumulates_pending_new_lines_while_enabled() {
        let mut f = AutoScrollFollow::default();
        f.observe_new_lines(3);
        assert_eq!(f.pending_lines(), 3);
        f.observe_new_lines(2);
        assert_eq!(f.pending_lines(), 5);
    }

    #[test]
    fn follow_scrolled_to_bottom_clears_pending() {
        let mut f = AutoScrollFollow::default();
        f.observe_new_lines(4);
        f.on_scrolled_to_bottom();
        assert_eq!(f.pending_lines(), 0);
        assert!(f.should_stick_to_bottom());
    }

    #[test]
    fn follow_disabled_stops_sticking_and_ignores_new_lines() {
        let mut f = AutoScrollFollow::default();
        f.set_enabled(false);
        assert!(!f.should_stick_to_bottom());
        f.observe_new_lines(10);
        assert_eq!(f.pending_lines(), 0);
    }

    #[test]
    fn follow_reenable_resumes_stick_without_pending() {
        let mut f = AutoScrollFollow::default();
        f.set_enabled(false);
        f.observe_new_lines(10);
        f.set_enabled(true);
        assert!(f.should_stick_to_bottom());
        assert_eq!(f.pending_lines(), 0);
    }
}
