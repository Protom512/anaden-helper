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

    #[test]
    fn run_exit_code_stop_is_zero() {
        assert_eq!(run_exit_code(&anaden_engine::LoopStopReason::Stop), 0);
    }

    #[test]
    fn run_exit_code_terminal_task_is_zero() {
        assert_eq!(
            run_exit_code(&anaden_engine::LoopStopReason::TerminalTask),
            0
        );
    }

    #[test]
    fn run_exit_code_max_iterations_preserves_today_zero() {
        assert_eq!(
            run_exit_code(&anaden_engine::LoopStopReason::MaxIterations),
            0
        );
    }

    #[test]
    fn run_exit_code_capture_error_is_hard_error() {
        let code = run_exit_code(&anaden_engine::LoopStopReason::CaptureError);
        assert_eq!(code, EXIT_RUN_HARDCERROR);
        assert_eq!(code, 1, "CaptureError must map to hard-error exit 1");
        assert_ne!(code, 0, "CaptureError must be non-zero");
    }

    #[test]
    fn run_exit_code_execute_error_is_hard_error() {
        let code = run_exit_code(&anaden_engine::LoopStopReason::ExecuteError);
        assert_eq!(code, EXIT_RUN_HARDCERROR);
        assert_eq!(code, 1, "ExecuteError must map to hard-error exit 1");
        assert_ne!(code, 0, "ExecuteError must be non-zero");
    }

    #[test]
    fn run_exit_code_hard_errors_distinct_from_timeout_precedent() {
        assert_ne!(EXIT_RUN_HARDCERROR, EXIT_RUN_TIMEOUT);
        assert_eq!(EXIT_RUN_TIMEOUT, 2);
    }

    #[test]
    fn run_exit_code_success_variants_share_zero() {
        assert_eq!(
            run_exit_code(&anaden_engine::LoopStopReason::Stop),
            run_exit_code(&anaden_engine::LoopStopReason::TerminalTask)
        );
        assert_eq!(
            run_exit_code(&anaden_engine::LoopStopReason::MaxIterations),
            run_exit_code(&anaden_engine::LoopStopReason::Stop)
        );
    }

    #[test]
    fn run_exit_code_goal_reached_is_zero() {
        assert_eq!(
            run_exit_code(&anaden_engine::LoopStopReason::GoalReached),
            EXIT_RUN_SUCCESS
        );
        assert_eq!(
            run_exit_code(&anaden_engine::LoopStopReason::GoalReached),
            0,
            "GoalReached must map to success exit 0"
        );
        assert_eq!(
            run_exit_code(&anaden_engine::LoopStopReason::GoalReached),
            run_exit_code(&anaden_engine::LoopStopReason::Stop)
        );
    }

    #[test]
    fn run_exit_code_goal_timeout_is_two() {
        assert_eq!(
            run_exit_code(&anaden_engine::LoopStopReason::GoalTimeout),
            EXIT_RUN_TIMEOUT
        );
        assert_eq!(
            run_exit_code(&anaden_engine::LoopStopReason::GoalTimeout),
            2,
            "GoalTimeout must map to timeout exit 2"
        );
        assert_ne!(
            run_exit_code(&anaden_engine::LoopStopReason::GoalTimeout),
            EXIT_RUN_HARDCERROR,
            "GoalTimeout must not collapse to hard-error(1)"
        );
    }

    #[test]
    fn run_exit_code_interrupted_is_two() {
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
            let code = run_exit_code(v);
            assert!(
                code == EXIT_RUN_SUCCESS || code == EXIT_RUN_TIMEOUT || code == EXIT_RUN_HARDCERROR,
                "variant {:?} produced unexpected exit code {}",
                v,
                code
            );
        }
    }

    #[test]
    fn validate_goal_none_is_ok() {
        assert!(validate_goal(&None).is_ok());
    }

    #[test]
    fn validate_goal_some_valid_loop_count_is_ok() {
        let goal = anaden_core::Goal {
            name: "farm50".to_string(),
            stop: anaden_core::StopCondition::LoopCount { target: 50 },
        };
        assert!(validate_goal(&Some(goal)).is_ok());
    }

    #[test]
    fn validate_goal_some_valid_timeout_is_ok() {
        let goal = anaden_core::Goal {
            name: "one_hour".to_string(),
            stop: anaden_core::StopCondition::Timeout { secs: 3600 },
        };
        assert!(validate_goal(&Some(goal)).is_ok());
    }

    #[test]
    fn validate_goal_some_valid_template_match_is_ok() {
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
        let goal = anaden_core::Goal {
            name: "bad".to_string(),
            stop: anaden_core::StopCondition::LoopCount { target: 0 },
        };
        let result = validate_goal(&Some(goal));
        assert!(result.is_err(), "zero target must error before driver");
        let msg = format!("{}", result.unwrap_err());
        assert!(msg.contains("greater than 0"), "got: {msg}");
        assert!(msg.contains("target"), "field name propagated: {msg}");
    }

    #[test]
    fn validate_goal_timeout_zero_is_err() {
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

    #[test]
    fn run_exit_code_wires_loop_generated_goal_timeout_to_two() {
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
        assert_eq!(
            run_exit_code(&outcome.reason),
            EXIT_RUN_TIMEOUT,
            "loop-generated GoalTimeout must map to EXIT_RUN_TIMEOUT(2) at CLI boundary"
        );
    }
}
