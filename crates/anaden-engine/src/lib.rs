//! 自動化エンジン。Sense→Think→Act ループを駆動する。

mod diagnostics;
mod orchestrator;
mod pipeline_driver;
mod pipeline_runner;
mod recovery;
mod state_machine;

pub use diagnostics::{diag_report_dir, save_diagnose_report};
pub use orchestrator::{AutomationConfig, Orchestrator, RunSummary};
pub use pipeline_driver::{
    Capture, GoalClock, Input, LoopOutcome, LoopStopReason, PipelineDriver, ProgressReport,
    RecoveryHook, StepOutcome, SystemClock, TaskMatchCount, format_progress_report,
    rescale_command,
};
pub use pipeline_runner::{
    InputCommand, PipelineState, TickResult, action_to_command, advance_next,
};
pub use recovery::RecoveryPolicy;
pub use state_machine::GameStateMachine;
