#![allow(clippy::unwrap_used)]
#![allow(clippy::expect_used)]
#![allow(clippy::panic)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anaden_core::{InputAction, ScreenRegion};
use anaden_device::DeviceError;
use anaden_engine::{Capture, Input, PipelineDriver, PipelineState, StepOutcome};
use anaden_vision::{Action, Algorithm, TaskDef};
use async_trait::async_trait;
use image::{DynamicImage, GrayImage, Luma};

const FULL_W: u32 = 1258;
const FULL_H: u32 = 708;
const NEEDLE_PX: u32 = 16;

struct FakeCapture {
    frames: Arc<Mutex<Vec<DynamicImage>>>,
}

#[async_trait]
impl Capture for FakeCapture {
    async fn capture(&self) -> Result<DynamicImage, DeviceError> {
        let mut frames = self.frames.lock().expect("frames lock");
        if frames.is_empty() {
            return Err(DeviceError::CommandFailed {
                message: "replay ran out of frames".into(),
            });
        }
        Ok(frames.remove(0))
    }
}

struct FakeInput {
    fired: Arc<Mutex<Vec<InputAction>>>,
}

#[async_trait]
impl Input for FakeInput {
    async fn execute(&self, action: &InputAction) -> Result<(), DeviceError> {
        self.fired.lock().expect("fired lock").push(action.clone());
        Ok(())
    }
}

fn needle_a() -> GrayImage {
    let mut img = GrayImage::new(NEEDLE_PX, NEEDLE_PX);
    for y in 0..NEEDLE_PX {
        for x in 0..NEEDLE_PX {
            let v = ((x + y) % 64) as u8;
            img.put_pixel(x, y, Luma([v]));
        }
    }
    img
}

fn needle_b() -> GrayImage {
    let mut img = GrayImage::new(NEEDLE_PX, NEEDLE_PX);
    for y in 0..NEEDLE_PX {
        for x in 0..NEEDLE_PX {
            let v = if ((x / 4) + (y / 4)) % 2 == 0 {
                255u8
            } else {
                0u8
            };
            img.put_pixel(x, y, Luma([v]));
        }
    }
    img
}

fn needle_chain() -> GrayImage {
    let mut img = GrayImage::new(NEEDLE_PX, NEEDLE_PX);
    for y in 0..NEEDLE_PX {
        for x in 0..NEEDLE_PX {
            let v = if x < NEEDLE_PX / 2 {
                255u8
            } else {
                ((y * 8) % 200) as u8
            };
            img.put_pixel(x, y, Luma([v]));
        }
    }
    img
}

fn canvas(embeddings: &[(&GrayImage, u32, u32)]) -> DynamicImage {
    let mut img = GrayImage::from_pixel(FULL_W, FULL_H, Luma([128]));
    for (needle, ox, oy) in embeddings {
        for y in 0..needle.height() {
            for x in 0..needle.width() {
                let p = needle.get_pixel(x, y)[0];
                img.put_pixel(ox + x, oy + y, Luma([p]));
            }
        }
    }
    DynamicImage::ImageLuma8(img)
}

fn persisted_template(needle: &GrayImage) -> PathBuf {
    let tmp = tempfile::tempdir().expect("tempdir");
    let p = tmp.path().join("needle.png");
    needle.save(&p).expect("save png");
    let _persisted = tmp.keep();
    p
}

fn candidate_task(
    name: &str,
    template: PathBuf,
    action: Option<Action>,
    next: Option<Vec<&str>>,
) -> TaskDef {
    TaskDef {
        name: name.into(),
        state: name.into(),
        algorithm: Algorithm::Ccoeff,
        template,
        roi: None,
        threshold: 0.9,
        base: None,
        action,
        next: next.map(|v| v.into_iter().map(String::from).collect()),
        candidates: None,
    }
}

fn dispatcher(candidates: Vec<&str>, own_template: PathBuf) -> TaskDef {
    TaskDef {
        name: "Dispatcher".into(),
        state: "Dispatcher".into(),
        algorithm: Algorithm::Ccoeff,
        template: own_template,
        roi: None,
        threshold: 0.9,
        base: None,
        action: Some(Action::ClickRect {
            roi: ScreenRegion::new(520, 320, 240, 80),
        }),
        next: Some(vec!["ViaSelf".into()]),
        candidates: Some(candidates.into_iter().map(String::from).collect()),
    }
}

fn driver_with(
    frames: Vec<DynamicImage>,
    tasks: Vec<TaskDef>,
) -> (
    PipelineDriver<FakeCapture, FakeInput>,
    Arc<Mutex<Vec<InputAction>>>,
) {
    let fired = Arc::new(Mutex::new(Vec::new()));
    let driver = PipelineDriver::new(
        FakeCapture {
            frames: Arc::new(Mutex::new(frames)),
        },
        FakeInput {
            fired: fired.clone(),
        },
        PipelineState::new("Dispatcher"),
        tasks,
        FULL_W,
        FULL_H,
        300,
    );
    (driver, fired)
}

#[tokio::test]
async fn candidates_second_match_fires_and_advances_to_its_next() {
    let tpl_a = persisted_template(&needle_a());
    let tpl_b = persisted_template(&needle_b());
    let tasks = vec![
        dispatcher(vec!["CandA", "CandB"], tpl_a.clone()),
        candidate_task("CandA", tpl_a, None, Some(vec!["WouldBeWrong"])),
        candidate_task(
            "CandB",
            tpl_b,
            Some(Action::ClickRect {
                roi: ScreenRegion::new(520, 320, 240, 80),
            }),
            Some(vec!["ChainEntry"]),
        ),
    ];
    let frames = vec![canvas(&[(&needle_b(), 150, 75)])];
    let (mut driver, fired) = driver_with(frames, tasks);

    let outcome = driver.run_once().await;
    match outcome {
        StepOutcome::Fired {
            next_current,
            fired: cmd,
        } => {
            assert_eq!(next_current.as_deref(), Some("ChainEntry"));
            assert!(cmd.is_some(), "CandB ClickRect must emit a command");
        }
        other => panic!("expected Fired from the second candidate, got {other:?}"),
    }
    assert_eq!(driver.current(), "ChainEntry");
    assert_eq!(
        fired.lock().expect("fired lock").len(),
        1,
        "exactly one input for the single candidate fire"
    );
}

#[tokio::test]
async fn candidates_all_unmatched_keeps_current_as_no_match() {
    let tpl_a = persisted_template(&needle_a());
    let tpl_b = persisted_template(&needle_b());
    let tasks = vec![
        dispatcher(vec!["CandA", "CandB"], tpl_a.clone()),
        candidate_task("CandA", tpl_a, Some(Action::ClickSelf), Some(vec!["A1"])),
        candidate_task("CandB", tpl_b, Some(Action::ClickSelf), Some(vec!["B1"])),
    ];
    let frames = vec![canvas(&[])];
    let (mut driver, fired) = driver_with(frames, tasks);

    let outcome = driver.run_once().await;
    assert_eq!(outcome, StepOutcome::NoMatch);
    assert_eq!(
        driver.current(),
        "Dispatcher",
        "current unchanged on no match"
    );
    assert!(fired.lock().expect("fired lock").is_empty());
}

#[tokio::test]
async fn candidates_next_chain_keeps_advancing() {
    let tpl_a = persisted_template(&needle_a());
    let tpl_b = persisted_template(&needle_b());
    let tpl_chain = persisted_template(&needle_chain());
    let tasks = vec![
        dispatcher(vec!["CandA", "CandB"], tpl_a.clone()),
        candidate_task("CandA", tpl_a, None, Some(vec!["WouldBeWrong"])),
        candidate_task(
            "CandB",
            tpl_b,
            Some(Action::ClickSelf),
            Some(vec!["ChainStep"]),
        ),
        candidate_task(
            "ChainStep",
            tpl_chain.clone(),
            Some(Action::ClickSelf),
            Some(vec!["Terminal"]),
        ),
        candidate_task("Terminal", tpl_chain, None, None),
    ];
    let frames = vec![
        canvas(&[(&needle_b(), 150, 75)]),
        canvas(&[(&needle_chain(), 300, 150)]),
    ];
    let (mut driver, fired) = driver_with(frames, tasks);

    let first = driver.run_once().await;
    assert!(
        matches!(first, StepOutcome::Fired { .. }),
        "candidate fire expected, got {first:?}"
    );
    assert_eq!(driver.current(), "ChainStep");

    let second = driver.run_once().await;
    assert!(
        matches!(second, StepOutcome::Fired { .. }),
        "chain step fire expected, got {second:?}"
    );
    assert_eq!(driver.current(), "Terminal");
    assert_eq!(fired.lock().expect("fired lock").len(), 2);
}

#[tokio::test]
async fn candidates_skip_dispatcher_own_template() {
    let tpl_b = persisted_template(&needle_b());
    let tasks = vec![
        dispatcher(vec!["CandB"], tpl_b.clone()),
        candidate_task("CandB", tpl_b, None, Some(vec!["ViaCandidate"])),
    ];
    let frames = vec![canvas(&[(&needle_b(), 150, 75)])];
    let (mut driver, fired) = driver_with(frames, tasks);

    let outcome = driver.run_once().await;
    assert_eq!(
        driver.current(),
        "ViaCandidate",
        "candidate next must win over the dispatcher own template path"
    );
    match outcome {
        StepOutcome::NoFire { next_current } => {
            assert_eq!(next_current.as_deref(), Some("ViaCandidate"));
        }
        other => panic!("DoNothing candidate must not fire, got {other:?}"),
    }
    assert!(fired.lock().expect("fired lock").is_empty());
}

#[tokio::test]
async fn candidates_verify_checks_fired_candidate() {
    let tpl_a = persisted_template(&needle_a());
    let tpl_b = persisted_template(&needle_b());
    let tasks = vec![
        dispatcher(vec!["CandB"], tpl_a),
        candidate_task(
            "CandB",
            tpl_b,
            Some(Action::ClickSelf),
            Some(vec!["AfterCand"]),
        ),
    ];
    let frames = vec![
        canvas(&[(&needle_b(), 400, 300)]),
        canvas(&[(&needle_a(), 150, 75)]),
    ];
    let (driver, fired) = driver_with(frames, tasks);
    let mut driver = driver.with_verify(true);

    let outcome = driver.run_once_verified().await;
    match outcome {
        StepOutcome::Fired { next_current, .. } => {
            assert_eq!(next_current.as_deref(), Some("AfterCand"));
        }
        other => panic!("verify must pass once the fired candidate is gone, got {other:?}"),
    }
    assert_eq!(driver.current(), "AfterCand");
    assert_eq!(fired.lock().expect("fired lock").len(), 1);
}

#[test]
fn candidates_task_toml_round_trips() {
    let body = "name = 'Dispatcher'
state = 'Dispatcher'
algorithm = 'ccoeff'
template = 'needle.png'
threshold = 0.9
candidates = ['CandA', 'CandB']
action = { type = 'do_nothing' }
";
    let def: TaskDef = toml::from_str(body).expect("parse TaskDef with candidates");
    assert_eq!(
        def.candidates,
        Some(vec!["CandA".to_string(), "CandB".to_string()])
    );

    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("Dispatcher.toml");
    anaden_vision::save_task_def(&def, &path).expect("save TaskDef");
    let saved = std::fs::read_to_string(&path).expect("read saved toml");
    assert!(
        saved.contains("candidates"),
        "candidates key must persist: {saved}"
    );

    let reloaded: TaskDef = toml::from_str(&saved).expect("reload saved TaskDef");
    assert_eq!(def, reloaded);
}
