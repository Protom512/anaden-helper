use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anaden_core::InputAction;
use anaden_device::DeviceError;
use anaden_engine::{Capture, Input, PipelineDriver, PipelineState, StepOutcome};
use anaden_vision::{
    ScreenScaler, TaskDef, load_pipeline, load_pipeline_manifest, load_watch_defs,
};
use async_trait::async_trait;
use image::DynamicImage;
use serde::Deserialize;

#[derive(Deserialize)]
struct Spec {
    schema: u32,
    task_dir: Vec<String>,
    unit: Vec<Unit>,
    blocked: Vec<Blocked>,
    gap: Vec<Gap>,
    replay: Vec<Replay>,
}

#[derive(Deserialize)]
struct Unit {
    name: String,
    fixture: String,
    expect: Vec<(String, f32)>,
}

#[derive(Deserialize)]
struct Blocked {
    name: String,
    fixture: String,
    reason: String,
    #[serde(default)]
    degenerate: bool,
}

#[derive(Deserialize)]
struct Gap {
    name: String,
    tasks: Vec<String>,
    fixtures: Vec<String>,
    reason: String,
}

#[derive(Deserialize)]
struct Replay {
    name: String,
    pipeline: String,
    start_task: String,
    watch_task: String,
    frames: Vec<String>,
    step: Vec<Step>,
}

#[derive(Deserialize)]
struct Step {
    outcome: String,
    current: String,
    next_current: Option<String>,
    watch_task: Option<String>,
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

fn spec_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("data")
        .join("scene_graph_acceptance.toml")
}

fn fixture_path(name: &str) -> PathBuf {
    workspace_root()
        .join("crates")
        .join("anaden-vision")
        .join("tests")
        .join("fixtures")
        .join(name)
}

fn load_spec() -> Spec {
    let path = spec_path();
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read spec {}: {e}", path.display()));
    let spec: Spec =
        toml::from_str(&raw).unwrap_or_else(|e| panic!("parse spec {}: {e}", path.display()));
    assert_eq!(spec.schema, 1, "unknown spec schema {}", spec.schema);
    assert!(!spec.unit.is_empty(), "spec must declare units");
    assert!(!spec.blocked.is_empty(), "spec must declare blocked units");
    spec
}

fn normalize(fixture: &str) -> DynamicImage {
    let path = fixture_path(fixture);
    let raw =
        image::open(&path).unwrap_or_else(|e| panic!("open fixture corpus: {e} (tracked file)"));
    let (normalized, crop) = ScreenScaler::new().normalize_capture(&raw);
    assert!(
        crop.offset_x > 0,
        "{fixture}: production captures carry a detected letterbox band (offset_x={}, \
         expected >0; band width varies with capture geometry: 10px@1952w, 37px@1918w)",
        crop.offset_x
    );
    assert_eq!(normalized.width(), 1280, "{fixture}: normalization target");
    normalized
}

fn normalize_degenerate(fixture: &str) -> DynamicImage {
    let path = fixture_path(fixture);
    let raw =
        image::open(&path).unwrap_or_else(|e| panic!("open fixture corpus: {e} (tracked file)"));
    let (normalized, crop) = ScreenScaler::new().normalize_capture(&raw);
    assert_eq!(
        crop.offset_x, 0,
        "{fixture}: degenerate fixtures must NOT carry the production 10px band"
    );
    assert_eq!(normalized.width(), 1280, "{fixture}: normalization target");
    normalized
}

fn task_dirs(spec: &Spec) -> Vec<PathBuf> {
    let templates = workspace_root().join("templates");
    let mut dirs = Vec::new();
    for relative in &spec.task_dir {
        let path = templates.join(relative);
        assert!(
            path.is_dir(),
            "spec task_dir {relative} must exist ({} missing)",
            path.display()
        );
        dirs.push(path.clone());
        let watch = path.join("watch");
        if watch.is_dir() {
            dirs.push(watch);
        }
    }
    dirs.sort();
    dirs
}

fn task_index(spec: &Spec) -> BTreeMap<String, TaskDef> {
    let mut index: BTreeMap<String, TaskDef> = BTreeMap::new();
    for dir in task_dirs(spec) {
        let tasks =
            load_pipeline(&dir).unwrap_or_else(|e| panic!("load task dir {}: {e}", dir.display()));
        for task in tasks {
            if let Some(previous) = index.insert(task.name.clone(), task.clone()) {
                panic!(
                    "duplicate task name {} (templates collide: {} vs new entry in {})",
                    task.name,
                    previous.template.display(),
                    dir.display()
                );
            }
        }
    }
    index
}

fn matching(task: &TaskDef, normalized: &DynamicImage) -> Option<f32> {
    task.detect(normalized, Path::new(""))
        .unwrap_or_else(|e| panic!("{} detect error: {e}", task.name))
        .map(|m| m.confidence.0)
}

fn expect_names(spec: &Spec) -> BTreeSet<String> {
    spec.unit
        .iter()
        .flat_map(|u| u.expect.iter().map(|(name, _)| name.clone()))
        .collect()
}

fn declared_names_per_fixture(spec: &Spec) -> BTreeMap<String, BTreeSet<String>> {
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for unit in &spec.unit {
        let entry = out.entry(unit.fixture.clone()).or_default();
        for (name, _) in &unit.expect {
            entry.insert(name.clone());
        }
    }
    out
}

fn matches_for(
    fixture: &str,
    names: &BTreeSet<String>,
    index: &BTreeMap<String, TaskDef>,
    degenerate: bool,
) -> BTreeMap<String, f32> {
    let normalized = if degenerate {
        normalize_degenerate(fixture)
    } else {
        normalize(fixture)
    };
    let mut out = BTreeMap::new();
    for name in names {
        let task = index
            .get(name)
            .unwrap_or_else(|| panic!("unit references unknown task {name}"));
        if let Some(conf) = matching(task, &normalized) {
            out.insert(name.clone(), conf);
        }
    }
    out
}

#[test]
fn units_reproduce_declared_confidences_and_nothing_else() {
    let spec = load_spec();
    let index = task_index(&spec);
    let names = expect_names(&spec);
    let mut drift: Vec<String> = Vec::new();
    let mut below_floor: Vec<String> = Vec::new();
    for unit in &spec.unit {
        let matched = matches_for(&unit.fixture, &names, &index, false);
        let declared: BTreeSet<String> = unit.expect.iter().map(|(n, _)| n.clone()).collect();
        let observed: BTreeSet<String> = matched.keys().cloned().collect();
        if observed != declared {
            drift.push(format!(
                "unit {} ({}): matched anchor set drifted from the spec — the fixture must \
                 resolve to exactly its declared anchors (observed confs: {matched:?})",
                unit.name, unit.fixture
            ));
            continue;
        }
        for (name, min_conf) in &unit.expect {
            let observed = matched
                .get(name)
                .copied()
                .expect("declared anchor is in the matched set");
            if observed < *min_conf {
                below_floor.push(format!(
                    "unit {}: {name} conf {observed:.4} below declared floor {min_conf:.2} on {}",
                    unit.name, unit.fixture
                ));
            }
        }
    }
    let mut report = String::new();
    if !drift.is_empty() {
        report.push_str(&drift.join("\n"));
    }
    if !below_floor.is_empty() {
        if !report.is_empty() {
            report.push('\n');
        }
        report.push_str(&below_floor.join("\n"));
    }
    if !report.is_empty() {
        report.push_str(
            "\n\nfull observed matrix (environment fingerprint for cross-machine drift):\n",
        );
        for unit in &spec.unit {
            let matched = matches_for(&unit.fixture, &names, &index, false);
            let normalized = normalize(&unit.fixture);
            report.push_str(&format!(
                "unit {} ({}): normalized={}x{} observed={:?}\n",
                unit.name,
                unit.fixture,
                normalized.width(),
                normalized.height(),
                matched
            ));
        }
    }
    assert!(
        report.is_empty(),
        "confidence matrix diverged from the spec (full run, no fail-fast):\n{report}"
    );
}

#[test]
fn blocked_units_still_have_no_anchor() {
    let spec = load_spec();
    let index = task_index(&spec);
    let names = expect_names(&spec);
    let declared_per_fixture = declared_names_per_fixture(&spec);
    let empty: BTreeSet<String> = BTreeSet::new();
    for blocked in &spec.blocked {
        assert!(
            !blocked.reason.trim().is_empty(),
            "blocked unit {} must carry a reason",
            blocked.name
        );
        let matched = matches_for(&blocked.fixture, &names, &index, blocked.degenerate);
        let allowed = declared_per_fixture.get(&blocked.fixture).unwrap_or(&empty);
        let undeclared: Vec<&String> = matched
            .keys()
            .filter(|name| !allowed.contains(*name))
            .collect();
        assert!(
            undeclared.is_empty(),
            "blocked unit {} ({}) is no longer anchor-less: {undeclared:?} now match \
             ({matched:?}). Either the unit is now acceptable (move it to [[unit]]) or the \
             spec fixture is wrong",
            blocked.name,
            blocked.fixture
        );
    }
}

#[test]
fn declared_gaps_are_still_gaps() {
    let spec = load_spec();
    let index = task_index(&spec);
    for gap in &spec.gap {
        assert!(
            !gap.reason.trim().is_empty(),
            "gap {} must carry a reason",
            gap.name
        );
        for fixture in &gap.fixtures {
            let normalized = normalize(fixture);
            for name in &gap.tasks {
                let task = index
                    .get(name)
                    .unwrap_or_else(|| panic!("gap {} references unknown task {name}", gap.name));
                let conf = matching(task, &normalized);
                assert!(
                    conf.is_none(),
                    "gap {} is closed: {name} now matches {fixture} (conf {conf:?}) — the \
                     dependent pipeline goal/watch workaround must be re-derived, then this \
                     gap entry removed",
                    gap.name
                );
            }
        }
    }
}

#[test]
fn unit_anchors_resolve_and_clear_their_own_thresholds() {
    let spec = load_spec();
    let index = task_index(&spec);
    for unit in &spec.unit {
        assert!(
            fixture_path(&unit.fixture).is_file(),
            "unit {}: fixture {} must exist",
            unit.name,
            unit.fixture
        );
        for (name, min_conf) in &unit.expect {
            let task = index
                .get(name)
                .unwrap_or_else(|| panic!("unit {} references unknown task {name}", unit.name));
            assert!(
                *min_conf >= task.threshold,
                "unit {}: declared floor {min_conf:.2} for {name} is below the task \
                 threshold {:.2} (would accept a detect() nomatch)",
                unit.name,
                task.threshold
            );
        }
    }
}

fn pipeline_dirs_with_manifest() -> Vec<PathBuf> {
    let root = workspace_root().join("templates").join("pipelines");
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&root).expect("read pipelines root") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() && path.join("pipeline.toml").is_file() {
            out.push(path);
        }
    }
    out.sort();
    out
}

#[test]
fn pipeline_next_edges_are_closed() {
    let dirs = pipeline_dirs_with_manifest();
    assert!(!dirs.is_empty(), "at least one pipeline must be registered");
    let mut checked = 0usize;
    for dir in dirs {
        let tasks =
            load_pipeline(&dir).unwrap_or_else(|e| panic!("load pipeline {}: {e}", dir.display()));
        let names: BTreeSet<&str> = tasks.iter().map(|t| t.name.as_str()).collect();
        for task in &tasks {
            let Some(next) = task.next.as_ref() else {
                continue;
            };
            for target in next {
                assert!(
                    names.contains(target.as_str()),
                    "{}: {}.next references {target}, which is not a task in the same \
                     pipeline (cross-pipeline edges must be declared, not implied)",
                    dir.display(),
                    task.name
                );
                checked += 1;
            }
        }
    }
    assert!(
        checked > 0,
        "no next edge was verified (vacuous-pass guard)"
    );
}

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

fn load_replay_tasks(replay: &Replay) -> (PathBuf, Vec<TaskDef>, Vec<String>) {
    let dir = workspace_root().join("templates").join(&replay.pipeline);
    let mut tasks = load_pipeline(&dir)
        .unwrap_or_else(|e| panic!("replay {}: load {}: {e}", replay.name, dir.display()));
    tasks.extend(
        load_watch_defs(&dir)
            .unwrap_or_else(|e| panic!("replay {}: load watch defs: {e}", replay.name)),
    );
    let manifest = load_pipeline_manifest(&dir)
        .unwrap_or_else(|e| panic!("replay {}: load manifest: {e}", replay.name));
    assert_eq!(
        manifest.start_task, replay.start_task,
        "replay {}: manifest start_task drifted from the spec",
        replay.name
    );
    assert!(
        manifest.watch.contains(&replay.watch_task),
        "replay {}: manifest watch {:?} must contain {}",
        replay.name,
        manifest.watch,
        replay.watch_task
    );
    let watch = manifest.watch.clone();
    (dir, tasks, watch)
}

fn assert_step(
    replay: &Replay,
    step: &Step,
    outcome: StepOutcome,
    driver: &PipelineDriver<FakeCapture, FakeInput>,
) {
    match (step.outcome.as_str(), outcome) {
        (
            "fired",
            StepOutcome::Fired {
                next_current,
                fired,
            },
        ) => {
            assert_eq!(
                next_current.as_deref(),
                step.next_current.as_deref(),
                "replay {}: fired step from {} advanced to {:?}, spec says {:?}",
                replay.name,
                step.current,
                next_current,
                step.next_current
            );
            assert!(
                fired.is_some(),
                "replay {}: fired step from {} emitted no command",
                replay.name,
                step.current
            );
            assert_eq!(
                driver.current(),
                step.next_current.as_deref().unwrap_or(""),
                "replay {}: state machine did not advance to the declared next task",
                replay.name
            );
        }
        ("no_match", StepOutcome::NoMatch) => {}
        ("watch_stop", StepOutcome::WatchFired { task, fired, stop }) => {
            assert_eq!(
                step.watch_task.as_deref(),
                Some(task.as_str()),
                "replay {}: watch fired {task}, spec says {:?}",
                replay.name,
                step.watch_task
            );
            assert!(
                stop,
                "replay {}: watch task must be action=stop",
                replay.name
            );
            assert!(
                fired.is_none(),
                "replay {}: stop watch must not emit an input command",
                replay.name
            );
        }
        (expected, other) => panic!(
            "replay {}: step from {} expected {expected}, got {other:?}",
            replay.name, step.current
        ),
    }
}

#[tokio::test]
async fn replays_reproduce_declared_driver_outcomes() {
    let spec = load_spec();
    assert!(!spec.replay.is_empty(), "spec must declare replays");
    for replay in &spec.replay {
        let (_dir, tasks, watch) = load_replay_tasks(replay);
        let names: BTreeSet<&str> = tasks.iter().map(|t| t.name.as_str()).collect();
        assert!(
            names.contains(replay.start_task.as_str()),
            "replay {}: start task {} must exist in the pipeline",
            replay.name,
            replay.start_task
        );

        let mut frames = Vec::new();
        for fixture in &replay.frames {
            frames.push(normalize(fixture));
        }
        assert_eq!(
            frames.len(),
            replay.step.len(),
            "replay {}: one frame per declared step",
            replay.name
        );
        let (device_w, device_h) = (frames[0].width(), frames[0].height());
        let frames = Arc::new(Mutex::new(frames));
        let fired = Arc::new(Mutex::new(Vec::new()));
        let mut driver = PipelineDriver::new(
            FakeCapture {
                frames: frames.clone(),
            },
            FakeInput {
                fired: fired.clone(),
            },
            PipelineState::new(replay.start_task.clone()),
            tasks,
            device_w,
            device_h,
            300,
        )
        .with_watch(watch);

        for step in &replay.step {
            assert_eq!(
                driver.current(),
                step.current,
                "replay {}: driver current drifted before the step (spec {})",
                replay.name,
                step.current
            );
            let outcome = driver.run_once().await;
            assert_step(replay, step, outcome, &driver);
        }
        assert_eq!(
            fired.lock().expect("fired lock").len(),
            replay.step.iter().filter(|s| s.outcome == "fired").count(),
            "replay {}: one recorded input per fired step",
            replay.name
        );
    }
}
