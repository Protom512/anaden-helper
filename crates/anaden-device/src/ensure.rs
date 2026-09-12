//! ゲーム起動保証の成果物型 (プラットフォーム非依存)。
//!
//! `Win32Launch::ensure_open` 等の起動保証呼び出しが返す純粋な結果 enum。
//! CLI の終了コード契約 (`anaden_cli_contract::ensure_open_exit_code`) は
//! 本 enum を射影する。

/// 起動保証 (`ensure_open` 系) のポーリング結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnsureOutcome {
    /// 既に起動・前景相当だった(起動不要)。
    AlreadyOpen,
    /// 起動し、待機期間内に生存・前景化を確認した。
    Launched,
    /// 起動したが待機期間経過でも生存・前景化を確認できなかった。タイムアウト。
    Timeout,
}
