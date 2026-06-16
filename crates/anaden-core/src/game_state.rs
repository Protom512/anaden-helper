//! ゲーム画面の状態を表す型。
//!
//! アナザーエデンの実際の画面遷移:
//!   タイトル画面 → ロード → フィールド画面（前回の終了位置で再開）
//!   フィールド画面 → 左下Menu → メニューバー
//!   フィールド画面 → バトル遭遇 → 戦闘画面
//!   フィールド画面 → 特定場所 → ミニゲーム（釣り等）
//!   フィールド画面 → NPC会話 → 会話ダイアログ
//!
//! ※「ホーム画面」は存在しない。ゲームはフィールド画面（マップ上の探索）が基本状態。

use serde::{Deserialize, Serialize};

/// ゲーム内の画面状態。すべての遷移先はここに定義する。
///
/// 設計意図: 状態の追加はここに集約する。Strategy の分岐もこの型に基づく。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GameState {
    /// タイトル画面。「タップして始める」「ロードゲーム」等が表示されている
    TitleScreen,
    /// フィールド画面。前回終了した場所から再開。マップ上の移動・探索状態
    Field,
    /// バトル中。フェーズ（自ターン/敵ターン/勝利/敗北）を含む
    InBattle(BattlePhase),
    /// メニューバー（左下のMenuから開く）。タブ（装備、スキル、編成等）を含む
    Menu(MenuTab),
    /// ミニゲーム中。種別を含む
    MiniGame(MiniGameType),
    /// ダイアログ表示中（NPC会話、確認、報酬受け取り等）
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

/// メニューバーのタブ種別。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MenuTab {
    /// パーティ編成
    PartyFormation,
    /// 装備・強化
    Equipment,
    /// スキルツリー（VCボード等）
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
    /// NPC会話（ストーリー進行・雑貨屋等）
    NpcConversation,
    /// 確認ダイアログ（はい/いいえ）
    Confirm,
    /// 報酬受け取り
    Reward,
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
    /// `Unknown` は認識失敗、`Loading` は遷移中を意味する。
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
