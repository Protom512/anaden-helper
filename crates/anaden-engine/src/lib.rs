//! 自動化エンジン。Sense→Think→Act ループを駆動する。

mod orchestrator;
mod recovery;
mod state_machine;

pub use orchestrator::{AutomationConfig, Orchestrator, RunSummary};
pub use recovery::RecoveryPolicy;
pub use state_machine::GameStateMachine;
