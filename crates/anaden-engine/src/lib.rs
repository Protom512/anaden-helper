mod diagnostics;
mod jitter;
mod pipeline_dir;
mod pipeline_driver;
mod pipeline_runner;
mod routine;
mod routine_runner;

pub use diagnostics::{diag_report_dir, save_diagnose_report};
pub use jitter::{Jitter, JitterConfig};
pub use pipeline_dir::resolve_pipeline_dir;
pub use pipeline_driver::{
    Capture, GoalClock, Input, LoopOutcome, LoopStopReason, PipelineDriver, ProgressReport,
    RecoveryClock, RecoveryContext, RecoveryHook, StepErrorKind, StepOutcome, SystemClock,
    SystemRecoveryClock, TaskMatchCount, VerifyReport, format_progress_report,
    is_recoverable_io_error, rescale_command, stop_reason_label,
};
pub use pipeline_runner::{
    InputCommand, PipelineState, TickResult, action_to_command, advance_next,
};
pub use routine::{
    DEFAULT_STEP_INTERVAL_SECS, DEFAULT_STEP_MAX_ITERS, OnFailure, RoutineDef, RoutineError,
    RoutineStep, load_routine,
};
pub use routine_runner::{
    PipelineInvoker, RoutineStepResult, RoutineSummary, StepStatus, classify_reason,
    format_dry_run, interrupted_summary, reason_label, run_routine, step_status_label,
};
