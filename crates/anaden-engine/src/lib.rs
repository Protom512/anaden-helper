//! 自動化エンジン。宣言的パイプライン (tick → capture/input ループ) を駆動する。
//!
//! 旧命令型 Orchestrator (Sense→Think→Act ループ・Android ADB 依存) は
//! Issue #188 で Android 経路とともに削除された。
//! Issue #199 で routine (複数 pipeline の連続実行) 系を追加した。

mod diagnostics;
mod pipeline_driver;
mod pipeline_runner;
mod routine;
mod routine_runner;

pub use diagnostics::{diag_report_dir, save_diagnose_report};
pub use pipeline_driver::{
    Capture, GoalClock, Input, LoopOutcome, LoopStopReason, PipelineDriver, ProgressReport,
    RecoveryHook, StepOutcome, SystemClock, TaskMatchCount, format_progress_report,
    rescale_command,
};
pub use pipeline_runner::{
    InputCommand, PipelineState, TickResult, action_to_command, advance_next,
};
pub use routine::{
    DEFAULT_STEP_INTERVAL_SECS, DEFAULT_STEP_MAX_ITERS, OnFailure, RoutineDef, RoutineError,
    RoutineStep, load_routine, resolve_step_pipeline_dir,
};
pub use routine_runner::{
    PipelineInvoker, RoutineStepResult, RoutineSummary, StepStatus, classify_reason,
    format_dry_run, reason_label, run_routine, step_status_label,
};
