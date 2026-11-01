use std::path::PathBuf;
use std::time::Duration;

use anaden_cli_contract::pipeline::{PipelineController, RunOptions, RunState};
use tokio_util::sync::CancellationToken;

#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {}

fn field_loop_pc_dir() -> PathBuf {
    PathBuf::from("../../templates/pipelines/field_loop_pc")
}

#[test]
fn validate_rejects_empty_start_task() {
    let mut opts = valid_opts();
    opts.start_task = String::new();
    let err = format!("{}", opts.validate().unwrap_err());
    assert!(err.contains("start_task"), "got: {err}");
}

#[test]
fn validate_rejects_unknown_algorithm() {
    let err = anaden_cli_contract::pipeline::resolve_algorithm("akaze").unwrap_err();
    assert!(err.contains("sse") && err.contains("ccoeff"), "got: {err}");
}

#[test]
fn validate_rejects_interval_zero() {
    let mut opts = valid_opts();
    opts.interval_secs = 0;
    let err = format!("{}", opts.validate().unwrap_err());
    assert!(err.contains("interval"), "got: {err}");
}

#[test]
fn validate_rejects_max_iters_zero() {
    let mut opts = valid_opts();
    opts.max_iters = 0;
    let err = format!("{}", opts.validate().unwrap_err());
    assert!(err.contains("max_iters"), "got: {err}");
}

#[test]
fn validate_propagates_invalid_goal() {
    let mut opts = valid_opts();
    opts.goal = Some(anaden_core::Goal {
        name: "bad".to_string(),
        stop: anaden_core::StopCondition::LoopCount { target: 0 },
    });
    let err = format!("{}", opts.validate().unwrap_err());
    assert!(err.contains("target"), "got: {err}");
}

#[test]
fn validate_checks_pipeline_dir_exists() {
    let mut opts = valid_opts();
    opts.pipeline_dir = PathBuf::from("Z:/no/such/dir");
    let err = format!("{}", opts.validate().unwrap_err());
    assert!(err.contains("pipeline"), "got: {err}");
}

#[test]
fn validate_checks_start_task_exists_in_pipeline() {
    let mut opts = valid_opts();
    opts.start_task = "no_such_task".to_string();
    let err = format!("{}", opts.validate().unwrap_err());
    assert!(err.contains("no_such_task"), "got: {err}");
}

fn valid_opts() -> RunOptions {
    RunOptions {
        pipeline_dir: field_loop_pc_dir(),
        start_task: "TapBottomStablePc".to_string(),
        algorithm: None,
        interval_secs: 1,
        max_iters: 3,
        width: None,
        ensure_open: false,
        ensure_open_wait_secs: 30,
        recover_launch: false,
        recover_nomatch_threshold: 5,
        verify_after_fire: true,
        goal: None,
    }
}

fn slow_then_max_iters(
    opts: RunOptions,
    cancel: CancellationToken,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<anaden_engine::LoopOutcome, anyhow::Error>> + Send>,
> {
    Box::pin(async move {
        tokio::select! {
            _ = cancel.cancelled() => {}
            _ = tokio::time::sleep(Duration::from_millis(200)) => {}
        }
        Ok(anaden_engine::LoopOutcome {
            iterations: opts.max_iters,
            fired_commands: vec![],
            terminal: "max_iterations".to_string(),
            reason: anaden_engine::LoopStopReason::MaxIterations,
            progress_report: anaden_engine::ProgressReport::default(),
        })
    })
}

fn immediate_cancel_outcome(
    _opts: RunOptions,
    cancel: CancellationToken,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<anaden_engine::LoopOutcome, anyhow::Error>> + Send>,
> {
    Box::pin(async move {
        cancel.cancelled().await;
        Ok(anaden_engine::LoopOutcome {
            iterations: 0,
            fired_commands: vec![],
            terminal: "interrupted".to_string(),
            reason: anaden_engine::LoopStopReason::Interrupted,
            progress_report: anaden_engine::ProgressReport::default(),
        })
    })
}

#[tokio::test]
async fn controller_initial_state_is_idle() {
    let ctrl = PipelineController::new();
    assert_eq!(ctrl.state(), RunState::Idle);
}

#[tokio::test]
async fn controller_rejects_second_start_while_running() {
    let ctrl = PipelineController::new();
    let opts = valid_opts();
    let jh1 = ctrl
        .try_start(opts.clone(), immediate_cancel_outcome)
        .unwrap();
    let err = ctrl
        .try_start(opts.clone(), slow_then_max_iters)
        .unwrap_err();
    assert!(err.is_already_running(), "got: {err:?}");
    ctrl.cancel().await;
    let _ = jh1.await;
    assert_eq!(ctrl.state(), RunState::Finished);
}

#[tokio::test]
async fn controller_stop_transitions_to_finished_with_interrupted_reason() {
    let ctrl = PipelineController::new();
    let opts = valid_opts();
    let jh = ctrl
        .try_start(opts.clone(), immediate_cancel_outcome)
        .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(ctrl.state(), RunState::Running);
    ctrl.cancel().await;
    let outcome = jh.await.unwrap().unwrap();
    assert_eq!(outcome.reason, anaden_engine::LoopStopReason::Interrupted);
    assert_eq!(ctrl.state(), RunState::Finished);
}

#[tokio::test]
async fn controller_can_restart_after_finish() {
    let ctrl = PipelineController::new();
    let opts = valid_opts();
    let jh = ctrl.try_start(opts.clone(), slow_then_max_iters).unwrap();
    let outcome = jh.await.unwrap().unwrap();
    assert_eq!(outcome.reason, anaden_engine::LoopStopReason::MaxIterations);
    for _ in 0..50 {
        if ctrl.state() == RunState::Finished {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(ctrl.state(), RunState::Finished);
    let jh2 = ctrl
        .try_start(opts.clone(), immediate_cancel_outcome)
        .unwrap();
    ctrl.cancel().await;
    let _ = jh2.await;
    assert_eq!(ctrl.state(), RunState::Finished);
}

#[tokio::test]
async fn controller_try_start_validates_options_first() {
    let ctrl = PipelineController::new();
    let mut opts = valid_opts();
    opts.start_task = String::new();
    let err = ctrl.try_start(opts, slow_then_max_iters).unwrap_err();
    assert!(!err.is_already_running());
    assert_eq!(ctrl.state(), RunState::Idle);
}
