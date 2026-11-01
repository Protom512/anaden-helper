use std::collections::HashMap;

use tracing::debug;

use anaden_core::{GameState, MiniGameType};

pub struct StrategyRegistry {
    strategies: HashMap<String, Box<dyn anaden_core::MiniGameStrategy>>,
}

impl Default for StrategyRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl StrategyRegistry {
    pub fn new() -> Self {
        Self {
            strategies: HashMap::new(),
        }
    }

    pub fn register(&mut self, strategy: Box<dyn anaden_core::MiniGameStrategy>) {
        let name = strategy.name().to_string();
        self.strategies.insert(name, strategy);
    }

    pub fn find_for_state(&self, state: &GameState) -> Option<&dyn anaden_core::MiniGameStrategy> {
        for strategy in self.strategies.values() {
            if let Some(_actions) = strategy.decide_actions(state) {
                debug!("Found strategy '{}' for state {:?}", strategy.name(), state);
                return Some(strategy.as_ref());
            }
        }
        None
    }

    pub fn find_for_minigame(
        &self,
        game_type: &MiniGameType,
    ) -> Option<&dyn anaden_core::MiniGameStrategy> {
        self.strategies
            .values()
            .find(|s| {
                let sgt = s.game_type();
                std::mem::discriminant(&sgt) == std::mem::discriminant(game_type)
            })
            .map(|s| s.as_ref())
    }

    pub fn len(&self) -> usize {
        self.strategies.len()
    }

    pub fn is_empty(&self) -> bool {
        self.strategies.is_empty()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use anaden_core::{InputAction, MiniGameType};

    struct DummyFishingStrategy;

    impl anaden_core::MiniGameStrategy for DummyFishingStrategy {
        fn game_type(&self) -> MiniGameType {
            MiniGameType::Fishing
        }

        fn decide_actions(&self, state: &GameState) -> Option<Vec<InputAction>> {
            match state {
                GameState::MiniGame(MiniGameType::Fishing) => {
                    Some(vec![InputAction::tap(540, 1200)])
                }
                _ => None,
            }
        }

        fn is_completed(&self, state: &GameState) -> bool {
            !matches!(state, GameState::MiniGame(MiniGameType::Fishing))
        }

        fn name(&self) -> &str {
            "dummy-fishing"
        }
    }

    #[test]
    fn register_and_find_strategy() {
        let mut registry = StrategyRegistry::new();
        registry.register(Box::new(DummyFishingStrategy));

        let found = registry.find_for_minigame(&MiniGameType::Fishing);
        assert!(found.is_some());

        let found = registry.find_for_minigame(&MiniGameType::MogekoHole);
        assert!(found.is_none());
    }

    #[test]
    fn find_strategy_by_state() {
        let mut registry = StrategyRegistry::new();
        registry.register(Box::new(DummyFishingStrategy));

        let found = registry.find_for_state(&GameState::MiniGame(MiniGameType::Fishing));
        assert!(found.is_some());

        let found = registry.find_for_state(&GameState::TitleScreen);
        assert!(found.is_none());
    }

    #[test]
    fn default_is_empty_and_equivalent_to_new() {
        let via_default = StrategyRegistry::default();
        let via_new = StrategyRegistry::new();

        assert!(via_default.is_empty());
        assert_eq!(via_default.len(), 0);
        assert_eq!(via_new.len(), via_default.len());
    }

    #[test]
    fn default_then_register_finds_strategy() {
        let mut registry = StrategyRegistry::default();
        registry.register(Box::new(DummyFishingStrategy));

        assert_eq!(registry.len(), 1);
        assert!(registry.find_for_minigame(&MiniGameType::Fishing).is_some());
    }
}
