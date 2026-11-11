use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use anaden_core::InputAction;
use anaden_device::DeviceError;
use anaden_engine::{
    Capture, Input, PipelineDriver, PipelineState, RecoveryContext, RecoveryHook, StepErrorKind,
    StepOutcome,
};
use anaden_vision::{Action, Algorithm, TaskDef};
use async_trait::async_trait;
use image::{DynamicImage, GrayImage, Luma};

struct FixedCapture(DynamicImage);

#[async_trait]
impl Capture for FixedCapture {
    async fn capture(&self) -> Result<DynamicImage, DeviceError> {
        Ok(self.0.clone())
    }
}

struct ScriptedInput {
    calls: AtomicUsize,
    fail_first_only: bool,
}

#[async_trait]
impl Input for ScriptedInput {
    async fn execute(&self, _action: &InputAction) -> Result<(), DeviceError> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fail_first_only && n > 0 {
            return Ok(());
        }
        Err(DeviceError::PlacementFault {
            message: "fire coordinates outside every monitor (test)".into(),
        })
    }
}

fn needle() -> GrayImage {
    let mut img = GrayImage::new(16, 16);
    for y in 0..16 {
        for x in 0..16 {
            let v = (((x * 13 + y * 29) % 251) as u8).saturating_add(2);
            img.put_pixel(x, y, Luma([v]));
        }
    }
    img
}

fn frame_with_needle() -> GrayImage {
    let mut img = GrayImage::from_pixel(1258, 708, Luma([128u8]));
    let n = needle();
    for y in 0..n.height() {
        for x in 0..n.width() {
            img.put_pixel(150 + x, 75 + y, *n.get_pixel(x, y));
        }
    }
    img
}

fn fixture() -> (tempfile::TempDir, Vec<TaskDef>, DynamicImage) {
    let dir = tempfile::tempdir().expect("tempdir");
    let tpl_path = dir.path().join("needle.png");
    needle().save(&tpl_path).expect("save needle");

    let tasks = vec![TaskDef {
        name: "A".into(),
        state: "S".into(),
        algorithm: Algorithm::Ccoeff,
        template: PathBuf::from(&tpl_path),
        roi: None,
        threshold: 0.9,
        base: None,
        action: Some(Action::ClickSelf),
        next: Some(vec!["B".into()]),
    }];
    (dir, tasks, DynamicImage::ImageLuma8(frame_with_needle()))
}

#[tokio::test]
async fn execute_error_keeps_the_matched_task_current() {
    let (_dir, tasks, frame) = fixture();
    let input = ScriptedInput {
        calls: AtomicUsize::new(0),
        fail_first_only: true,
    };
    let mut driver = PipelineDriver::new(
        FixedCapture(frame),
        input,
        PipelineState::new("A"),
        tasks,
        1258,
        708,
        300,
    );

    let first = driver.run_once().await;
    match first {
        StepOutcome::Error { kind, .. } => assert_eq!(kind, StepErrorKind::GateRejected),
        other => panic!("expected a GateRejected error, got {other:?}"),
    }
    assert_eq!(
        driver.current(),
        "A",
        "a rejected fire must not advance the state machine to the task we never reached"
    );

    let second = driver.run_once().await;
    assert!(
        matches!(second, StepOutcome::Fired { .. }),
        "the retry on the same frame must fire once the input path works: {second:?}"
    );
    assert_eq!(driver.current(), "B");
}

#[tokio::test]
async fn placement_recovery_does_not_arm_a_boot_grace() {
    let (_dir, tasks, frame) = fixture();
    let recovery_calls = Arc::new(AtomicUsize::new(0));
    let counter = recovery_calls.clone();
    let hook: RecoveryHook = Box::new(move |_ctx: RecoveryContext| {
        let counter = counter.clone();
        Box::pin(async move {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    });

    let input = ScriptedInput {
        calls: AtomicUsize::new(0),
        fail_first_only: false,
    };
    let mut driver = PipelineDriver::new(
        FixedCapture(frame),
        input,
        PipelineState::new("A"),
        tasks,
        1258,
        708,
        300,
    )
    .with_recovery_grace(Duration::from_secs(180))
    .with_max_recovery_attempts(10);

    let _outcome = driver
        .run_loop_with_recovery(Duration::ZERO, 3, 1, Some(hook))
        .await;

    assert_eq!(
        recovery_calls.load(Ordering::SeqCst),
        3,
        "a placement clamp must not open a 180s boot grace; every cycle's gate rejection \
         must reach the recovery hook (DEF-012 live run: 13 cycles, 1 recovery, 0 fires)"
    );
}
