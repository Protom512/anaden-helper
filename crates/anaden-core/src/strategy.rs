use crate::{GameState, InputAction, MiniGameType};

pub trait MiniGameStrategy: Send + Sync {
    fn game_type(&self) -> MiniGameType;

    fn decide_actions(&self, state: &GameState) -> Option<Vec<InputAction>>;

    fn is_completed(&self, state: &GameState) -> bool;

    fn name(&self) -> &str {
        "unnamed-strategy"
    }
}
