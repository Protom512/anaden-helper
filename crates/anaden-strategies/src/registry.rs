//! 戦略の名前解決と管理。

use std::collections::HashMap;

use tracing::debug;

use anaden_core::{GameState, MiniGameType};

/// ミニゲーム戦略を名前で管理するレジストリ。
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

    /// 戦略を登録する。
    pub fn register(&mut self, strategy: Box<dyn anaden_core::MiniGameStrategy>) {
        let name = strategy.name().to_string();
        self.strategies.insert(name, strategy);
    }

    /// 指定したゲーム状態を処理できる戦略を検索する。
    pub fn find_for_state(&self, state: &GameState) -> Option<&dyn anaden_core::MiniGameStrategy> {
        for strategy in self.strategies.values() {
            if let Some(_actions) = strategy.decide_actions(state) {
                debug!("Found strategy '{}' for state {:?}", strategy.name(), state);
                return Some(strategy.as_ref());
            }
        }
        None
    }

    /// 指定したミニゲーム種別の戦略を検索する。
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

    /// 登録されている戦略の数。
    pub fn len(&self) -> usize {
        self.strategies.len()
    }

    /// 戦略が空かどうか。
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

    // ---- verify_after_fire wiring 前提: Default trait impl ----
    // 戦略レジストリは Orchestrator::new で StrategyRegistry::new() 経由で構築されるが、
    // 汎用コンテキスト(Default::default() や derive 先での利用)のため Default を実装する。
    // new() と等価(空のレジストリ)でなければならない。これが崩れると Orchestrator の
    // デフォルト構築経路が空ではなくなり、戦略未登録でも find_for_* が hit する偽陽性に繋がる。
    #[test]
    fn default_is_empty_and_equivalent_to_new() {
        let via_default = StrategyRegistry::default();
        let via_new = StrategyRegistry::new();

        // Default 経由でも空(戦略0件)。new() と同じ初期状態。
        assert!(via_default.is_empty());
        assert_eq!(via_default.len(), 0);
        assert_eq!(via_new.len(), via_default.len());
    }

    #[test]
    fn default_then_register_finds_strategy() {
        // Default で作ったレジストリにも通常通り register 可能(振る舞いが new() と同一)。
        let mut registry = StrategyRegistry::default();
        registry.register(Box::new(DummyFishingStrategy));

        assert_eq!(registry.len(), 1);
        assert!(registry.find_for_minigame(&MiniGameType::Fishing).is_some());
    }
}
