//! StudioApp の状態定義群 (Issue #162 Shard 1: app.rs 分割) — facade (Issue #175)。
//!
//! 旧 app_state.rs (669行) を自然な seam で 3 モジュールへ分割した:
//! - [`crate::app_state_core`]: StudioApp 本体 (構造体・構築) + AppMode +
//!   EngineKind + 定数 (HEATMAP_DOWNSCALE / STATE_OPTIONS 等)。
//! - [`crate::app_state_connection`]: 接続状態・チェック (ConnectionState /
//!   ConnectionStatus / check_windows_process)。
//! - [`crate::app_state_pipeline_task`]: pipeline task 構築・保存
//!   (PipelineActionKind / pipeline_task_spec / save_pipeline_task)。
//!
//! 依存方向は app_state_core → app_state_connection / app_state_pipeline_task
//! の単一方向 (循環なし)。UI 描画 impl は app_ui 系、状態操作 impl
//! (キュー実行・ファイル入出力) は app.rs に置く。本モジュールは旧公開
//! API を再公開する facade で、呼び出し元 (app / app_ui 系・app.rs の
//! re-export チェーン含む) は無修正。

pub use crate::app_state_connection::{ConnectionState, ConnectionStatus, check_windows_process};
pub use crate::app_state_core::{AppMode, StudioApp};
pub(crate) use crate::app_state_core::{
    DEFAULT_TEMPLATE_NAME, EngineKind, HEATMAP_DOWNSCALE, STATE_OPTIONS,
};
pub use crate::app_state_pipeline_task::{
    PipelineActionKind, pipeline_task_spec, save_pipeline_task,
};
