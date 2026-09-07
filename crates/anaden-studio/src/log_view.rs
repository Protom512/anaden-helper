//! 実行ログ/状態表示コア（シャード4, Issue #83）— facade (Issue #172)。
//!
//! 旧 log_view.rs (950行) を自然な seam で 3 モジュールへ分割した:
//! - [`crate::log_status`]: 純状態トラッカ (RunStatus / AutoScrollFollow)。
//! - [`crate::log_buffer`]: 固定長ログバッファ + revision ベース差分
//!   スナップショット + チャネル drain (LogLevel / LogEntry / LogEvent /
//!   LogBuffer / SharedLogBuffer / drain_channel_into / DEFAULT_MAX_LINES)。
//! - [`crate::log_process`]: 子プロセス stdout/stderr リーダ起動
//!   (spawn_stdout_reader / spawn_output_readers / SharedChild)。
//!
//! 依存方向は log_process → log_buffer → log_status の単一方向 (循環なし)。
//! 本モジュールは旧公開 API を再公開する facade で、呼び出し元
//! (app / app_state / app_ui / childproc / runner_exec / runner_ui /
//! integration tests / benches) は無修正。

pub use crate::log_buffer::{
    DEFAULT_MAX_LINES, LogBuffer, LogEntry, LogEvent, LogLevel, SharedLogBuffer, drain_channel_into,
};
pub use crate::log_process::{SharedChild, spawn_output_readers, spawn_stdout_reader};
pub use crate::log_status::{AutoScrollFollow, RunStatus};
