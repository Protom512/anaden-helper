use serde::{Deserialize, Serialize};

use crate::{GameState, ScreenRegion};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TemplateMatch {
    pub region: ScreenRegion,
    pub confidence: MatchConfidence,
    pub state: GameState,
}

#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct MatchConfidence(pub f32);

impl MatchConfidence {
    pub const MAX: Self = Self(1.0);
    pub const MIN: Self = Self(0.0);
    pub const DEFAULT_THRESHOLD: Self = Self(0.95);

    pub fn new(value: f32) -> Self {
        Self(value.clamp(0.0, 1.0))
    }

    pub fn exceeds_threshold(&self, threshold: &MatchConfidence) -> bool {
        self >= threshold
    }

    pub fn is_high(&self) -> bool {
        self.0 >= 0.9
    }

    pub fn is_low(&self) -> bool {
        self.0 < 0.5
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecognitionResult {
    pub matches: Vec<TemplateMatch>,
    pub screen_size: (u32, u32),
}

impl RecognitionResult {
    pub fn best_match(&self) -> Option<&TemplateMatch> {
        self.matches.first()
    }

    pub fn best_above_threshold(&self, threshold: &MatchConfidence) -> Option<&TemplateMatch> {
        self.matches
            .iter()
            .find(|m| m.confidence.exceeds_threshold(threshold))
    }

    pub fn has_state(&self, state: &GameState) -> bool {
        self.matches.iter().any(|m| m.state == *state)
    }

    pub fn is_empty(&self) -> bool {
        self.matches.is_empty()
    }

    pub fn to_game_state(&self, threshold: &MatchConfidence) -> GameState {
        self.best_above_threshold(threshold)
            .map(|m| m.state.clone())
            .unwrap_or(GameState::Unknown)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_state::GameState;

    #[test]
    fn confidence_clamping() {
        let high = MatchConfidence::new(1.5);
        assert_eq!(high, MatchConfidence::MAX);

        let low = MatchConfidence::new(-0.3);
        assert_eq!(low, MatchConfidence::MIN);
    }

    #[test]
    fn threshold_check() {
        let conf = MatchConfidence::new(0.96);
        assert!(conf.exceeds_threshold(&MatchConfidence::DEFAULT_THRESHOLD));

        let low = MatchConfidence::new(0.94);
        assert!(!low.exceeds_threshold(&MatchConfidence::DEFAULT_THRESHOLD));
    }

    #[test]
    fn recognition_result_to_game_state() {
        let result = RecognitionResult {
            matches: vec![TemplateMatch {
                region: ScreenRegion::new(0, 0, 100, 100),
                confidence: MatchConfidence::new(0.96),
                state: GameState::TitleScreen,
            }],
            screen_size: (1080, 2400),
        };

        let state = result.to_game_state(&MatchConfidence::DEFAULT_THRESHOLD);
        assert_eq!(state, GameState::TitleScreen);
    }

    #[test]
    fn empty_result_returns_unknown() {
        let result = RecognitionResult {
            matches: vec![],
            screen_size: (1080, 2400),
        };

        let state = result.to_game_state(&MatchConfidence::DEFAULT_THRESHOLD);
        assert_eq!(state, GameState::Unknown);
    }
}
