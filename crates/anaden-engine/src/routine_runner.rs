use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::routine::{OnFailure, RoutineDef, RoutineError, RoutineStep};
use crate::{LoopOutcome, LoopStopReason, resolve_pipeline_dir};

#[async_trait]
pub trait PipelineInvoker: Send + Sync {
    async fn invoke(&self, step: &RoutineStep) -> Result<LoopOutcome, RoutineError>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Completed(LoopStopReason),
    Failed(LoopStopReason),
    NoFire(LoopStopReason),
    Interrupted,
    Skipped,
    InvokerError(String),
}

impl StepStatus {
    #[must_use]
    pub fn is_failure(&self) -> bool {
        matches!(
            self,
            Self::Failed(_) | Self::NoFire(_) | Self::InvokerError(_)
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoutineStepResult {
    pub step: String,
    pub pipeline_dir: String,
    pub status: StepStatus,
    pub iterations: u64,
    pub fired_count: usize,
    pub terminal: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoutineSummary {
    pub routine: String,
    pub results: Vec<RoutineStepResult>,
    pub total_iterations: u64,
    pub total_fired: usize,
    pub completed: usize,
    pub failed: usize,
    pub skipped: usize,
    pub aborted: bool,
}

impl RoutineSummary {
    #[must_use]
    pub fn all_ok(&self) -> bool {
        self.failed == 0 && self.skipped == 0 && !self.aborted
    }

    #[must_use]
    pub fn outcome_label(&self) -> String {
        if self.all_ok() {
            format!("routine 完了 ({}/{})", self.completed, self.results.len())
        } else {
            format!(
                "routine 失敗 (failed={} skipped={} aborted={})",
                self.failed, self.skipped, self.aborted
            )
        }
    }

    #[must_use]
    pub fn format_text(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("=== routine '{}' サマリ ===\n", self.routine));
        for (i, r) in self.results.iter().enumerate() {
            out.push_str(&format!(
                "  [{i}] {} ({}) => {} iterations={} fired={} terminal={}\n",
                r.step,
                r.pipeline_dir,
                step_status_label(&r.status),
                r.iterations,
                r.fired_count,
                if r.terminal.is_empty() {
                    "-"
                } else {
                    &r.terminal
                }
            ));
        }
        out.push_str(&format!(
            "合計: steps={} completed={} failed={} skipped={} iterations={} fired={} aborted={}\n",
            self.results.len(),
            self.completed,
            self.failed,
            self.skipped,
            self.total_iterations,
            self.total_fired,
            self.aborted
        ));
        out
    }
}

#[must_use]
fn classify_reason(reason: &LoopStopReason) -> StepStatus {
    match reason {
        LoopStopReason::Stop
        | LoopStopReason::TerminalTask
        | LoopStopReason::GoalReached
        | LoopStopReason::MaxIterations => StepStatus::Completed(reason.clone()),
        LoopStopReason::CaptureError
        | LoopStopReason::ExecuteError
        | LoopStopReason::GoalTimeout
        | LoopStopReason::RecoveryExhausted => StepStatus::Failed(reason.clone()),
        LoopStopReason::Interrupted => StepStatus::Interrupted,
    }
}

#[must_use]
pub fn step_status_label(status: &StepStatus) -> String {
    match status {
        StepStatus::Completed(reason) => format!("完了 ({})", reason_label(reason)),
        StepStatus::Failed(reason) => format!("失敗 ({})", reason_label(reason)),
        StepStatus::NoFire(reason) => format!("失敗 ({}・発火なし)", reason_label(reason)),
        StepStatus::Interrupted => "割り込み".to_string(),
        StepStatus::Skipped => "スキップ".to_string(),
        StepStatus::InvokerError(e) => format!("失敗 (invoker: {e})"),
    }
}

#[must_use]
pub fn reason_label(reason: &LoopStopReason) -> &'static str {
    match reason {
        LoopStopReason::Stop => "Stop アクション到達",
        LoopStopReason::TerminalTask => "終端タスク到達",
        LoopStopReason::MaxIterations => "最大サイクル到達",
        LoopStopReason::GoalReached => "宣言的ゴール到達",
        LoopStopReason::GoalTimeout => "ゴール未到達タイムアウト",
        LoopStopReason::Interrupted => "Ctrl+C/SIGINT で中断",
        LoopStopReason::CaptureError => "キャプチャエラー",
        LoopStopReason::ExecuteError => "発火エラー",
        LoopStopReason::RecoveryExhausted => "リカバリ試行上限到達 (環境故障で進行不能)",
    }
}

pub async fn run_routine(def: &RoutineDef, invoker: &dyn PipelineInvoker) -> RoutineSummary {
    let mut results = Vec::with_capacity(def.steps.len());
    let mut aborted = false;
    for step in &def.steps {
        if aborted {
            results.push(skipped_result(step));
            continue;
        }
        let result = match invoker.invoke(step).await {
            Ok(outcome) => step_result(step, outcome),
            Err(e) => RoutineStepResult {
                step: step.name.clone(),
                pipeline_dir: step.pipeline_dir.clone(),
                status: StepStatus::InvokerError(e.to_string()),
                iterations: 0,
                fired_count: 0,
                terminal: String::new(),
            },
        };
        let stop_routine = match &result.status {
            StepStatus::Interrupted => true,
            status if status.is_failure() => step.on_failure == OnFailure::Stop,
            _ => false,
        };
        aborted = stop_routine;
        results.push(result);
    }
    summarize(def, results, aborted)
}

fn step_result(step: &RoutineStep, outcome: LoopOutcome) -> RoutineStepResult {
    let status =
        if outcome.reason == LoopStopReason::MaxIterations && outcome.fired_commands.is_empty() {
            StepStatus::NoFire(outcome.reason)
        } else {
            classify_reason(&outcome.reason)
        };
    RoutineStepResult {
        step: step.name.clone(),
        pipeline_dir: step.pipeline_dir.clone(),
        status,
        iterations: outcome.iterations,
        fired_count: outcome.fired_commands.len(),
        terminal: outcome.terminal,
    }
}

fn skipped_result(step: &RoutineStep) -> RoutineStepResult {
    RoutineStepResult {
        step: step.name.clone(),
        pipeline_dir: step.pipeline_dir.clone(),
        status: StepStatus::Skipped,
        iterations: 0,
        fired_count: 0,
        terminal: String::new(),
    }
}

fn summarize(def: &RoutineDef, results: Vec<RoutineStepResult>, aborted: bool) -> RoutineSummary {
    let total_iterations = results.iter().map(|r| r.iterations).sum();
    let total_fired = results.iter().map(|r| r.fired_count).sum();
    let completed = results
        .iter()
        .filter(|r| matches!(r.status, StepStatus::Completed(_)))
        .count();
    let failed = results.iter().filter(|r| r.status.is_failure()).count();
    let skipped = results
        .iter()
        .filter(|r| r.status == StepStatus::Skipped)
        .count();
    RoutineSummary {
        routine: def.name.clone(),
        results,
        total_iterations,
        total_fired,
        completed,
        failed,
        skipped,
        aborted,
    }
}

#[must_use]
pub fn interrupted_summary(def: &RoutineDef) -> RoutineSummary {
    RoutineSummary {
        routine: def.name.clone(),
        results: Vec::new(),
        total_iterations: 0,
        total_fired: 0,
        completed: 0,
        failed: 0,
        skipped: def.steps.len(),
        aborted: true,
    }
}

#[must_use]
pub fn format_dry_run(def: &RoutineDef, root: &std::path::Path) -> String {
    let mut out = format!(
        "=== routine '{}' (dry-run: {} ステップ) ===\n",
        def.name,
        def.steps.len()
    );
    for (i, step) in def.steps.iter().enumerate() {
        let resolved = resolve_pipeline_dir(std::path::Path::new(&step.pipeline_dir), root);
        out.push_str(&format!(
            "  [{i}] {} pipeline={} start={} max_iters={} interval={}s on_failure={}\n",
            step.name,
            resolved
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| step.pipeline_dir.clone()),
            step.start_task,
            step.max_iters,
            step.interval,
            match step.on_failure {
                OnFailure::Stop => "stop",
                OnFailure::Skip => "skip",
            }
        ));
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::routine::{OnFailure, RoutineStep};
    use std::sync::Mutex;
    use std::sync::OnceLock;

    struct RecordingInvoker {
        calls: OnceLock<Mutex<Vec<String>>>,
        scripted: Vec<Result<LoopOutcome, RoutineError>>,
    }

    impl RecordingInvoker {
        fn new(scripted: Vec<Result<LoopOutcome, RoutineError>>) -> Self {
            Self {
                calls: OnceLock::new(),
                scripted,
            }
        }

        fn recorded(&self) -> Vec<String> {
            self.calls
                .get()
                .map(|m| m.lock().unwrap().clone())
                .unwrap_or_default()
        }
    }

    #[async_trait]
    impl PipelineInvoker for RecordingInvoker {
        async fn invoke(&self, step: &RoutineStep) -> Result<LoopOutcome, RoutineError> {
            self.calls
                .get_or_init(|| Mutex::new(Vec::new()))
                .lock()
                .unwrap()
                .push(step.name.clone());
            let index = self.recorded().len() - 1;
            match self.scripted.get(index) {
                Some(res) => res.clone(),
                None => Err(RoutineError::ReadFailed {
                    path: "scripted".into(),
                    reason: "no more scripted responses".into(),
                }),
            }
        }
    }

    fn outcome(reason: LoopStopReason, iterations: u64, fired: usize) -> LoopOutcome {
        LoopOutcome {
            iterations,
            fired_commands: vec![crate::InputCommand::Tap { x: 1, y: 1 }; fired],
            terminal: "term".to_string(),
            reason,
            progress_report: crate::ProgressReport {
                jitter_seed: None,
                verify: Default::default(),
                iterations,
                fired_count: fired as u64,
                per_task_matches: vec![],
                elapsed_ms: 0,
                terminal_task: None,
                reached_goal: None,
                stop_reason: None,
                recovery_attempts: 0,
                max_consecutive_recovery: 0,
            },
        }
    }

    fn step(name: &str, on_failure: OnFailure) -> RoutineStep {
        RoutineStep {
            name: name.to_string(),
            pipeline_dir: format!("templates/pipelines/{name}"),
            start_task: "T".to_string(),
            max_iters: 10,
            interval: 1,
            on_failure,
        }
    }

    fn two_step_def(on_failure_step1: OnFailure) -> RoutineDef {
        RoutineDef {
            name: "rt".to_string(),
            steps: vec![step("s1", on_failure_step1), step("s2", OnFailure::Stop)],
        }
    }

    #[tokio::test]
    async fn runs_steps_in_definition_order() {
        let invoker = RecordingInvoker::new(vec![
            Ok(outcome(LoopStopReason::Stop, 3, 1)),
            Ok(outcome(LoopStopReason::TerminalTask, 5, 2)),
        ]);
        let def = two_step_def(OnFailure::Stop);
        let summary = run_routine(&def, &invoker).await;
        assert_eq!(invoker.recorded(), vec!["s1", "s2"]);
        assert!(summary.all_ok());
    }

    #[tokio::test]
    async fn aggregates_totals_and_counts() {
        let invoker = RecordingInvoker::new(vec![
            Ok(outcome(LoopStopReason::Stop, 3, 2)),
            Ok(outcome(LoopStopReason::MaxIterations, 7, 4)),
        ]);
        let def = two_step_def(OnFailure::Stop);
        let summary = run_routine(&def, &invoker).await;
        assert_eq!(summary.total_iterations, 10);
        assert_eq!(summary.total_fired, 6);
        assert_eq!(summary.completed, 2);
        assert_eq!(summary.failed, 0);
        assert_eq!(summary.skipped, 0);
        assert_eq!(summary.results[0].terminal, "term");
        assert_eq!(summary.results[0].fired_count, 2);
    }

    #[tokio::test]
    async fn max_iterations_with_fired_counts_as_completed() {
        let invoker =
            RecordingInvoker::new(vec![Ok(outcome(LoopStopReason::MaxIterations, 10, 1))]);
        let def = RoutineDef {
            name: "r".to_string(),
            steps: vec![step("s1", OnFailure::Stop)],
        };
        let summary = run_routine(&def, &invoker).await;
        assert_eq!(summary.completed, 1);
        assert!(summary.all_ok());
    }

    #[tokio::test]
    async fn max_iterations_zero_fired_is_nofire_failure() {
        let invoker =
            RecordingInvoker::new(vec![Ok(outcome(LoopStopReason::MaxIterations, 10, 0))]);
        let def = RoutineDef {
            name: "r".to_string(),
            steps: vec![step("s1", OnFailure::Stop)],
        };
        let summary = run_routine(&def, &invoker).await;
        assert_eq!(
            summary.completed, 0,
            "fired=0 の MaxIterations は完了扱いにしない"
        );
        assert_eq!(summary.failed, 1);
        assert!(matches!(
            summary.results[0].status,
            StepStatus::NoFire(LoopStopReason::MaxIterations)
        ));
        assert!(
            !summary.all_ok(),
            "no_fire ステップがある routine は exit 2 相当"
        );
        let label = step_status_label(&summary.results[0].status);
        assert!(label.contains("発火なし"), "label: {label}");
        assert!(label.contains("失敗"), "label: {label}");
    }

    #[tokio::test]
    async fn nofire_failure_follows_on_failure_policy() {
        let invoker = RecordingInvoker::new(vec![
            Ok(outcome(LoopStopReason::MaxIterations, 10, 0)),
            Ok(outcome(LoopStopReason::Stop, 9, 9)),
        ]);
        let def = two_step_def(OnFailure::Stop);
        let summary = run_routine(&def, &invoker).await;
        assert_eq!(invoker.recorded(), vec!["s1"], "s2 must not run");
        assert_eq!(summary.failed, 1);
        assert_eq!(summary.skipped, 1);
        assert!(summary.aborted);
        assert_eq!(summary.results[1].status, StepStatus::Skipped);
    }

    #[tokio::test]
    async fn failure_with_stop_policy_skips_remaining_steps() {
        let invoker = RecordingInvoker::new(vec![
            Ok(outcome(LoopStopReason::CaptureError, 2, 0)),
            Ok(outcome(LoopStopReason::Stop, 9, 9)),
        ]);
        let def = two_step_def(OnFailure::Stop);
        let summary = run_routine(&def, &invoker).await;
        assert_eq!(invoker.recorded(), vec!["s1"], "s2 must not run");
        assert_eq!(summary.failed, 1);
        assert_eq!(summary.skipped, 1);
        assert!(summary.aborted);
        assert!(!summary.all_ok());
        assert_eq!(summary.results[1].status, StepStatus::Skipped);
    }

    #[tokio::test]
    async fn failure_with_skip_policy_continues_to_next_step() {
        let invoker = RecordingInvoker::new(vec![
            Ok(outcome(LoopStopReason::ExecuteError, 2, 0)),
            Ok(outcome(LoopStopReason::Stop, 4, 1)),
        ]);
        let def = two_step_def(OnFailure::Skip);
        let summary = run_routine(&def, &invoker).await;
        assert_eq!(invoker.recorded(), vec!["s1", "s2"]);
        assert_eq!(summary.failed, 1);
        assert_eq!(summary.completed, 1);
        assert_eq!(summary.skipped, 0);
        assert!(!summary.aborted);
        assert!(!summary.all_ok(), "failure remains in summary");
    }

    #[tokio::test]
    async fn goal_timeout_is_failure_subject_to_policy() {
        let invoker = RecordingInvoker::new(vec![
            Ok(outcome(LoopStopReason::GoalTimeout, 5, 0)),
            Ok(outcome(LoopStopReason::Stop, 1, 0)),
        ]);
        let def = two_step_def(OnFailure::Skip);
        let summary = run_routine(&def, &invoker).await;
        assert_eq!(summary.failed, 1);
        assert_eq!(invoker.recorded(), vec!["s1", "s2"]);
    }

    #[tokio::test]
    async fn interrupted_aborts_regardless_of_policy() {
        let invoker = RecordingInvoker::new(vec![
            Ok(outcome(LoopStopReason::Interrupted, 1, 0)),
            Ok(outcome(LoopStopReason::Stop, 9, 9)),
        ]);
        let def = two_step_def(OnFailure::Skip);
        let summary = run_routine(&def, &invoker).await;
        assert_eq!(invoker.recorded(), vec!["s1"]);
        assert!(summary.aborted);
        assert_eq!(summary.results[0].status, StepStatus::Interrupted);
        assert_eq!(summary.results[1].status, StepStatus::Skipped);
    }

    #[tokio::test]
    async fn invoker_error_is_failure_and_stops_when_policy_stop() {
        let invoker = RecordingInvoker::new(vec![Err(RoutineError::ReadFailed {
            path: "x.toml".into(),
            reason: "boom".into(),
        })]);
        let def = two_step_def(OnFailure::Stop);
        let summary = run_routine(&def, &invoker).await;
        assert_eq!(invoker.recorded(), vec!["s1"]);
        assert_eq!(summary.failed, 1);
        assert!(summary.aborted);
        assert!(matches!(
            summary.results[0].status,
            StepStatus::InvokerError(_)
        ));
    }

    #[tokio::test]
    async fn summary_text_contains_step_and_total_lines() {
        let invoker = RecordingInvoker::new(vec![
            Ok(outcome(LoopStopReason::Stop, 3, 1)),
            Ok(outcome(LoopStopReason::CaptureError, 2, 0)),
        ]);
        let summary = run_routine(&two_step_def(OnFailure::Skip), &invoker).await;
        let text = summary.format_text();
        assert!(text.contains("routine 'rt'"), "{text}");
        assert!(text.contains("[0] s1"), "{text}");
        assert!(text.contains("[1] s2"), "{text}");
        assert!(text.contains("failed=1"), "{text}");
    }

    #[test]
    fn dry_run_text_lists_all_steps_with_params() {
        let def = two_step_def(OnFailure::Stop);
        let text = format_dry_run(&def, std::path::Path::new("C:/no-root"));
        assert!(text.contains("dry-run: 2 ステップ"), "{text}");
        assert!(text.contains("[0] s1"), "{text}");
        assert!(text.contains("start=T"), "{text}");
        assert!(text.contains("on_failure=stop"), "{text}");
    }

    #[test]
    fn interrupted_summary_marks_all_steps_skipped_and_aborted() {
        let def = two_step_def(OnFailure::Stop);
        let s = interrupted_summary(&def);
        assert_eq!(s.routine, def.name);
        assert!(s.results.is_empty());
        assert_eq!(s.total_iterations, 0);
        assert_eq!(s.total_fired, 0);
        assert_eq!(s.completed, 0);
        assert_eq!(s.failed, 0);
        assert_eq!(s.skipped, 2);
        assert!(s.aborted);
        assert!(!s.all_ok());
        assert!(
            s.format_text().contains("aborted=true"),
            "summary must report interruption: {}",
            s.format_text()
        );
    }

    #[test]
    fn routine_summary_roundtrips_json_with_all_step_statuses() {
        let summary = RoutineSummary {
            routine: "daily".to_string(),
            results: vec![
                RoutineStepResult {
                    step: "s1".to_string(),
                    pipeline_dir: "templates/pipelines/login".to_string(),
                    status: StepStatus::Completed(LoopStopReason::GoalReached),
                    iterations: 12,
                    fired_count: 3,
                    terminal: "field_hud".to_string(),
                },
                RoutineStepResult {
                    step: "s2".to_string(),
                    pipeline_dir: "p2".to_string(),
                    status: StepStatus::NoFire(LoopStopReason::MaxIterations),
                    iterations: 50,
                    fired_count: 0,
                    terminal: String::new(),
                },
                RoutineStepResult {
                    step: "s3".to_string(),
                    pipeline_dir: "p3".to_string(),
                    status: StepStatus::Failed(LoopStopReason::CaptureError),
                    iterations: 1,
                    fired_count: 0,
                    terminal: String::new(),
                },
                RoutineStepResult {
                    step: "s4".to_string(),
                    pipeline_dir: "p4".to_string(),
                    status: StepStatus::Interrupted,
                    iterations: 0,
                    fired_count: 0,
                    terminal: String::new(),
                },
                RoutineStepResult {
                    step: "s5".to_string(),
                    pipeline_dir: "p5".to_string(),
                    status: StepStatus::Skipped,
                    iterations: 0,
                    fired_count: 0,
                    terminal: String::new(),
                },
                RoutineStepResult {
                    step: "s6".to_string(),
                    pipeline_dir: "p6".to_string(),
                    status: StepStatus::InvokerError("boom".to_string()),
                    iterations: 0,
                    fired_count: 0,
                    terminal: String::new(),
                },
            ],
            total_iterations: 63,
            total_fired: 3,
            completed: 1,
            failed: 3,
            skipped: 1,
            aborted: true,
        };
        let json = serde_json::to_string(&summary).expect("serialize");
        let back: RoutineSummary = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, summary);
    }

    #[test]
    fn outcome_label_reflects_all_ok_and_failure_breakdown() {
        let ok = RoutineSummary {
            routine: "r".to_string(),
            results: vec![RoutineStepResult {
                step: "s1".to_string(),
                pipeline_dir: "p".to_string(),
                status: StepStatus::Completed(LoopStopReason::Stop),
                iterations: 1,
                fired_count: 0,
                terminal: String::new(),
            }],
            total_iterations: 1,
            total_fired: 0,
            completed: 1,
            failed: 0,
            skipped: 0,
            aborted: false,
        };
        assert_eq!(ok.outcome_label(), "routine 完了 (1/1)");

        let mut ng = ok.clone();
        ng.failed = 2;
        ng.skipped = 1;
        ng.aborted = true;
        assert_eq!(
            ng.outcome_label(),
            "routine 失敗 (failed=2 skipped=1 aborted=true)"
        );
    }
}
