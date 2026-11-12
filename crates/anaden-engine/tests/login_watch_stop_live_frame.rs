#![allow(clippy::unwrap_used)]
#![allow(clippy::expect_used)]
#![allow(clippy::panic)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anaden_core::InputAction;
use anaden_device::DeviceError;
use anaden_engine::{Capture, Input, InputCommand, LoopStopReason, PipelineDriver, PipelineState};
use anaden_vision::{load_pipeline, load_watch_defs};
use async_trait::async_trait;
use image::DynamicImage;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

fn login_dir() -> PathBuf {
    workspace_root()
        .join("templates")
        .join("pipelines")
        .join("login")
}

struct OneFrameCapture {
    frame: Option<DynamicImage>,
}

#[async_trait]
impl Capture for OneFrameCapture {
    async fn capture(&self) -> Result<DynamicImage, DeviceError> {
        self.frame
            .clone()
            .ok_or_else(|| DeviceError::CommandFailed {
                message: "no more frames".into(),
            })
    }
}

#[derive(Default)]
struct RecordingInput {
    fired: Arc<Mutex<Vec<InputCommand>>>,
}

#[async_trait]
impl Input for RecordingInput {
    async fn execute(&self, action: &InputAction) -> Result<(), DeviceError> {
        match action {
            InputAction::Tap(p) => {
                self.fired
                    .lock()
                    .expect("fired lock")
                    .push(InputCommand::Tap { x: p.x, y: p.y });
                Ok(())
            }
            other => Err(DeviceError::CommandFailed {
                message: format!("unexpected action: {other:?}"),
            }),
        }
    }
}

fn clipped_arrival_frame() -> DynamicImage {
    let raw = image::open(
        workspace_root()
            .join("crates")
            .join("anaden-vision")
            .join("tests")
            .join("fixtures")
            .join("town_live_20261112_3783x2128.png"),
    )
    .expect("open tracked day-town fixture");
    let mut rgba = raw.to_rgba8();
    let height = rgba.height();
    for y in (height - 100)..height {
        for x in 0..rgba.width() {
            rgba.put_pixel(x, y, image::Rgba([0, 0, 0, 255]));
        }
    }
    DynamicImage::ImageRgba8(rgba)
}

#[tokio::test]
async fn login_watch_stops_the_loop_on_a_clipped_arrival_frame() {
    let dir = login_dir();
    let mut tasks = load_pipeline(&dir).expect("load login chain");
    tasks.extend(load_watch_defs(&dir).expect("load login watch defs"));
    let frame = clipped_arrival_frame();
    let (width, height) = (frame.width(), frame.height());

    let input = RecordingInput::default();
    let fired = input.fired.clone();
    let mut driver = PipelineDriver::new(
        OneFrameCapture { frame: Some(frame) },
        input,
        PipelineState::new("title_pc"),
        tasks,
        width,
        height,
        300,
    )
    .with_watch(vec!["LoginFieldTownBarPc".into()]);

    let outcome = driver.run_loop(Duration::ZERO, 3).await;
    assert_eq!(
        outcome.reason,
        LoopStopReason::Stop,
        "the arrival sentinel must stop the loop, got {:?}",
        outcome.reason
    );
    assert_eq!(outcome.terminal, "watch stop: LoginFieldTownBarPc");
    assert_eq!(outcome.iterations, 1, "watch is evaluated before the chain");
    assert!(
        fired.lock().expect("fired lock").is_empty(),
        "a Stop sentinel must not fire an input command"
    );
    assert_eq!(outcome.progress_report.fired_count, 0);
}
