//! ログ行/UI イベントから漸進更新される純状態トラッカ群
//! (Issue #172: log_view.rs 分割で旧モジュールから移動)。
//!
//! - [`RunStatus`]: pipeline 実行状態サマリ（現在ゴール/ループ回数/停止理由）。
//!   ログ行を `observe` で観測して状態を漸進更新する。
//! - [`AutoScrollFollow`]: 自動スクロール追従の純ロジック (Issue #139 T4)。
//!
//! いずれも IO を持たない純構造体（egui 非依存）で単体テスト可能。

/// 自動スクロール追従の純ロジック (Issue #139 T4)。
///
/// 有効時 (既定) はログ末尾へ張り付き (`should_stick_to_bottom` = true)。
/// UI が新着行数を `observe_new_lines` で通知し、実際に末尾へスクロール
/// されたら `on_scrolled_to_bottom` でペンディングを清算する。MAA/MDA の
/// 「ユーザーが上へスクロールしたら追従を一時停止」相当は enabled トグルで
/// 表現し、無効中は新着行のカウント自体を行わない。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoScrollFollow {
    enabled: bool,
    pending: usize,
}

impl Default for AutoScrollFollow {
    fn default() -> Self {
        Self {
            enabled: true,
            pending: 0,
        }
    }
}

impl AutoScrollFollow {
    /// 追従を有効/無効化する。有効化時にペンディングはクリアされる
    /// （再有効化した瞬間に末尾へ張り付くため）。
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.pending = 0;
        }
    }

    /// 有効か。
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// UI のスクロール領域を末尾へ張り付けるべきか。
    #[must_use]
    pub fn should_stick_to_bottom(&self) -> bool {
        self.enabled
    }

    /// 新着行数を観測する（有効時のみ蓄積）。
    pub fn observe_new_lines(&mut self, n: usize) {
        if self.enabled {
            self.pending = self.pending.saturating_add(n);
        }
    }

    /// 末尾へスクロール完了を通知し、ペンディングを清算する。
    pub fn on_scrolled_to_bottom(&mut self) {
        self.pending = 0;
    }

    /// 未スクロールの新着行数（UI の「新着 N 行」バッジ表示用）。
    #[must_use]
    pub fn pending_lines(&self) -> usize {
        self.pending
    }
}

/// pipeline の実行状態サマリ。ログ行から漸進的に更新される。
///
/// CLI（anaden-cli main.rs）は決定的な出力を行う:
/// - 開始時: `run_loop 開始: interval=... max_iters=N goal=<名前>`
/// - 終了時: `サイクル数: N` / `停止理由:   <ラベル>`
///
/// 本構造体はそれらの行を解析して状態を保持する。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RunStatus {
    /// 実行中か（開始行を観測し、停止理由行を未観測）。
    pub running: bool,
    /// 現在のゴール名（開始行の `goal=`。`(none)` は None）。
    pub goal: Option<String>,
    /// ループ回数（`サイクル数: N` 行。実行中は未知 = None）。
    pub iterations: Option<u64>,
    /// 停止理由ラベル（`停止理由:` 行の右辺）。
    pub stop_reason: Option<String>,
}

impl RunStatus {
    /// 未実行（何も観測していない）状態。
    pub fn new() -> Self {
        Self::default()
    }

    /// 1 行を観測して状態を更新する純メソッド。
    ///
    /// 解析対象行（anaden-cli の出力契約）:
    /// - `run_loop 開始: ... goal=X` → running=true, goal=Some(X)
    ///   （`goal=(none)` は goal=None）
    /// - `サイクル数: N` → iterations=Some(N)
    /// - `停止理由:   L`（コロン後の空白は任意） → stop_reason=Some(L),
    ///   running=false
    pub fn observe(&mut self, line: &str) {
        if line.contains("run_loop 開始") {
            self.running = true;
            self.iterations = None;
            self.stop_reason = None;
            self.goal = line.split("goal=").nth(1).and_then(|rest| {
                let g = rest.trim();
                (g != "(none)").then(|| g.to_string())
            });
        } else if let Some((_, rest)) = line.split_once("サイクル数:") {
            self.iterations = rest.trim().parse::<u64>().ok();
        } else if let Some((_, rest)) = line.split_once("停止理由:") {
            self.stop_reason = Some(rest.trim().to_string());
            self.running = false;
        }
    }

    /// 状態の一行サマリ（UI のステータスバー表示用の純関数）。
    #[allow(dead_code)]
    pub fn summary(&self) -> String {
        if self.running {
            format!(
                "実行中 goal={} iterations={}",
                self.goal.as_deref().unwrap_or("(none)"),
                self.iterations
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "?".into())
            )
        } else if let Some(reason) = &self.stop_reason {
            format!(
                "停止 reason={} iterations={}",
                reason,
                self.iterations
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "?".into())
            )
        } else {
            "未実行".to_string()
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    // ---- RunStatus ----

    #[test]
    fn status_start_line_sets_running_and_goal() {
        let mut s = RunStatus::new();
        s.observe("INFO anaden_cli: run_loop 開始: interval=2s max_iters=10 goal=farm50");
        assert!(s.running);
        assert_eq!(s.goal.as_deref(), Some("farm50"));
        assert_eq!(s.iterations, None);
        assert_eq!(s.stop_reason, None);
    }

    #[test]
    fn status_start_line_without_goal_is_none() {
        let mut s = RunStatus::new();
        s.observe("INFO anaden_cli: run_loop 開始: interval=2s max_iters=10 goal=(none)");
        assert!(s.running);
        assert_eq!(s.goal, None);
    }

    #[test]
    fn status_result_lines_set_iterations_and_stop() {
        let mut s = RunStatus::new();
        s.observe("run_loop 開始: interval=2s max_iters=10 goal=g1");
        s.observe("サイクル数: 42");
        s.observe("停止理由:   宣言的ゴール到達(正常)");
        assert!(!s.running);
        assert_eq!(s.iterations, Some(42));
        assert_eq!(s.stop_reason.as_deref(), Some("宣言的ゴール到達(正常)"));
    }

    #[test]
    fn status_summary_varies_by_phase() {
        let mut s = RunStatus::new();
        assert_eq!(s.summary(), "未実行");
        s.observe("run_loop 開始: interval=2s goal=g1");
        assert_eq!(s.summary(), "実行中 goal=g1 iterations=?");
        s.observe("サイクル数: 3");
        s.observe("停止理由: 最大サイクル到達");
        assert_eq!(s.summary(), "停止 reason=最大サイクル到達 iterations=3");
    }

    #[test]
    fn status_restart_resets_previous_result() {
        let mut s = RunStatus::new();
        s.observe("サイクル数: 5");
        s.observe("停止理由: 最大サイクル到達");
        s.observe("run_loop 開始: interval=2s goal=g2");
        assert!(s.running);
        assert_eq!(s.iterations, None);
        assert_eq!(s.stop_reason, None);
        assert_eq!(s.goal.as_deref(), Some("g2"));
    }

    // ---- AutoScrollFollow (T4: 自動スクロール追従の純ロジック) ----

    #[test]
    fn follow_defaults_enabled_with_no_pending() {
        let f = AutoScrollFollow::default();
        assert!(f.should_stick_to_bottom());
        assert_eq!(f.pending_lines(), 0);
    }

    #[test]
    fn follow_accumulates_pending_new_lines_while_enabled() {
        let mut f = AutoScrollFollow::default();
        f.observe_new_lines(3);
        assert_eq!(f.pending_lines(), 3);
        f.observe_new_lines(2);
        assert_eq!(f.pending_lines(), 5);
    }

    #[test]
    fn follow_scrolled_to_bottom_clears_pending() {
        let mut f = AutoScrollFollow::default();
        f.observe_new_lines(4);
        f.on_scrolled_to_bottom();
        assert_eq!(f.pending_lines(), 0);
        assert!(f.should_stick_to_bottom());
    }

    #[test]
    fn follow_disabled_stops_sticking_and_ignores_new_lines() {
        let mut f = AutoScrollFollow::default();
        f.set_enabled(false);
        assert!(!f.should_stick_to_bottom());
        f.observe_new_lines(10);
        assert_eq!(f.pending_lines(), 0);
    }

    #[test]
    fn follow_reenable_resumes_stick_without_pending() {
        let mut f = AutoScrollFollow::default();
        f.set_enabled(false);
        f.observe_new_lines(10);
        f.set_enabled(true);
        assert!(f.should_stick_to_bottom());
        assert_eq!(f.pending_lines(), 0);
    }
}
