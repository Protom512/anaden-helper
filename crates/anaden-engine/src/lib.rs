//! 自動化エンジン。宣言的パイプライン (tick → capture/input ループ) を駆動する。
//!
//! 旧命令型 Orchestrator (Sense→Think→Act ループ・Android ADB 依存) は
//! Issue #188 で Android 経路とともに削除された。

mod diagnostics;
mod pipeline_driver;
mod pipeline_runner;

pub use diagnostics::{diag_report_dir, save_diagnose_report};
pub use pipeline_driver::{
    Capture, GoalClock, Input, LoopOutcome, LoopStopReason, PipelineDriver, ProgressReport,
    RecoveryHook, StepOutcome, SystemClock, TaskMatchCount, format_progress_report,
    rescale_command,
};
pub use pipeline_runner::{
    InputCommand, PipelineState, TickResult, action_to_command, advance_next,
};
