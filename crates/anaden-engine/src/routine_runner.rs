//! Routine 連続実行エンジン (Issue #199)。
//!
//! pipeline 実行を [`PipelineInvoker`] trait で抽象化し、本番は CLI 側の
//! PipelineDriver 実装・テストは recording double へ差し替える
//! (contract coupling: engine は Win32 に依存しない)。
//! [`run_routine`] はステップ結果 (成功/失敗/発火数/停止理由) を集約し
//! on_failure ポリシー (Stop/Skip) を適用、全体サマリ ([`RoutineSummary`]) を返す。

use async_trait::async_trait;

use crate::routine::{OnFailure, RoutineDef, RoutineError, RoutineStep};
use crate::{LoopOutcome, LoopStopReason, resolve_pipeline_dir};

/// 1ステップ (= pipeline 1実行) の起動抽象。
///
/// 本番実装は CLI 側 (`anaden-cli` の Win32RoutineInvoker) が PipelineDriver を
/// 駆動する。engine は本 trait 経由でのみ pipeline を実行するため、
/// テストは recording double で順序・集計・ポリシーを検証できる。
#[async_trait]
pub trait PipelineInvoker: Send + Sync {
    /// ステップ指定の pipeline を実行してループ成果物を返す。
    ///
    /// # Errors
    /// pipeline 読込失敗・デバイス構築失敗等、実行開始前後の異常は
    /// [`RoutineError`] で返す (panic しない)。
    async fn invoke(&self, step: &RoutineStep) -> Result<LoopOutcome, RoutineError>;
}

/// ステップの実行結果分類。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepStatus {
    /// 正常完了 (Stop/TerminalTask/GoalReached/MaxIterations 到達)。
    Completed(LoopStopReason),
    /// 失敗 (CaptureError/ExecuteError/GoalTimeout または invoker エラー)。
    Failed(LoopStopReason),
    /// Ctrl+C 等の割り込みで中断 (on_failure ポリシーの対象外 = 常に中止)。
    Interrupted,
    /// 実行しなかった (前方ステップ失敗 + on_failure=Stop、または割り込み後の残ステップ)。
    Skipped,
    /// invoker が Err を返した (成果物なし・失敗扱い)。
    InvokerError(String),
}

impl StepStatus {
    /// 失敗系 (Failed/InvokerError) かどうか。
    #[must_use]
    pub fn is_failure(&self) -> bool {
        matches!(self, Self::Failed(_) | Self::InvokerError(_))
    }
}

/// 1ステップの実行記録。
#[derive(Debug, Clone, PartialEq)]
pub struct RoutineStepResult {
    /// ステップ名。
    pub step: String,
    /// pipeline ディレクトリ (未解決の定義文字列)。
    pub pipeline_dir: String,
    /// 実行結果分類。
    pub status: StepStatus,
    /// 実行サイクル数 (未実行・invoker エラー時は 0)。
    pub iterations: u64,
    /// 発火コマンド数。
    pub fired_count: usize,
    /// 終端タスク名 / 停止理由文字列 (未実行時は空)。
    pub terminal: String,
}

/// routine 全体の実行サマリ (ステップ別 + 合計)。
#[derive(Debug, Clone, PartialEq)]
pub struct RoutineSummary {
    /// routine 名。
    pub routine: String,
    /// ステップ別の実行記録 (定義順)。
    pub results: Vec<RoutineStepResult>,
    /// 全ステップのサイクル数合計。
    pub total_iterations: u64,
    /// 全ステップの発火数合計。
    pub total_fired: usize,
    /// 正常完了ステップ数。
    pub completed: usize,
    /// 失敗ステップ数 (invoker エラー込み)。
    pub failed: usize,
    /// 未実行 (skipped) ステップ数。
    pub skipped: usize,
    /// 中断したか (on_failure=Stop での失敗 or 割り込み)。
    pub aborted: bool,
}

impl RoutineSummary {
    /// 全ステップが失敗なく実行完了したか。
    ///
    /// `aborted` (失敗による中止・割り込み) や skipped ステップがあれば false。
    #[must_use]
    pub fn all_ok(&self) -> bool {
        self.failed == 0 && self.skipped == 0 && !self.aborted
    }

    /// 人間可読テキスト (CLI 出力・evidence `routine-summary.txt` の単一情報源)。
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

/// [`LoopStopReason`] を routine ステップの成否へ分類する。
///
/// - Completed: Stop / TerminalTask / GoalReached / MaxIterations
/// - Failed:    CaptureError / ExecuteError / GoalTimeout
/// - Interrupted: 割り込み (ユーザー中止 = on_failure に関わらず routine 中止)
#[must_use]
pub fn classify_reason(reason: &LoopStopReason) -> StepStatus {
    match reason {
        LoopStopReason::Stop
        | LoopStopReason::TerminalTask
        | LoopStopReason::GoalReached
        | LoopStopReason::MaxIterations => StepStatus::Completed(reason.clone()),
        LoopStopReason::CaptureError
        | LoopStopReason::ExecuteError
        | LoopStopReason::GoalTimeout => StepStatus::Failed(reason.clone()),
        LoopStopReason::Interrupted => StepStatus::Interrupted,
    }
}

/// [`StepStatus`] の1行ラベル (サマリ出力用)。
#[must_use]
pub fn step_status_label(status: &StepStatus) -> String {
    match status {
        StepStatus::Completed(reason) => format!("完了 ({})", reason_label(reason)),
        StepStatus::Failed(reason) => format!("失敗 ({})", reason_label(reason)),
        StepStatus::Interrupted => "割り込み".to_string(),
        StepStatus::Skipped => "スキップ".to_string(),
        StepStatus::InvokerError(e) => format!("失敗 (invoker: {e})"),
    }
}

/// [`LoopStopReason`] の日本語ラベル (CLI `run` の表示と同一語彙)。
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
    }
}

/// routine を順次実行し、集計サマリを返す。
///
/// 事前に [`RoutineDef::validate`] 済みの定義を渡すこと (本関数は再検証しない)。
/// - 各ステップを定義順に [`PipelineInvoker::invoke`] で実行し、
///   [`classify_reason`] で成否を分類する。
/// - 失敗 (Failed/InvokerError) 時: `on_failure == Stop` なら残りを
///   [`StepStatus::Skipped`] として中止。`Skip` なら次ステップへ継続。
/// - 割り込み (Interrupted): ポリシーに関わらず残りを Skipped として中止。
///
/// panic しない (invoker エラーは InvokerError ステップとして集計)。
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
            // 割り込みはポリシーに関わらず中止 (ユーザー意思の尊重)。
            StepStatus::Interrupted => true,
            // 失敗系は on_failure ポリシーに従う。
            status if status.is_failure() => step.on_failure == OnFailure::Stop,
            _ => false,
        };
        aborted = stop_routine;
        results.push(result);
    }
    summarize(def, results, aborted)
}

/// [`LoopOutcome`] からステップ記録を組み立てる。
fn step_result(step: &RoutineStep, outcome: LoopOutcome) -> RoutineStepResult {
    RoutineStepResult {
        step: step.name.clone(),
        pipeline_dir: step.pipeline_dir.clone(),
        status: classify_reason(&outcome.reason),
        iterations: outcome.iterations,
        fired_count: outcome.fired_commands.len(),
        terminal: outcome.terminal,
    }
}

/// 未実行ステップの記録。
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

/// ステップ記録列から全体サマリを導出する (純関数)。
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

/// Ctrl+C 等で routine 実行前に中断された場合のサマリ (CLI の select! 配線用)。
///
/// 全ステップを未実行 (skipped)・aborted として記録する。実行中ステップの
/// 打ち切り詳細は残らない (future drop による中断のため) — 「中断された」事実
/// と件数のみを誠実に報告する。
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

/// バリデーション済み routine の dry-run 表示テキスト (ステップ一覧)。
///
/// `--dry-run` の CLI 表示・GUI プレビューの単一情報源。pipeline_dir は
/// root 基準で解決した絶対パスを併記する (解決不能なら定義文字列のまま)。
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

    /// 呼出記録 + スクリプト化された応答を返す recording double。
    struct RecordingInvoker {
        calls: OnceLock<Mutex<Vec<String>>>,
        /// 呼出順に対応する応答。None (= 呼び出し超過) は InvokerError。
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
                iterations,
                fired_count: fired as u64,
                per_task_matches: vec![],
                elapsed_ms: 0,
                terminal_task: None,
                reached_goal: None,
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

    // ---- 順序と集計 ----

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
    async fn max_iterations_counts_as_completed() {
        let invoker =
            RecordingInvoker::new(vec![Ok(outcome(LoopStopReason::MaxIterations, 10, 0))]);
        let def = RoutineDef {
            name: "r".to_string(),
            steps: vec![step("s1", OnFailure::Stop)],
        };
        let summary = run_routine(&def, &invoker).await;
        assert_eq!(summary.completed, 1);
        assert!(summary.all_ok());
    }

    // ---- on_failure ポリシー ----

    #[tokio::test]
    async fn failure_with_stop_policy_skips_remaining_steps() {
        let invoker = RecordingInvoker::new(vec![
            Ok(outcome(LoopStopReason::CaptureError, 2, 0)),
            Ok(outcome(LoopStopReason::Stop, 9, 9)), // 呼ばれない
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
            Ok(outcome(LoopStopReason::Stop, 9, 9)), // 呼ばれない
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

    // ---- 表示ヘルパ ----

    #[tokio::test]
    async fn summary_text_contains_step_and_total_lines() {
        let invoker = RecordingInvoker::new(vec![
            Ok(outcome(LoopStopReason::Stop, 3, 1)),
            Ok(outcome(LoopStopReason::CaptureError, 2, 0)),
        ]);
        // on_failure=skip: 両ステップ実行・1 失敗。
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
}
