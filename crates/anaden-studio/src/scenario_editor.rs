pub use crate::scenario_save::{
    ScenarioSaveError, ScenarioSaveOutcome, save_scenario, save_scenario_with_warnings,
};
pub use crate::scenario_state::{ScenarioEditorState, resolve_template_reference};
pub use crate::scenario_validate::{SCREEN_HEIGHT, SCREEN_WIDTH, ScenarioValidationError};

pub(crate) use crate::scenario_state::goal_summary;
