//! 認識失敗時の回復戦略。
//!
/// 画面が `Unknown` 状態になった場合の回復アクションを定義する。
use tracing::{info, warn};

use anaden_core::InputAction;

/// 回復ポリシー。Unknown 状態が継続した場合のアクションを決定する。
pub struct RecoveryPolicy {
    /// 最大リトライ回数。これを超えた場合はエラーとする。
    max_retries: u32,
}

impl Default for RecoveryPolicy {
    fn default() -> Self {
        Self { max_retries: 5 }
    }
}

impl RecoveryPolicy {
    pub fn new(max_retries: u32) -> Self {
        Self { max_retries }
    }

    /// 連続 Unknown 回数に応じた回復アクションを返す。
    ///
    /// 戦略:
    /// - 1〜2回: 待機（画面遷移の途中かもしれない）
    /// - 3〜4回: バックキーを押す（想定外のダイアログを閉じる）
    /// - 5回以上: エラー（諦める）
    pub fn recover_actions(&self, unknown_streak: u32) -> RecoveryAction {
        if unknown_streak > self.max_retries {
            warn!(
                "Unknown state streak ({}) exceeds max retries ({})",
                unknown_streak, self.max_retries
            );
            return RecoveryAction::GiveUp;
        }

        match unknown_streak {
            0 => RecoveryAction::None,
            1..=2 => {
                info!("Unknown state streak {}, waiting...", unknown_streak);
                RecoveryAction::Actions(vec![InputAction::wait_secs(1)])
            }
            3..=4 => {
                info!(
                    "Unknown state streak {}, pressing back button",
                    unknown_streak
                );
                RecoveryAction::Actions(vec![
                    InputAction::wait_ms(500),
                    // Android の「戻る」キーコード (KEYCODE_BACK = 4)
                    // 注: 実際のキーイベント送信は InputAction の拡張で対応
                    InputAction::tap(50, 2300), // 画面左下の戻るボタン位置（暫定）
                    InputAction::wait_secs(1),
                ])
            }
            _ => RecoveryAction::GiveUp,
        }
    }
}

/// 回復アクションの結果。
#[derive(Debug)]
pub enum RecoveryAction {
    /// 回復不要
    None,
    /// 実行すべき入力アクション
    Actions(Vec<InputAction>),
    /// リトライ上限に達した。諦める。
    GiveUp,
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn no_recovery_for_zero_streak() {
        let policy = RecoveryPolicy::default();
        assert!(matches!(policy.recover_actions(0), RecoveryAction::None));
    }

    #[test]
    fn wait_for_low_streak() {
        let policy = RecoveryPolicy::default();
        let result = policy.recover_actions(1);
        assert!(matches!(result, RecoveryAction::Actions(_)));
    }

    #[test]
    fn back_button_for_medium_streak() {
        let policy = RecoveryPolicy::default();
        let result = policy.recover_actions(3);
        assert!(matches!(result, RecoveryAction::Actions(_)));
    }

    #[test]
    fn give_up_at_max_retries() {
        let policy = RecoveryPolicy::new(5);
        assert!(matches!(policy.recover_actions(6), RecoveryAction::GiveUp));
    }

    // ---- verify_after_fire wiring 前提: Default trait impl の保存 ----
    // かつて RecoveryPolicy は inherent `pub fn default()` を持っていたが、
    // Generic コンテキスト(derive Default の伝播や `T: Default` 境界)で使えるよう
    // trait Default へ移行した。移行で変えてはならない不変量:
    //   (1) Default::default() の max_retries == 5 (旧 inherent と同値)
    //   (2) default() == new(5) (意味的に等価)
    // これが崩れると Orchestrator::default 経由でのリカバリ上限が暗黙に変わり、
    // Unknown streak の GiveUp タイミングがズレる(偽成功防止とは別の安全弁の退行)。
    #[test]
    fn default_trait_impl_preserves_max_retries_five() {
        // trait Default 経由で構築。inherent method ではないことを型レベルで担保:
        // RecoveryPolicy: Default 境界を通る関数へ渡して確認。
        fn require_default<T: Default>(v: T) -> T {
            v
        }
        let policy = require_default(RecoveryPolicy::default());

        // max_retries == 5 の契約: streak 4 までは回復アクション(Actions)を返し、
        // streak >= 5 で GiveUp になる。recover_actions は streak == max_retries(5) を
        // match の `_` アームで GiveUp 扱いする(早期 `> max_retries` は 6+ の安全網)。
        // この境界を default が保存していなければ、Orchestrator の安全弁タイミングがズレる。
        assert!(
            matches!(policy.recover_actions(4), RecoveryAction::Actions(_)),
            "streak 4 は max_retries(5) 未満なので回復アクションのはず"
        );
        assert!(
            matches!(policy.recover_actions(5), RecoveryAction::GiveUp),
            "streak 5 == max_retries で GiveUp になるはず"
        );
    }

    #[test]
    fn default_is_equivalent_to_new_five() {
        // default() と new(5) は観測可能な振る舞いで等価(同じ streak 入力 → 同じアクション)。
        let from_default = RecoveryPolicy::default();
        let from_new = RecoveryPolicy::new(5);

        for streak in 0..=8 {
            assert_eq!(
                matches!(from_default.recover_actions(streak), RecoveryAction::GiveUp),
                matches!(from_new.recover_actions(streak), RecoveryAction::GiveUp),
                "streak {streak} で GiveUp 判定が default と new(5) で不一致"
            );
        }
    }
}
