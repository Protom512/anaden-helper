//! StudioApp の UI 描画 impl 群 (Issue #162 Shard 1 → Issue #174 分割)。
//!
//! Issue #174 により以下のモジュールへ分割した (コード移動のみ・依存方向は
//! app_ui_body → app_ui_authoring / app_ui_batch の単一方向):
//! - [`crate::app_ui_body`] — 描画エントリ (eframe::App / render_modebar /
//!   render_body ディスパッチ)
//! - [`crate::app_ui_authoring`] — 作成モード本体 (render_authoring)
//! - [`crate::app_ui_tasks`] — Tasks ペイン (render_task_list / task_detail_ui /
//!   task_log_ui)
//! - [`crate::app_ui_batch`] — バッチ評価モード (batch_ui / run_batch)
//!
//! 描画 impl はすべて StudioApp のメソッドのため、呼び出し元 (app.rs /
//! shell / shell_nav / tests) はメソッド呼び出しのまま無修正。
//! 状態定義は app_state.rs、状態操作 (キュー実行・ファイル入出力) は app.rs。
