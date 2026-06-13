//! ゲーム画面の状態を表す型。
//!
//! ゲーム内の各画面は離散状態として定義する。テンプレートマッチングの結果は
//! ここで定義された `GameState` のいずれかにマッピングされる。

use serde::{Deserialize, Serialize};

/// ゲーム内の画面状態。すべての遷移先はここに定義する。
///
/// 設計意図: 状態の追加はここに集約する。Strategy の分岐もこの型に基づく。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GameState {
    /// タイトル画面。「タップして始める」が表示されている
    TitleScreen,
    /// ホーム画面（地図選択、キャラ一覧等）
    HomeScreen,
    /// バトル中。フェーズ（自ターン/敵ターン/勝利/敗北）を含む
    InBattle(BattlePhase),
    /// メニュー画面。タブ（装備、スキル、編成等）を含む
    Menu(MenuTab),
    /// ミニゲーム中。種別を含む
    MiniGame(MiniGameType),
    /// ダイアログ表示中（確認、報酬受け取り等）
    Dialog(DialogType),
    /// ロード中（暗転画面、ローディングアイコン等）
    Loading,
    /// 認識不能（テンプレートにマッチしない）
    Unknown,
}

/// バトル内のフェーズ。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BattlePhase {
    /// プレイヤーのターン。コマンド選択中
    PlayerTurn,
    /// 敵のターン。攻撃アニメーション中
    EnemyTurn,
    /// 勝利画面。経験値・報酬表示
    Victory,
    /// 敗北画面。コンティニュー確認
    Defeat,
}

/// メニュー内のタブ種別。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MenuTab {
    /// パーティ編成
    PartyFormation,
    /// 装備・強化
    Equipment,
    /// スキルツリー
    SkillTree,
    /// アイテム一覧
    Items,
    /// その他（設定、ヘルプ等）
    Other(String),
}

/// ミニゲームの種別。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MiniGameType {
    /// 釣りミニゲーム
    Fishing,
    /// モゲコの穴（将来対応）
    MogekoHole,
    /// 未定義のミニゲーム（名前で識別）
    Other(String),
}

/// ダイアログの種別。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DialogType {
    /// 確認ダイアログ（はい/いいえ）
    Confirm,
    /// 報酬受け取り
    Reward,
    /// ストーリー会話
    Story,
    /// 不明なダイアログ
    Unknown,
}

impl GameState {
    /// この状態が「処理完了」を意味するか。
    /// ループの終了判定に使用する。
    pub fn is_terminal(&self) -> bool {
        matches!(self, GameState::TitleScreen)
    }

    /// この状態が安定状態（遷移可能）か。
    /// `Unknown` は認識失敗を意味するため、リカバリーの対象になる。
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
            GameState::InBattle(BattlePhase::PlayerTurn),
            GameState::MiniGame(MiniGameType::Fishing),
            GameState::Dialog(DialogType::Confirm),
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
        set.insert(GameState::HomeScreen.clone());
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn title_screen_is_terminal() {
        assert!(GameState::TitleScreen.is_terminal());
        assert!(!GameState::HomeScreen.is_terminal());
    }

    #[test]
    fn unknown_and_loading_are_not_stable() {
        assert!(!GameState::Unknown.is_stable());
        assert!(!GameState::Loading.is_stable());
        assert!(GameState::HomeScreen.is_stable());
    }
}
