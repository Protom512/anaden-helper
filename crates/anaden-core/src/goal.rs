//! 宣言的ゴール（終端状態）と停止条件のドメインモデル。
//!
//! パイプラインは「ここに到達したら停止する」という終端条件を宣言できる。
//! 本モジュールは純粋な契約層であり、I/O も async も持たない。
//! 上位層（`anaden-engine`）がループ毎に [`evaluate`] を呼び、
//! [`GoalStatus`] に基づいて構造化終了を行う。
//!
//! ## ゴールの意味論（count-tracking）
//!
//! [`GoalStatusContext::iterations`] は **tick 数**（`evaluate` の呼出回数）を数える。
//! すなわち、画像認識 NoMatch やアクションエラーを含む全反復を 1 tick として扱う。
//! 「N回成功したら停止」ではなく「N回評価したら停止」が `LoopCount` の意味である。
//!
//! これは UC-1 の `loop_count=50` を「パイプラインを50周したら停止」という
//! 時間駆動の意味に固定し、認識成功率に依存しない終端保証を与える。
//!
//! ## 非スコープ
//!
//! 複数ゴールの AND/OR 合成、自動リスタート、動的ゴール変更は扱わない。
//! 1つの [`Goal`] は単一バリアントの [`StopCondition`] のみを持つ。

use serde::{Deserialize, Serialize};

use crate::ScreenRegion;

/// ゴールの停止条件。
///
/// 各バリアントがユースケース1つに対応する:
/// - `LoopCount`: UC-1（指定回数の反復で停止）
/// - `TemplateMatch`: UC-2（テンプレートマッチで停止）
/// - `Timeout`: UC-3（指定秒数経過で停止）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum StopCondition {
    /// 指定回数の反復（tick）に到達したら停止する。
    ///
    /// `target` は正の整数でなければならない（`validate` 参照）。
    LoopCount {
        /// 目標反復数。1以上。
        target: u64,
    },
    /// 指定タスクのテンプレートが閾値以上の信頼度でマッチしたら停止する。
    ///
    /// `confidence` は (0.0, 1.0] の範囲でなければならない。
    TemplateMatch {
        /// マッチ対象のタスク名（テンプレート識別子）。
        task: String,
        /// マッチ判定の信頼度閾値。0.0 < confidence <= 1.0。
        confidence: f32,
    },
    /// 指定秒数が経過したら停止する。
    Timeout {
        /// タイムアウトまでの秒数。1以上。
        secs: u64,
    },
}

/// ゴール検証エラー。
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum GoalError {
    /// `LoopCount::target` または `Timeout::secs` が 0 だった。
    #[error("value must be greater than 0 (field: {field})")]
    NonPositive {
        /// 0 だったフィールド名。
        field: &'static str,
    },
    /// `TemplateMatch::confidence` が (0.0, 1.0] の範囲外だった。
    #[error("confidence must be in (0.0, 1.0], got {value}")]
    InvalidConfidence {
        /// 入力された信頼度。
        value: f32,
    },
}

impl StopCondition {
    /// 停止条件の不変量を検証する。
    ///
    /// - `LoopCount { target }`: `target > 0`
    /// - `TemplateMatch { confidence, .. }`: `0.0 < confidence <= 1.0`
    /// - `Timeout { secs }`: `secs > 0`
    ///
    /// # Errors
    /// 不変量違反の場合、対応する [`GoalError`] バリアントを返す。
    pub fn validate(&self) -> Result<(), GoalError> {
        match self {
            Self::LoopCount { target } => {
                if *target == 0 {
                    return Err(GoalError::NonPositive { field: "target" });
                }
                Ok(())
            }
            Self::TemplateMatch { confidence, .. } => {
                if !(*confidence > 0.0 && *confidence <= 1.0) {
                    return Err(GoalError::InvalidConfidence { value: *confidence });
                }
                Ok(())
            }
            Self::Timeout { secs } => {
                if *secs == 0 {
                    return Err(GoalError::NonPositive { field: "secs" });
                }
                Ok(())
            }
        }
    }
}

/// 宣言的ゴール。1つの [`StopCondition`] を終端状態として保持する。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Goal {
    /// ゴール的人間可読名（レポート・ログ用）。
    pub name: String,
    /// 終端条件。
    pub stop: StopCondition,
}

impl Goal {
    /// 停止条件を検証する（[`StopCondition::validate`] への転送）。
    ///
    /// # Errors
    /// [`StopCondition::validate`] が返す [`GoalError`] をそのまま返す。
    pub fn validate(&self) -> Result<(), GoalError> {
        self.stop.validate()
    }
}

/// ゴール評価時に参照する、ループの進行状況。
///
/// `iterations` は **tick 数**（`evaluate` 呼出毎に +1 される）。
/// 認識成功・NoMatch・エラーを含む全反復を数える（モジュール doc 参照）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GoalStatusContext {
    /// これまでの評価回数（tick 数）。
    pub iterations: u64,
    /// 最後にマッチしたテンプレート情報（タスク名、信頼度、領域）。
    /// まだ一度もマッチしていない場合は `None`。
    pub last_match: Option<(String, f32, ScreenRegion)>,
}

impl GoalStatusContext {
    /// 空コンテキスト（0反復、マッチ無し）を作成する。
    pub fn new() -> Self {
        Self::default()
    }

    /// 反復数を1つ進め、必要に応じて `last_match` を更新する。
    ///
    /// `last_match` は `Some` の場合常に上書きし、直近のマッチを保持する。
    pub fn tick(&mut self, last_match: Option<(String, f32, ScreenRegion)>) {
        self.iterations = self.iterations.saturating_add(1);
        if let Some(m) = last_match {
            self.last_match = Some(m);
        }
    }
}

/// ゴール評価の結果としての終端状態。
///
/// pure 関数 [`evaluate`] が返す。上位層はこれを見て構造化終了する。
#[derive(Debug, Clone, PartialEq)]
pub enum GoalStatus {
    /// まだ終端に到達していない。ループを継続する。
    NotYet,
    /// ゴールに到達した（正常終端）。レポートを含む。
    Reached(GoalReport),
    /// ゴール評価が失敗した（異常終端）。レポートを含む。
    Failed(GoalReport),
}

/// ゴール到達/失敗時の構造化レポート。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalReport {
    /// 評価対象だったゴール名。
    pub goal: String,
    /// 到達時の反復数（tick 数）。
    pub iterations: u64,
    /// 終端理由の人間可読記述。
    pub reason: String,
    /// 最後にマッチしたテンプレート情報（任意）。
    pub last_match: Option<(String, f32, ScreenRegion)>,
}

/// ゴールを現在のコンテキストに対して純粋評価する。
///
/// 入力も出力も値のみ。I/O・時間・乱数に依存しない（`Timeout` は
/// 与えられた `elapsed_secs` と宣言値の比較のみを行う）。
///
/// # 引数
/// - `goal`: 評価対象のゴール。
/// - `ctx`: 現在の進行状況。
/// - `elapsed_secs`: タイムアウト判定用の経過秒数（呼出側が計測）。
///
/// # 戻り値
/// - `Reached`: 停止条件を満たした。
/// - `Failed`: `Timeout` で `elapsed_secs >= secs` に到達。
///   （`LoopCount`/`TemplateMatch` の到達は正常終端 `Reached` 扱い。）
/// - `NotYet`: まだ継続。
pub fn evaluate(goal: &Goal, ctx: &GoalStatusContext, elapsed_secs: u64) -> GoalStatus {
    match &goal.stop {
        StopCondition::LoopCount { target } => {
            if ctx.iterations >= *target {
                GoalStatus::Reached(GoalReport {
                    goal: goal.name.clone(),
                    iterations: ctx.iterations,
                    reason: format!("reached loop_count target ({target})"),
                    last_match: ctx.last_match.clone(),
                })
            } else {
                GoalStatus::NotYet
            }
        }
        StopCondition::TemplateMatch { task, confidence } => {
            if let Some((name, conf, _region)) = &ctx.last_match
                && name == task
                && *conf >= *confidence
            {
                return GoalStatus::Reached(GoalReport {
                    goal: goal.name.clone(),
                    iterations: ctx.iterations,
                    reason: format!(
                        "template '{task}' matched at confidence {conf} >= {confidence}"
                    ),
                    last_match: ctx.last_match.clone(),
                });
            }
            GoalStatus::NotYet
        }
        StopCondition::Timeout { secs } => {
            if elapsed_secs >= *secs {
                GoalStatus::Failed(GoalReport {
                    goal: goal.name.clone(),
                    iterations: ctx.iterations,
                    reason: format!("timeout after {elapsed_secs}s (limit {secs}s)"),
                    last_match: ctx.last_match.clone(),
                })
            } else {
                GoalStatus::NotYet
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    fn region() -> ScreenRegion {
        ScreenRegion::new(10, 20, 100, 50)
    }

    // ===== validation: StopCondition =====

    #[test]
    fn validate_loop_count_ok() {
        let s = StopCondition::LoopCount { target: 50 };
        assert!(s.validate().is_ok());
    }

    #[test]
    fn validate_loop_count_zero_rejected() {
        let s = StopCondition::LoopCount { target: 0 };
        let err = s.validate().unwrap_err();
        assert_eq!(err, GoalError::NonPositive { field: "target" });
    }

    #[test]
    fn validate_template_match_ok() {
        let s = StopCondition::TemplateMatch {
            task: "clear_dungeon".to_string(),
            confidence: 0.85,
        };
        assert!(s.validate().is_ok());
    }

    #[test]
    fn validate_template_match_confidence_one_ok() {
        let s = StopCondition::TemplateMatch {
            task: "t".to_string(),
            confidence: 1.0,
        };
        assert!(s.validate().is_ok());
    }

    #[test]
    fn validate_template_match_confidence_zero_rejected() {
        let s = StopCondition::TemplateMatch {
            task: "t".to_string(),
            confidence: 0.0,
        };
        let err = s.validate().unwrap_err();
        assert_eq!(err, GoalError::InvalidConfidence { value: 0.0 });
    }

    #[test]
    fn validate_template_match_confidence_negative_rejected() {
        let s = StopCondition::TemplateMatch {
            task: "t".to_string(),
            confidence: -0.1,
        };
        assert!(matches!(
            s.validate().unwrap_err(),
            GoalError::InvalidConfidence { .. }
        ));
    }

    #[test]
    fn validate_template_match_confidence_over_one_rejected() {
        let s = StopCondition::TemplateMatch {
            task: "t".to_string(),
            confidence: 1.5,
        };
        assert!(matches!(
            s.validate().unwrap_err(),
            GoalError::InvalidConfidence { .. }
        ));
    }

    #[test]
    fn validate_timeout_ok() {
        let s = StopCondition::Timeout { secs: 3600 };
        assert!(s.validate().is_ok());
    }

    #[test]
    fn validate_timeout_zero_rejected() {
        let s = StopCondition::Timeout { secs: 0 };
        let err = s.validate().unwrap_err();
        assert_eq!(err, GoalError::NonPositive { field: "secs" });
    }

    #[test]
    fn goal_validate_delegates_to_stop() {
        let goal = Goal {
            name: "g".to_string(),
            stop: StopCondition::LoopCount { target: 0 },
        };
        assert!(goal.validate().is_err());
    }

    // ===== serde: deny_unknown_fields =====

    #[test]
    fn deserialize_loop_count() {
        let toml = r#"
        name = "farm50"
        [stop]
        LoopCount = { target = 50 }
        "#;
        let goal: Goal = toml::from_str(toml).unwrap();
        assert_eq!(goal.name, "farm50");
        assert_eq!(goal.stop, StopCondition::LoopCount { target: 50 });
    }

    #[test]
    fn deserialize_template_match() {
        let toml = r#"
        name = "find_clear"
        [stop.TemplateMatch]
        task = "clear"
        confidence = 0.85
        "#;
        let goal: Goal = toml::from_str(toml).unwrap();
        assert_eq!(
            goal.stop,
            StopCondition::TemplateMatch {
                task: "clear".to_string(),
                confidence: 0.85
            }
        );
    }

    #[test]
    fn deserialize_timeout() {
        let toml = r#"
        name = "one_hour"
        [stop]
        Timeout = { secs = 3600 }
        "#;
        let goal: Goal = toml::from_str(toml).unwrap();
        assert_eq!(goal.stop, StopCondition::Timeout { secs: 3600 });
    }

    #[test]
    fn deserialize_unknown_top_field_rejected() {
        let toml = r#"
        name = "g"
        bogus = true
        [stop]
        LoopCount = { target = 1 }
        "#;
        assert!(toml::from_str::<Goal>(toml).is_err());
    }

    #[test]
    fn deserialize_unknown_stop_field_rejected() {
        let toml = r#"
        name = "g"
        [stop.LoopCount]
        target = 1
        extra = 2
        "#;
        assert!(toml::from_str::<Goal>(toml).is_err());
    }

    // ===== GoalStatusContext =====

    #[test]
    fn context_new_starts_empty() {
        let ctx = GoalStatusContext::new();
        assert_eq!(ctx.iterations, 0);
        assert!(ctx.last_match.is_none());
    }

    #[test]
    fn context_tick_increments_iterations() {
        let mut ctx = GoalStatusContext::new();
        ctx.tick(None);
        ctx.tick(None);
        assert_eq!(ctx.iterations, 2);
        assert!(ctx.last_match.is_none());
    }

    #[test]
    fn context_tick_records_last_match() {
        let mut ctx = GoalStatusContext::new();
        ctx.tick(Some(("task_a".to_string(), 0.9, region())));
        ctx.tick(None);
        // last_match は None で上書きされない
        assert_eq!(ctx.last_match, Some(("task_a".to_string(), 0.9, region())));
    }

    #[test]
    fn context_tick_updates_to_latest_match() {
        let mut ctx = GoalStatusContext::new();
        ctx.tick(Some(("a".to_string(), 0.5, region())));
        ctx.tick(Some(("b".to_string(), 0.95, region())));
        assert_eq!(ctx.last_match.as_ref().map(|m| m.0.as_str()), Some("b"));
    }

    #[test]
    fn context_tick_saturates_on_overflow() {
        let mut ctx = GoalStatusContext {
            iterations: u64::MAX,
            last_match: None,
        };
        ctx.tick(None);
        assert_eq!(ctx.iterations, u64::MAX);
    }

    // ===== evaluate: LoopCount (UC-1) =====

    #[test]
    fn evaluate_loop_count_not_yet() {
        let goal = Goal {
            name: "farm".to_string(),
            stop: StopCondition::LoopCount { target: 50 },
        };
        let ctx = GoalStatusContext {
            iterations: 49,
            last_match: None,
        };
        assert_eq!(evaluate(&goal, &ctx, 0), GoalStatus::NotYet);
    }

    #[test]
    fn evaluate_loop_count_reached() {
        let goal = Goal {
            name: "farm".to_string(),
            stop: StopCondition::LoopCount { target: 50 },
        };
        let ctx = GoalStatusContext {
            iterations: 50,
            last_match: Some(("x".to_string(), 0.7, region())),
        };
        match evaluate(&goal, &ctx, 0) {
            GoalStatus::Reached(report) => {
                assert_eq!(report.goal, "farm");
                assert_eq!(report.iterations, 50);
                assert!(report.reason.contains("50"));
                assert_eq!(report.last_match, Some(("x".to_string(), 0.7, region())));
            }
            other => panic!("expected Reached, got {other:?}"),
        }
    }

    #[test]
    fn evaluate_loop_count_reached_at_target() {
        // target 1: 1 tick で到達
        let goal = Goal {
            name: "once".to_string(),
            stop: StopCondition::LoopCount { target: 1 },
        };
        let ctx = GoalStatusContext {
            iterations: 1,
            last_match: None,
        };
        assert!(matches!(evaluate(&goal, &ctx, 0), GoalStatus::Reached(_)));
    }

    // ===== evaluate: TemplateMatch (UC-2) =====

    #[test]
    fn evaluate_template_match_not_yet_no_match() {
        let goal = Goal {
            name: "find".to_string(),
            stop: StopCondition::TemplateMatch {
                task: "clear".to_string(),
                confidence: 0.85,
            },
        };
        let ctx = GoalStatusContext::new();
        assert_eq!(evaluate(&goal, &ctx, 0), GoalStatus::NotYet);
    }

    #[test]
    fn evaluate_template_match_not_yet_wrong_task() {
        let goal = Goal {
            name: "find".to_string(),
            stop: StopCondition::TemplateMatch {
                task: "clear".to_string(),
                confidence: 0.85,
            },
        };
        let ctx = GoalStatusContext {
            iterations: 5,
            last_match: Some(("other".to_string(), 0.99, region())),
        };
        assert_eq!(evaluate(&goal, &ctx, 0), GoalStatus::NotYet);
    }

    #[test]
    fn evaluate_template_match_not_yet_low_confidence() {
        let goal = Goal {
            name: "find".to_string(),
            stop: StopCondition::TemplateMatch {
                task: "clear".to_string(),
                confidence: 0.85,
            },
        };
        let ctx = GoalStatusContext {
            iterations: 5,
            last_match: Some(("clear".to_string(), 0.84, region())),
        };
        assert_eq!(evaluate(&goal, &ctx, 0), GoalStatus::NotYet);
    }

    #[test]
    fn evaluate_template_match_reached_at_threshold() {
        let goal = Goal {
            name: "find".to_string(),
            stop: StopCondition::TemplateMatch {
                task: "clear".to_string(),
                confidence: 0.85,
            },
        };
        let ctx = GoalStatusContext {
            iterations: 7,
            last_match: Some(("clear".to_string(), 0.85, region())),
        };
        match evaluate(&goal, &ctx, 0) {
            GoalStatus::Reached(report) => {
                assert_eq!(report.iterations, 7);
                assert!(report.reason.contains("clear"));
                assert!(report.reason.contains("0.85"));
            }
            other => panic!("expected Reached, got {other:?}"),
        }
    }

    #[test]
    fn evaluate_template_match_reached_above_threshold() {
        let goal = Goal {
            name: "find".to_string(),
            stop: StopCondition::TemplateMatch {
                task: "clear".to_string(),
                confidence: 0.85,
            },
        };
        let ctx = GoalStatusContext {
            iterations: 3,
            last_match: Some(("clear".to_string(), 0.99, region())),
        };
        assert!(matches!(evaluate(&goal, &ctx, 0), GoalStatus::Reached(_)));
    }

    // ===== evaluate: Timeout (UC-3) =====

    #[test]
    fn evaluate_timeout_not_yet() {
        let goal = Goal {
            name: "limit".to_string(),
            stop: StopCondition::Timeout { secs: 60 },
        };
        let ctx = GoalStatusContext {
            iterations: 100,
            last_match: None,
        };
        assert_eq!(evaluate(&goal, &ctx, 59), GoalStatus::NotYet);
    }

    #[test]
    fn evaluate_timeout_failed_at_limit() {
        let goal = Goal {
            name: "limit".to_string(),
            stop: StopCondition::Timeout { secs: 60 },
        };
        let ctx = GoalStatusContext {
            iterations: 120,
            last_match: Some(("partial".to_string(), 0.6, region())),
        };
        match evaluate(&goal, &ctx, 60) {
            GoalStatus::Failed(report) => {
                assert_eq!(report.goal, "limit");
                assert_eq!(report.iterations, 120);
                assert!(report.reason.contains("60"));
                assert_eq!(
                    report.last_match,
                    Some(("partial".to_string(), 0.6, region()))
                );
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn evaluate_timeout_failed_after_limit() {
        let goal = Goal {
            name: "limit".to_string(),
            stop: StopCondition::Timeout { secs: 60 },
        };
        let ctx = GoalStatusContext::new();
        assert!(matches!(evaluate(&goal, &ctx, 120), GoalStatus::Failed(_)));
    }

    #[test]
    fn evaluate_timeout_includes_progress_in_report() {
        // UC-3: タイムアウト時に進捗レポートが含まれる
        let goal = Goal {
            name: "limit".to_string(),
            stop: StopCondition::Timeout { secs: 30 },
        };
        let ctx = GoalStatusContext {
            iterations: 42,
            last_match: Some(("boss_half".to_string(), 0.71, region())),
        };
        let report = match evaluate(&goal, &ctx, 30) {
            GoalStatus::Failed(r) => r,
            other => panic!("expected Failed, got {other:?}"),
        };
        assert_eq!(report.iterations, 42);
        assert_eq!(
            report.last_match,
            Some(("boss_half".to_string(), 0.71, region()))
        );
    }
}
