//! ミニゲーム戦略の trait 定義。
//!
//! 各ミニゲームの操作ロジックは、この trait を実装することでプラグインのように追加できる。
//! Strategy は「純粋な判断」のみを行い、副作用（デバイス入力）は呼び出し側に委ねる。

use crate::{GameState, InputAction, MiniGameType};

/// ミニゲーム戦略の契約。
///
/// 設計意図: 戦略は「現在状態 → 取るべき行動のリスト」を返す純粋関数に近い形にする。
/// 副作用（デバイス入力）は `Orchestrator` が担当する。
/// これにより、各戦略を単体テスト可能にする。
pub trait MiniGameStrategy: Send + Sync {
    /// この戦略が対象とするミニゲーム種別。
    fn game_type(&self) -> MiniGameType;

    /// 現在のゲーム状態から、次に取るべき行動を決定する。
    ///
    /// 戻り値が `None` の場合、この戦略では処理できないことを意味する。
    /// `Orchestrator` は別の戦略またはデフォルト動作に委譲する。
    fn decide_actions(&self, state: &GameState) -> Option<Vec<InputAction>>;

    /// この戦略が処理を終了すべきか判定する。
    ///
    /// 終了条件の例:
    /// - 釣り: 魚を釣り上げた、または失敗した
    /// - バトル: 勝利または敗北した
    fn is_completed(&self, state: &GameState) -> bool;

    /// 戦略の名前（ログ・デバッグ用）。
    fn name(&self) -> &str {
        "unnamed-strategy"
    }
}
