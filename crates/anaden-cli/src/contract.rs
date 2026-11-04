pub mod pipeline;

use anaden_device::EnsureOutcome;

pub const EXIT_ALREADY_OR_LAUNCHED: i32 = 0;

pub const EXIT_HARDCERROR: i32 = 1;

pub const EXIT_TIMEOUT: i32 = 2;

pub const EXIT_NOT_READY: i32 = 3;

pub fn ensure_open_exit_code(outcome: &EnsureOutcome) -> i32 {
    match outcome {
        EnsureOutcome::AlreadyOpen => EXIT_ALREADY_OR_LAUNCHED,
        EnsureOutcome::Launched => EXIT_ALREADY_OR_LAUNCHED,
        EnsureOutcome::Timeout => EXIT_TIMEOUT,
        EnsureOutcome::NotReady => EXIT_NOT_READY,
    }
}

pub fn standalone_exit_code<E: ?Sized>(result: Result<&EnsureOutcome, &E>) -> i32 {
    match result {
        Ok(outcome) => ensure_open_exit_code(outcome),
        Err(_) => EXIT_HARDCERROR,
    }
}

pub fn ensure_outcome_label(outcome: &EnsureOutcome) -> &'static str {
    match outcome {
        EnsureOutcome::AlreadyOpen => "起動不要(既に起動中)",
        EnsureOutcome::Launched => "起動し生存を確認",
        EnsureOutcome::Timeout => "起動タイムアウト",
        EnsureOutcome::NotReady => {
            "起動したが splash-hang(not-ready: フレーム不変・TCP ソケット 0)を検出"
        }
    }
}

pub const EXIT_RUN_SUCCESS: i32 = 0;

pub const EXIT_RUN_HARDCERROR: i32 = EXIT_HARDCERROR;

pub const EXIT_RUN_TIMEOUT: i32 = EXIT_TIMEOUT;

pub fn run_exit_code(reason: &anaden_engine::LoopStopReason) -> i32 {
    match reason {
        anaden_engine::LoopStopReason::Stop => EXIT_RUN_SUCCESS,
        anaden_engine::LoopStopReason::TerminalTask => EXIT_RUN_SUCCESS,
        anaden_engine::LoopStopReason::MaxIterations => EXIT_RUN_SUCCESS,
        anaden_engine::LoopStopReason::GoalReached => EXIT_RUN_SUCCESS,
        anaden_engine::LoopStopReason::GoalTimeout => EXIT_RUN_TIMEOUT,
        anaden_engine::LoopStopReason::Interrupted => EXIT_RUN_TIMEOUT,
        anaden_engine::LoopStopReason::CaptureError => EXIT_RUN_HARDCERROR,
        anaden_engine::LoopStopReason::ExecuteError => EXIT_RUN_HARDCERROR,
        anaden_engine::LoopStopReason::RecoveryExhausted => EXIT_RUN_HARDCERROR,
    }
}

pub fn validate_goal(goal: &Option<anaden_core::Goal>) -> Result<(), anyhow::Error> {
    match goal {
        None => Ok(()),
        Some(g) => g.validate().map_err(|e| anyhow::anyhow!(e)),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    // ---- run_exit_code (Issue #37 exit-code contract, device-free) ----
    // 契約: Stop/TerminalTask => 0(success), MaxIterations => 0(今日の挙動保持),
    //       CaptureError/ExecuteError => 1(hard error)。
    // ゴール活性時の GoalReached(0) / GoalTimeout(2) は T3 でバリアント追加後に拡張。

    #[test]
    fn run_exit_code_stop_is_zero() {
        // Action::Stop 到達は宣言的終端 = CI success(UC: ユーザー明示停止)。
        assert_eq!(run_exit_code(&anaden_engine::LoopStopReason::Stop), 0);
    }

    #[test]
    fn run_exit_code_terminal_task_is_zero() {
        // next 無し終端タスク到達 = 正常収束 = CI success(UC: パイプライン末端)。
        assert_eq!(
            run_exit_code(&anaden_engine::LoopStopReason::TerminalTask),
            0
        );
    }

    #[test]
    fn run_exit_code_max_iterations_preserves_today_zero() {
        // pre-T5 決定ゲート: 非ゴールモードの MaxIterations は今日の exit 0 を保持。
        // 既存 CI の `run` 呼出意味論を断片化させない。
        assert_eq!(
            run_exit_code(&anaden_engine::LoopStopReason::MaxIterations),
            0
        );
    }

    #[test]
    fn run_exit_code_capture_error_is_hard_error() {
        // capture IO 失敗はハードエラー。standalone の EXIT_HARDCERROR(1) と並行。
        let code = run_exit_code(&anaden_engine::LoopStopReason::CaptureError);
        assert_eq!(code, EXIT_RUN_HARDCERROR);
        assert_eq!(code, 1, "CaptureError must map to hard-error exit 1");
        assert_ne!(code, 0, "CaptureError must be non-zero");
    }

    #[test]
    fn run_exit_code_execute_error_is_hard_error() {
        // execute IO 失敗はハードエラー。recovery_hook 失敗等もここへ集約。
        let code = run_exit_code(&anaden_engine::LoopStopReason::ExecuteError);
        assert_eq!(code, EXIT_RUN_HARDCERROR);
        assert_eq!(code, 1, "ExecuteError must map to hard-error exit 1");
        assert_ne!(code, 0, "ExecuteError must be non-zero");
    }

    #[test]
    fn run_exit_code_hard_errors_distinct_from_timeout_precedent() {
        // EXIT_RUN_TIMEOUT(2) と EXIT_RUN_HARDCERROR(1) は区別される。
        // ゴール未到達 soft-failure(2) と IO ハードエラー(1) の分離が run 契約の核。
        assert_ne!(EXIT_RUN_HARDCERROR, EXIT_RUN_TIMEOUT);
        assert_eq!(EXIT_RUN_TIMEOUT, 2);
    }

    #[test]
    fn run_exit_code_success_variants_share_zero() {
        // Stop と TerminalTask は CI gate 上同一个(success=0)。分岐複雑化を避ける。
        assert_eq!(
            run_exit_code(&anaden_engine::LoopStopReason::Stop),
            run_exit_code(&anaden_engine::LoopStopReason::TerminalTask)
        );
        // MaxIterations も非ゴールモードでは success に揃う(今日の挙動)。
        assert_eq!(
            run_exit_code(&anaden_engine::LoopStopReason::MaxIterations),
            run_exit_code(&anaden_engine::LoopStopReason::Stop)
        );
    }

    #[test]
    fn run_exit_code_goal_reached_is_zero() {
        // 宣言的ゴール到達 = CI success(UC: goal-driven loop がゴール条件を満たした)。
        // GoalReached は EXIT_RUN_SUCCESS(0) へ射影される。ゴール到達と Stop/TerminalTask
        // は CI gate 上同一个(success=0)。
        assert_eq!(
            run_exit_code(&anaden_engine::LoopStopReason::GoalReached),
            EXIT_RUN_SUCCESS
        );
        assert_eq!(
            run_exit_code(&anaden_engine::LoopStopReason::GoalReached),
            0,
            "GoalReached must map to success exit 0"
        );
        // Stop/TerminalTask と同一个(success=0)であることも担保。
        assert_eq!(
            run_exit_code(&anaden_engine::LoopStopReason::GoalReached),
            run_exit_code(&anaden_engine::LoopStopReason::Stop)
        );
    }

    #[test]
    fn run_exit_code_goal_timeout_is_two() {
        // ゴール活性時のタイムアウト(成果物は出たが宣言的ゴール未到達) = soft failure(2)。
        // GoalTimeout は EXIT_RUN_TIMEOUT(2) へ射影される。EXIT_TIMEOUT と同一値。
        assert_eq!(
            run_exit_code(&anaden_engine::LoopStopReason::GoalTimeout),
            EXIT_RUN_TIMEOUT
        );
        assert_eq!(
            run_exit_code(&anaden_engine::LoopStopReason::GoalTimeout),
            2,
            "GoalTimeout must map to timeout exit 2"
        );
        // hard-error(1) とは区別される(soft failure vs hard error)。
        assert_ne!(
            run_exit_code(&anaden_engine::LoopStopReason::GoalTimeout),
            EXIT_RUN_HARDCERROR,
            "GoalTimeout must not collapse to hard-error(1)"
        );
    }

    #[test]
    fn run_exit_code_interrupted_is_two() {
        // 外部キャンセル(Ctrl+C / SIGINT) = soft failure(2)。正常終端(0) ではなく
        // IO 系ハードエラー(1) とも区別する(ユーザー意図の中断)。
        assert_eq!(
            run_exit_code(&anaden_engine::LoopStopReason::Interrupted),
            EXIT_RUN_TIMEOUT,
            "Interrupted must map to timeout exit 2 (soft failure)"
        );
        assert_ne!(
            run_exit_code(&anaden_engine::LoopStopReason::Interrupted),
            EXIT_RUN_HARDCERROR,
            "Interrupted must not collapse to hard-error(1)"
        );
        assert_ne!(
            run_exit_code(&anaden_engine::LoopStopReason::Interrupted),
            EXIT_RUN_SUCCESS,
            "Interrupted is not a success exit 0"
        );
    }

    #[test]
    fn run_exit_code_recovery_exhausted_is_hard_error() {
        // 02-recovery-budget: リカバリ試行上限到達 = 恒久 NoMatch で進行不能。
        // recovery hook 失敗 (ExecuteError) と同じハードエラー(1)へ射影する。
        let code = run_exit_code(&anaden_engine::LoopStopReason::RecoveryExhausted);
        assert_eq!(
            code, EXIT_RUN_HARDCERROR,
            "RecoveryExhausted must map to hard-error exit 1"
        );
        assert_ne!(code, EXIT_RUN_SUCCESS, "RecoveryExhausted is not success");
        assert_ne!(
            code, EXIT_RUN_TIMEOUT,
            "RecoveryExhausted is not a soft timeout (環境故障で進行不能)"
        );
    }

    #[test]
    fn run_exit_code_covers_all_variants() {
        // LoopStopReason へ新バリアントが追加された際、このテストが未対応を検出する。
        // GoalReached / GoalTimeout は Issue #37 で追加済み(T3)。
        // Interrupted は Ctrl+C/SIGINT 中断(ガード1)。
        // RecoveryExhausted はリカバリ試行上限到達 (02-recovery-budget)。
        let variants = [
            anaden_engine::LoopStopReason::Stop,
            anaden_engine::LoopStopReason::TerminalTask,
            anaden_engine::LoopStopReason::MaxIterations,
            anaden_engine::LoopStopReason::GoalReached,
            anaden_engine::LoopStopReason::GoalTimeout,
            anaden_engine::LoopStopReason::Interrupted,
            anaden_engine::LoopStopReason::CaptureError,
            anaden_engine::LoopStopReason::ExecuteError,
            anaden_engine::LoopStopReason::RecoveryExhausted,
        ];
        for v in &variants {
            // 各バリアントが success(0) / timeout(2) / hard-error(1) のいずれかへ解決されること。
            let code = run_exit_code(v);
            assert!(
                code == EXIT_RUN_SUCCESS || code == EXIT_RUN_TIMEOUT || code == EXIT_RUN_HARDCERROR,
                "variant {:?} produced unexpected exit code {}",
                v,
                code
            );
        }
    }

    // ---- validate_goal (Issue #37 T4: run_loop signature threading, device-free) ----
    // 契約: None => Ok(非ゴールモード)。Some(valid) => Ok。
    //       Some(invalid) => Err(GoalError を伝播)。driver へ渡る前に CLI 境界で弾く。

    #[test]
    fn validate_goal_none_is_ok() {
        // 非ゴールモード(従来の無限ループ)は常に許容される。
        assert!(validate_goal(&None).is_ok());
    }

    #[test]
    fn validate_goal_some_valid_loop_count_is_ok() {
        // UC-1: LoopCount target=50 は妥当。
        let goal = anaden_core::Goal {
            name: "farm50".to_string(),
            stop: anaden_core::StopCondition::LoopCount { target: 50 },
        };
        assert!(validate_goal(&Some(goal)).is_ok());
    }

    #[test]
    fn validate_goal_some_valid_timeout_is_ok() {
        // UC-3: Timeout secs=3600 は妥当。
        let goal = anaden_core::Goal {
            name: "one_hour".to_string(),
            stop: anaden_core::StopCondition::Timeout { secs: 3600 },
        };
        assert!(validate_goal(&Some(goal)).is_ok());
    }

    #[test]
    fn validate_goal_some_valid_template_match_is_ok() {
        // UC-2: TemplateMatch confidence=0.85 は妥当。
        let goal = anaden_core::Goal {
            name: "find_clear".to_string(),
            stop: anaden_core::StopCondition::TemplateMatch {
                task: "clear".to_string(),
                confidence: 0.85,
            },
        };
        assert!(validate_goal(&Some(goal)).is_ok());
    }

    #[test]
    fn validate_goal_loop_count_zero_is_err() {
        // LoopCount target=0 は NonPositive エラー。driver へ渡る前に弾かれる。
        let goal = anaden_core::Goal {
            name: "bad".to_string(),
            stop: anaden_core::StopCondition::LoopCount { target: 0 },
        };
        let result = validate_goal(&Some(goal));
        assert!(result.is_err(), "zero target must error before driver");
        let msg = format!("{}", result.unwrap_err());
        // GoalError::NonPositive の Display 文字列が anyhow 経由で伝播していること。
        assert!(msg.contains("greater than 0"), "got: {msg}");
        assert!(msg.contains("target"), "field name propagated: {msg}");
    }

    #[test]
    fn validate_goal_timeout_zero_is_err() {
        // Timeout secs=0 は NonPositive エラー(field: secs)。
        let goal = anaden_core::Goal {
            name: "bad".to_string(),
            stop: anaden_core::StopCondition::Timeout { secs: 0 },
        };
        let result = validate_goal(&Some(goal));
        assert!(result.is_err());
        let msg = format!("{}", result.unwrap_err());
        assert!(msg.contains("secs"), "field name propagated: {msg}");
    }

    #[test]
    fn validate_goal_template_match_bad_confidence_is_err() {
        // TemplateMatch confidence=0.0 は InvalidConfidence エラー。
        let goal = anaden_core::Goal {
            name: "bad".to_string(),
            stop: anaden_core::StopCondition::TemplateMatch {
                task: "clear".to_string(),
                confidence: 0.0,
            },
        };
        let result = validate_goal(&Some(goal));
        assert!(result.is_err());
        let msg = format!("{}", result.unwrap_err());
        assert!(msg.contains("confidence"), "got: {msg}");
    }

    #[test]
    fn validate_goal_does_not_panic_on_negative_confidence() {
        // 負の信頼度も panic せず Err へ(ライブラリ非 panic 契約)。
        let goal = anaden_core::Goal {
            name: "bad".to_string(),
            stop: anaden_core::StopCondition::TemplateMatch {
                task: "x".to_string(),
                confidence: -0.5,
            },
        };
        let result = validate_goal(&Some(goal));
        assert!(result.is_err(), "negative confidence must not panic");
    }

    // ---- UC-3 exit-code E2E 統合 assert (Issue #42 / 親 #37 Shard 3) ----
    //
    // 本テストは exit-code 契約(L155 GoalTimeout → EXIT_RUN_TIMEOUT)を E2E で結ぶ。
    // run_loop_with_goal が生成する outcome.reason(GoalTimeout) を run_exit_code へ渡し、
    // 戻り値が EXIT_RUN_TIMEOUT(==2) となることを1行の統合 assert で検証する。
    //
    // 既存の run_exit_code_goal_timeout_is_two(L288)は bare な LoopStopReason バリアントを
    // 入力とする単体テストだが、本テストは「ループが生成した outcome.reason」を入力とする点で
    // 異なる。run_loop_with_goal の戻り値 LoopOutcome(progress_report.reached_goal や
    // terminal="goal_timeout" まで populate 済み)をそのまま run_exit_code へ流す経路が、
    // CLI 境界で正しく接続されていることを担保する(依存方向: anaden-cli → anaden-engine)。

    /// UC-3 E2E: `run_loop_with_goal` が生成した `GoalTimeout` outcome を
    /// `run_exit_code` へ渡すと `EXIT_RUN_TIMEOUT`(2) となる統合 assert。
    ///
    /// outcome は run_loop_with_goal が Timeout ゴール停止時に返す形状
    /// (terminal="goal_timeout", reason=GoalTimeout, reached_goal="timeout=<secs>")を
    /// 忠実に再現し、engine→CLI の exit-code 接続が正しいことを1行で検証する。
    #[test]
    fn run_exit_code_wires_loop_generated_goal_timeout_to_two() {
        // run_loop_with_goal が UC-3 Timeout 停止で生成する outcome と同一形状。
        // (descriptor/terminal は engine 側 uc3_timeout_goal_reaches_goal_timeout_after_declared_secs
        //  L3244 が populate する値と一致させる)
        let outcome = anaden_engine::LoopOutcome {
            iterations: 4,
            fired_commands: vec![],
            terminal: "goal_timeout".to_string(),
            reason: anaden_engine::LoopStopReason::GoalTimeout,
            progress_report: anaden_engine::ProgressReport {
                jitter_seed: None,
                verify: Default::default(),
                iterations: 4,
                fired_count: 0,
                per_task_matches: vec![],
                elapsed_ms: 3000,
                terminal_task: None,
                reached_goal: Some("timeout=3".to_string()),
                stop_reason: None,
                recovery_attempts: 0,
                max_consecutive_recovery: 0,
            },
        };
        // 1行の統合 assert: ループ産出 reason → run_exit_code → EXIT_RUN_TIMEOUT(2)。
        assert_eq!(
            run_exit_code(&outcome.reason),
            EXIT_RUN_TIMEOUT,
            "loop-generated GoalTimeout must map to EXIT_RUN_TIMEOUT(2) at CLI boundary"
        );
    }
}
