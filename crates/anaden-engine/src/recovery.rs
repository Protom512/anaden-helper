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

impl RecoveryPolicy {
    pub fn new(max_retries: u32) -> Self {
        Self { max_retries }
    }

    pub fn default() -> Self {
        Self { max_retries: 5 }
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
}
