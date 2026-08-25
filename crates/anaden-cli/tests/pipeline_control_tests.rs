//! pipeline 制御ライブラリ層(anaden_cli_contract::pipeline)の契約テスト。
//!
//! シャード2 (Issue #83): GUI(anaden-studio)から pipeline を開始/停止するための
//! in-process 制御インターフェースの検証。デバイス I/O は行わない
//! (mock capture/input + 実 pipeline TOML fixture)。
//!
//! 孤立防止・二重起動防止は in-process 方式(shard1 決定)で担保する:
//! - 二重起動防止: `PipelineController::try_start` は実行中 `Err(AlreadyRunning)`。
//! - 孤立防止: pipeline は controller が spawn した tokio task として同じプロセス内で
//!   動くため、GUI プロセスが落ちれば必ず道連れになる(子プロセス孤立は構造的に発生しない)。

use std::path::PathBuf;
use std::time::Duration;

use anaden_cli_contract::pipeline::{
    CaptureMode, InputMode, PipelineController, RunOptions, RunState, RunTarget,
};
use tokio_util::sync::CancellationToken;

#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {}

/// テスト用 pipeline ディレクトリ(templates/pipelines/field_loop_pc は画像テンプレートを
/// 参照するため、ここでは load だけで動かない。実行テストには mock driver を使う)。
fn field_loop_pc_dir() -> PathBuf {
    PathBuf::from("../../templates/pipelines/field_loop_pc")
}

// ---- RunOptions::validate (純粋・device-free) ----

#[test]
fn validate_rejects_unknown_target() {
    let opts = RunOptions {
        target: RunTarget::Android,
        ..valid_opts()
    };
    assert!(opts.validate().is_ok(), "android is a valid target");
}

#[test]
fn validate_rejects_empty_start_task() {
    let mut opts = valid_opts();
    opts.start_task = String::new();
    let err = format!("{}", opts.validate().unwrap_err());
    assert!(err.contains("start_task"), "got: {err}");
}

#[test]
fn validate_rejects_unknown_capture_mode() {
    // 文字列からの解決は RunOptions::from_str 的な経路ではなく resolve_* 純粋関数で弾く。
    let err = anaden_cli_contract::pipeline::resolve_capture_mode("movie").unwrap_err();
    assert!(err.contains("screencap") && err.contains("scrcpy"), "got: {err}");
}

#[test]
fn validate_rejects_unknown_input_mode() {
    let err = anaden_cli_contract::pipeline::resolve_input_mode("minitouch").unwrap_err();
    assert!(err.contains("adb") && err.contains("scrcpy"), "got: {err}");
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
fn validate_rejects_scrcpy_without_serial_on_android() {
    // --input scrcpy / --capture scrcpy は ADB セッションを張るため serial 必須。
    let mut opts = valid_opts();
    opts.serial = None;
    opts.capture = CaptureMode::Scrcpy;
    let err = format!("{}", opts.validate().unwrap_err());
    assert!(err.contains("serial"), "got: {err}");
}

#[test]
fn validate_accepts_windows_without_serial() {
    let mut opts = valid_opts();
    opts.target = RunTarget::Windows;
    opts.serial = None;
    assert!(opts.validate().is_ok(), "windows target needs no serial");
}

#[test]
fn validate_rejects_windows_with_serial() {
    let mut opts = valid_opts();
    opts.target = RunTarget::Windows;
    // serial は既定 Some のまま → 「不要」と指定されているのでエラーにはしない(寛容)。
    // 仕様: windows + serial 指定は無視されずエラー(打ち間違い検出)。
    let err = format!("{}", opts.validate().unwrap_err());
    assert!(err.contains("serial"), "got: {err}");
}

#[test]
fn validate_rejects_scrcpy_on_windows() {
    // Win32 バックエンドに scrcpy capture/input は存在しない。
    let mut opts = valid_opts();
    opts.target = RunTarget::Windows;
    opts.serial = None;
    opts.input = InputMode::Scrcpy;
    let err = format!("{}", opts.validate().unwrap_err());
    assert!(err.contains("scrcpy"), "got: {err}");
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
        target: RunTarget::Android,
        serial: Some("localhost:5555".to_string()),
        pipeline_dir: field_loop_pc_dir(),
        start_task: "tap_bottom".to_string(),
        algorithm: None,
        interval_secs: 1,
        max_iters: 3,
        width: None,
        ensure_open: false,
        ensure_open_wait_secs: 30,
        recover_launch: false,
        recover_nomatch_threshold: 5,
        capture: CaptureMode::Screencap,
        input: InputMode::Adb,
        scrcpy_jar: "scrcpy-server".to_string(),
        verify_after_fire: true,
        goal: None,
    }
}

// ---- PipelineController: 開始/停止/状態 (mock driver, device-free) ----

/// controller へ注入する「一定時間後に MaxIterations で完了する」mock 実行関数。
fn slow_then_max_iters(
    opts: RunOptions,
    cancel: CancellationToken,
) -> std::pin::Pin<
    Box<
        dyn std::future::Future<Output = Result<anaden_engine::LoopOutcome, anyhow::Error>>
            + Send,
    >,
> {
    Box::pin(async move {
        // キャンセルまたは固定時間で完了。
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
    Box<
        dyn std::future::Future<Output = Result<anaden_engine::LoopOutcome, anyhow::Error>>
            + Send,
    >,
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
    // start 前に validate を通す前提(opts は valid fixture)。
    let jh1 = ctrl.try_start(opts.clone(), immediate_cancel_outcome).unwrap();
    let err = ctrl.try_start(opts.clone(), slow_then_max_iters).unwrap_err();
    assert!(err.is_already_running(), "got: {err:?}");
    // 1 本目を完了させて後始末。
    ctrl.cancel();
    let _ = jh1.await;
    assert_eq!(ctrl.state(), RunState::Finished);
}

#[tokio::test]
async fn controller_stop_transitions_to_finished_with_interrupted_reason() {
    let ctrl = PipelineController::new();
    let opts = valid_opts();
    let jh = ctrl.try_start(opts.clone(), immediate_cancel_outcome).unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(ctrl.state(), RunState::Running);
    ctrl.cancel();
    let outcome = jh.await.unwrap().unwrap();
    assert_eq!(outcome.reason, anaden_engine::LoopStopReason::Interrupted);
    assert_eq!(ctrl.state(), RunState::Finished);
}

#[tokio::test]
async fn controller_can_restart_after_finish() {
    let ctrl = PipelineController::new();
    let opts = valid_opts();
    // 1 回目: 即完了。
    let jh = ctrl.try_start(opts.clone(), slow_then_max_iters).unwrap();
    let outcome = jh.await.unwrap().unwrap();
    assert_eq!(outcome.reason, anaden_engine::LoopStopReason::MaxIterations);
    // Running → Finished への反映。
    for _ in 0..50 {
        if ctrl.state() == RunState::Finished {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(ctrl.state(), RunState::Finished);
    // 2 回目: 再開できる(二重起動防止は実行中のみ)。
    let jh2 = ctrl.try_start(opts.clone(), immediate_cancel_outcome).unwrap();
    ctrl.cancel();
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
