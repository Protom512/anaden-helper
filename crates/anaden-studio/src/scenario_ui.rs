//! シナリオ編集 UI: scenario_editor / scenario_panel への facade (Issue #166)。
//!
//! - scenario_editor.rs: 純状態モデル (egui 非依存)。`ScenarioEditorState` /
//!   `ScenarioValidationError` / `save_scenario` / `save_scenario_with_warnings` /
//!   `ScenarioSaveError` / `ScenarioSaveOutcome` /
//!   `resolve_template_reference` / `goal_summary` / `sweep_removed_taskdefs` /
//!   `SCREEN_WIDTH`/`SCREEN_HEIGHT`。
//! - scenario_panel.rs: egui パネル層。`ScenarioPanel` (フォーム状態・描画・
//!   rfd 呼び出し・保存/登録フローの GUI 配線)。
//!
//! 呼び出し元 (app_state / app_ui / scenario_task_link / scenario_load / tests) は
//! 従来どおり `crate::scenario_ui::{...}` (`anaden_studio::scenario_ui::{...}`) で
//! 参照できる (`app` facade と同一パターン・Issue #162)。

pub use crate::scenario_editor::{
    SCREEN_HEIGHT, SCREEN_WIDTH, ScenarioEditorState, ScenarioSaveError, ScenarioSaveOutcome,
    ScenarioValidationError, resolve_template_reference, save_scenario,
    save_scenario_with_warnings,
};
pub use crate::scenario_panel::ScenarioPanel;
