pub mod action;
pub mod game_state;
pub mod goal;
pub mod recognition;
pub mod region;
pub mod strategy;

pub use action::{InputAction, ScreenPoint};
pub use game_state::{BattlePhase, DialogType, GameState, MenuTab, MiniGameType};
pub use goal::{
    Goal, GoalError, GoalReport, GoalStatus, GoalStatusContext, StopCondition, evaluate,
};
pub use recognition::{MatchConfidence, RecognitionResult, TemplateMatch};
pub use region::ScreenRegion;
pub use strategy::MiniGameStrategy;

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn reexports_match_concrete_module_paths() {
        // ルート再エクスポートが具象モジュールと同一であることを型レベルで検証。
        fn assert_same<T>(_a: T, _b: T) {}

        assert_same(InputAction::tap(1, 2), action::InputAction::tap(1, 2));
        assert_same(
            ScreenRegion::new(0, 0, 100, 200),
            region::ScreenRegion::new(0, 0, 100, 200),
        );
    }
}
