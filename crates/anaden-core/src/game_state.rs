use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GameState {
    TitleScreen,
    Field,
    InBattle(BattlePhase),
    Menu(MenuTab),
    MiniGame(MiniGameType),
    Dialog(DialogType),
    Loading,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BattlePhase {
    PlayerTurn,
    EnemyTurn,
    Victory,
    Defeat,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MenuTab {
    PartyFormation,
    Equipment,
    SkillTree,
    Items,
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MiniGameType {
    Fishing,
    MogekoHole,
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DialogType {
    NpcConversation,
    Confirm,
    Reward,
    Unknown,
}

impl GameState {
    pub fn is_terminal(&self) -> bool {
        matches!(self, GameState::TitleScreen)
    }

    pub fn is_stable(&self) -> bool {
        !matches!(self, GameState::Unknown | GameState::Loading)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn game_state_clone_preserves_equality() {
        let states = vec![
            GameState::TitleScreen,
            GameState::Field,
            GameState::InBattle(BattlePhase::PlayerTurn),
            GameState::MiniGame(MiniGameType::Fishing),
            GameState::Dialog(DialogType::NpcConversation),
            GameState::Unknown,
        ];
        for state in &states {
            assert_eq!(*state, state.clone());
        }
    }

    #[test]
    fn game_state_hash_and_eq_consistency() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(GameState::TitleScreen.clone());
        set.insert(GameState::TitleScreen.clone());
        set.insert(GameState::Field.clone());
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn title_screen_is_terminal() {
        assert!(GameState::TitleScreen.is_terminal());
        assert!(!GameState::Field.is_terminal());
    }

    #[test]
    fn unknown_and_loading_are_not_stable() {
        assert!(!GameState::Unknown.is_stable());
        assert!(!GameState::Loading.is_stable());
        assert!(GameState::Field.is_stable());
    }
}
