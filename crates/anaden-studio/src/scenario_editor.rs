//! シナリオ編集ドメインモデル facade (Issue #160 Shard 1 / Issue #166 / #173 分割)。
//!
//! `templates/pipelines/<name>/` 配下の pipeline manifest (start_task + goals) と
//! TaskDef 群を GUI で作成・編集するための純状態モデル群の re-export facade。
//! egui 非依存の状態操作層であり、描画パネルは `scenario_panel` が本モデルの
//! 上に構築する (`strategy_ui` / `tasks` と同じ「純モデル + egui パネル分離」
//! パターン)。呼び出し元互換のため scenario_ui も本モジュールの公開アイテムを
//! re-export する。
//!
//! Issue #173 で god-module 分割シリーズ (#162/#166/#168/#170/#172) と同一
//! パターンで 3 モジュールへ分割した:
//!
//! - `scenario_state`: 状態操作本体 ([`ScenarioEditorState`] +
//!   [`resolve_template_reference`] + `goal_summary`)。
//! - `scenario_validate`: バリデーション ([`ScenarioValidationError`] +
//!   `validation_issues`/`validate` + [`SCREEN_WIDTH`]/[`SCREEN_HEIGHT`])。
//! - `scenario_save`: 保存/IO ([`save_scenario`] / [`save_scenario_with_warnings`] +
//!   [`ScenarioSaveError`] / [`ScenarioSaveOutcome`])。
//!
//! 依存方向は `scenario_save` → `scenario_validate` → `scenario_state` の
//! 単一方向 (循環なし)。保存は `anaden_vision::save_task_def` /
//! `save_pipeline_manifest` に委譲 (保存 -> load 往復はテストで機械保証)。

pub use crate::scenario_save::{
    ScenarioSaveError, ScenarioSaveOutcome, save_scenario, save_scenario_with_warnings,
};
pub use crate::scenario_state::{ScenarioEditorState, resolve_template_reference};
pub use crate::scenario_validate::{SCREEN_HEIGHT, SCREEN_WIDTH, ScenarioValidationError};

pub(crate) use crate::scenario_state::goal_summary;
