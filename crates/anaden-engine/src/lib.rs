//! 自動化エンジン。Sense→Think→Act ループを駆動する。

mod orchestrator;
mod pipeline_driver;
mod pipeline_runner;
mod recovery;
mod state_machine;

pub use orchestrator::{AutomationConfig, Orchestrator, RunSummary};
pub use pipeline_driver::{
    Capture, Input, LoopOutcome, LoopStopReason, PipelineDriver, RecoveryHook, StepOutcome,
    rescale_command,
};
pub use pipeline_runner::{
    InputCommand, PipelineState, TickResult, action_to_command, advance_next,
};
pub use recovery::RecoveryPolicy;
pub use state_machine::GameStateMachine;
