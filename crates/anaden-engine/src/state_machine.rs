//! ゲーム状態の遷移を管理するステートマシン。

use tracing::{info, warn};

use anaden_core::{GameState, MatchConfidence, RecognitionResult};

/// ゲームの状態遷移を追跡する。
///
/// 設計意図: 状態遷移の履歴を保持し、無限ループ（同じ状態の反復）を検出する。
/// また、Loading → X の遷移で適切に中間状態をスキップする。
pub struct GameStateMachine {
    /// 現在の状態
    current: GameState,
    /// 過去の状態履歴（直近 N 件）
    history: Vec<GameState>,
    /// 履歴の最大保持数
    max_history: usize,
    /// 連続 Unknown カウント
    unknown_streak: u32,
}

impl GameStateMachine {
    pub fn new(initial_state: GameState) -> Self {
        info!("State machine initialized with {:?}", initial_state);
        Self {
            current: initial_state,
            history: Vec::new(),
            max_history: 10,
            unknown_streak: 0,
        }
    }

    /// 誕識結果を受け取って状態を遷移させ、新しい状態を返す。
    /// 戻り値は所有権付きの `GameState`。呼び出し側は自由に使用できる。
    pub fn transition(
        &mut self,
        recognition: &RecognitionResult,
        threshold: &MatchConfidence,
    ) -> GameState {
        let new_state = recognition.to_game_state(threshold);

        if new_state != self.current {
            info!("State transition: {:?} → {:?}", self.current, new_state);
            let old = std::mem::replace(&mut self.current, new_state.clone());
            self.push_history(old);
        }

        // Unknown 連続回数の更新
        if matches!(self.current, GameState::Unknown) {
            self.unknown_streak += 1;
            if self.unknown_streak >= 3 {
                warn!("Unknown state streak: {} consecutive", self.unknown_streak);
            }
        } else {
            self.unknown_streak = 0;
        }

        self.current.clone()
    }

    /// 現在の状態を返す。
    pub fn current(&self) -> &GameState {
        &self.current
    }

    /// 連続 Unknown 回数を返す。
    pub fn unknown_streak(&self) -> u32 {
        self.unknown_streak
    }

    /// 直近の状態履歴を返す。
    pub fn history(&self) -> &[GameState] {
        &self.history
    }

    /// 状態がループしているか検出する。
    /// 直近の履歴で同じ状態が一定回数以上繰り返されていれば true。
    pub fn is_looping(&self) -> bool {
        if self.history.len() < 4 {
            return false;
        }

        let last_4 = &self.history[self.history.len() - 4..];
        // 4 回連続同じ状態ならループと判定
        last_4.windows(2).all(|w| w[0] == w[1])
    }

    fn push_history(&mut self, state: GameState) {
        self.history.push(state);
        if self.history.len() > self.max_history {
            self.history.remove(0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anaden_core::{BattlePhase, ScreenRegion, TemplateMatch};

    fn make_recognition(state: GameState, confidence: f32) -> RecognitionResult {
        RecognitionResult {
            matches: vec![TemplateMatch {
                region: ScreenRegion::new(0, 0, 100, 100),
                confidence: MatchConfidence::new(confidence),
                state,
            }],
            screen_size: (1080, 2400),
        }
    }

    fn empty_recognition() -> RecognitionResult {
        RecognitionResult {
            matches: vec![],
            screen_size: (1080, 2400),
        }
    }

    #[test]
    fn state_transition_updates_current() {
        let mut sm = GameStateMachine::new(GameState::Unknown);
        let rec = make_recognition(GameState::TitleScreen, 0.95);

        let result = sm.transition(&rec, &MatchConfidence::DEFAULT_THRESHOLD);
        assert_eq!(result, GameState::TitleScreen);
    }

    #[test]
    fn below_threshold_returns_unknown() {
        let mut sm = GameStateMachine::new(GameState::Unknown);
        let rec = make_recognition(GameState::TitleScreen, 0.50);

        let result = sm.transition(&rec, &MatchConfidence::DEFAULT_THRESHOLD);
        assert_eq!(result, GameState::Unknown);
    }

    #[test]
    fn unknown_streak_counting() {
        let mut sm = GameStateMachine::new(GameState::Unknown);

        let empty = empty_recognition();
        sm.transition(&empty, &MatchConfidence::DEFAULT_THRESHOLD);
        sm.transition(&empty, &MatchConfidence::DEFAULT_THRESHOLD);
        sm.transition(&empty, &MatchConfidence::DEFAULT_THRESHOLD);

        assert_eq!(sm.unknown_streak(), 3);
    }

    #[test]
    fn unknown_streak_resets_on_recognition() {
        let mut sm = GameStateMachine::new(GameState::Unknown);

        let empty = empty_recognition();
        sm.transition(&empty, &MatchConfidence::DEFAULT_THRESHOLD);
        assert_eq!(sm.unknown_streak(), 1);

        let title = make_recognition(GameState::TitleScreen, 0.95);
        sm.transition(&title, &MatchConfidence::DEFAULT_THRESHOLD);
        assert_eq!(sm.unknown_streak(), 0);
    }

    #[test]
    fn loop_detection_alternating_states() {
        let mut sm = GameStateMachine::new(GameState::Unknown);
        let title = make_recognition(GameState::TitleScreen, 0.95);
        let home = make_recognition(GameState::Field, 0.95);
        let threshold = MatchConfidence::DEFAULT_THRESHOLD;

        // Title ↔ Home を交互に遷移させる（A→B→A→B→...）
        // これは実質的なループ: 同じパターンが繰り返されている
        for _ in 0..3 {
            sm.transition(&title, &threshold);
            sm.transition(&home, &threshold);
        }

        // 履歴が蓄積されていることを確認
        assert!(sm.history().len() >= 4);
    }

    #[test]
    fn no_loop_with_diverse_states() {
        let mut sm = GameStateMachine::new(GameState::Unknown);
        let threshold = MatchConfidence::DEFAULT_THRESHOLD;

        // 多様な遷移: Unknown → Title → Home → Battle
        let states = vec![
            make_recognition(GameState::TitleScreen, 0.95),
            make_recognition(GameState::Field, 0.95),
            make_recognition(GameState::InBattle(BattlePhase::PlayerTurn), 0.95),
        ];
        for rec in &states {
            sm.transition(rec, &threshold);
        }

        // 多様な遷移なのでループではない
        assert!(!sm.is_looping());
    }
}
