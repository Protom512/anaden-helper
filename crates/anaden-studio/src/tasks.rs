//! MAA 型チェックボックスタスク一覧 UI のドメインロジック facade
//! (Issue #144 / #162 Shard 2 / #170)。
//!
//! `app.rs` は配線のみに徹し、ドメインロジックは下位モジュールへ完全分離する
//! (architecture-coupling-balance: high-cohesion)。本モジュールは公開シンボルの
//! 再エクスポートのみを担い、呼び出し元のパスを不変に保つ。
//!
//! - タスク定義 TOML ドメイン (定義の読み込み・パース・有効化書き戻し):
//!   [`crate::tasks_def`]
//! - チェック順キュー組み立て・逐次実行状態機械・spawn 引数列:
//!   [`crate::tasks_queue`]
//! - 表示モデル純関数 (checkbox_label / TaskDetailView / queue 表示):
//!   [`crate::tasks_view`]
//! - TOML 外科的行編集の純粋関数 (implemented フリップ・pipeline_dir 書き戻し):
//!   [`crate::tasks_toml`]

pub use crate::tasks_def::{
    TaskDefinition, TaskError, TaskKind, enable_task, load_task_definitions,
};
pub use crate::tasks_queue::{
    QueueAction, QueueEntry, QueueExec, QueueState, TaskListState, TaskQueue, resolve_start_task,
    spawn_args,
};
pub use crate::tasks_view::{
    QueueOrderRow, TaskDetailView, checkbox_label, queue_order_rows, queue_position_label,
    task_detail_view,
};
