//! 画像認識の結果を表す型。
//!
//! テンプレートマッチングの結果は、ここで定義された型で上位層に渡される。
//! 認識の成否と信頼度を明示的に扱う。

use serde::{Deserialize, Serialize};

use crate::{GameState, ScreenRegion};

/// テンプレートマッチングの1件のマッチ結果。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TemplateMatch {
    /// マッチした画面上の領域
    pub region: ScreenRegion,
    /// マッチの信頼度（0.0〜1.0）。1.0 に近いほど一致度が高い
    pub confidence: MatchConfidence,
    /// このテンプレートに対応するゲーム状態
    pub state: GameState,
}

/// テンプレートマッチの信頼度。
///
/// 設計意図: 信頼度は `f32` のラッパーとして型安全に扱う。
/// 閾値との比較をメソッドで提供し、マジックナンバーを排除する。
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct MatchConfidence(pub f32);

impl MatchConfidence {
    /// 信頼度の最大値。
    pub const MAX: Self = Self(1.0);
    /// 信頼度の最小値。
    pub const MIN: Self = Self(0.0);
    /// テンプレートマッチのデフォルト閾値（95%）。
    pub const DEFAULT_THRESHOLD: Self = Self(0.95);

    /// 新しい信頼度を作成。値は [0.0, 1.0] にクランプされる。
    pub fn new(value: f32) -> Self {
        Self(value.clamp(0.0, 1.0))
    }

    /// 閾値を満たしているか判定する。
    pub fn exceeds_threshold(&self, threshold: &MatchConfidence) -> bool {
        self >= threshold
    }

    /// 信頼度が高い（90%以上）か。
    pub fn is_high(&self) -> bool {
        self.0 >= 0.9
    }

    /// 信頼度が低い（50%未満）か。
    pub fn is_low(&self) -> bool {
        self.0 < 0.5
    }
}

/// 画面全体の認識結果。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecognitionResult {
    /// 検出されたすべてのマッチ（信頼度降順）
    pub matches: Vec<TemplateMatch>,
    /// スクリーンショットの解像度（幅, 高さ）
    pub screen_size: (u32, u32),
}

impl RecognitionResult {
    /// 最も信頼度の高いマッチを返す。マッチがない場合は `None`。
    pub fn best_match(&self) -> Option<&TemplateMatch> {
        self.matches.first()
    }

    /// 指定閾値を超えるマッチのうち、最も信頼度の高いものを返す。
    pub fn best_above_threshold(&self, threshold: &MatchConfidence) -> Option<&TemplateMatch> {
        self.matches
            .iter()
            .find(|m| m.confidence.exceeds_threshold(threshold))
    }

    /// 指定したゲーム状態にマッチするものがあるか。
    pub fn has_state(&self, state: &GameState) -> bool {
        self.matches.iter().any(|m| m.state == *state)
    }

    /// 認識結果が空（何もマッチしなかった）か。
    pub fn is_empty(&self) -> bool {
        self.matches.is_empty()
    }

    /// 認識結果から最も確からしい GameState を決定する。
    /// マッチがない場合は `GameState::Unknown` を返す。
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
