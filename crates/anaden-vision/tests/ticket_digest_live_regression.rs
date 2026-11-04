use std::path::{Path, PathBuf};

use anaden_vision::{ScreenScaler, TaskDef, load_pipeline};

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

fn ticket_digest_tasks() -> Vec<TaskDef> {
    let dir = workspace_root()
        .join("templates")
        .join("pipelines")
        .join("ticket_digest");
    load_pipeline(&dir)
        .unwrap_or_else(|e| panic!("load ticket_digest pipeline {}: {e}", dir.display()))
}

fn find_task<'a>(tasks: &'a [TaskDef], name: &str) -> &'a TaskDef {
    tasks
        .iter()
        .find(|t| t.name == name)
        .unwrap_or_else(|| panic!("{name} must exist in ticket_digest pipeline"))
}

fn detect_conf(task: &TaskDef, normalized: &image::DynamicImage) -> Option<f32> {
    task.detect(normalized, Path::new(""))
        .unwrap_or_else(|e| panic!("{} detect error: {e}", task.name))
        .map(|m| m.confidence.0)
}

fn normalize_fixture(name: &str) -> image::DynamicImage {
    let path = fixture(name);
    let raw = image::open(&path)
        .unwrap_or_else(|e| panic!("open fixture {}: {e} (tracked file)", path.display()));
    assert_eq!(
        (raw.width(), raw.height()),
        (1918, 1048),
        "{name} must be the 2026-11-01 PrintWindow capture (Issue #260 E4 evidence)"
    );
    let (normalized, _crop) = ScreenScaler::new().normalize_capture(&raw);
    assert_eq!(
        normalized.width(),
        1280,
        "normalize must produce the 1280-base frame"
    );
    normalized
}

#[test]
#[allow(clippy::unwrap_used)]
fn ad_select_anchors_match_live_fixture_through_production_preprocessing() {
    let normalized = normalize_fixture("ad_select_live_1918x1048.png");
    let tasks = ticket_digest_tasks();
    for name in ["AdSelectDetectPc", "HardRowDetectPc", "VeryHardRowDetectPc"] {
        let task = find_task(&tasks, name);
        let conf = detect_conf(task, &normalized).unwrap_or_else(|| {
            panic!(
                "{name} must MATCH the AD-select live fixture (roi={:?} thr={})",
                task.roi, task.threshold
            )
        });
        assert!(
            conf >= 0.95,
            "{name}: conf {conf:.4} below the 0.95 quality buffer (threshold {})",
            task.threshold
        );
    }
}

#[test]
#[allow(clippy::unwrap_used)]
fn result_congrats_anchor_matches_reward_fixture() {
    let normalized = normalize_fixture("reward_congrats_live_1918x1048.png");
    let tasks = ticket_digest_tasks();
    let task = find_task(&tasks, "ResultCongratsDetectPc");
    let conf = detect_conf(task, &normalized).unwrap_or_else(|| {
        panic!(
            "ResultCongratsDetectPc must MATCH the reward fixture (roi={:?} thr={})",
            task.roi, task.threshold
        )
    });
    assert!(
        conf >= 0.95,
        "ResultCongratsDetectPc: conf {conf:.4} below the 0.95 quality buffer"
    );
}

#[test]
#[allow(clippy::unwrap_used)]
fn anchors_do_not_cross_match_other_screen_fixture() {
    let ad_select = normalize_fixture("ad_select_live_1918x1048.png");
    let reward = normalize_fixture("reward_congrats_live_1918x1048.png");
    let tasks = ticket_digest_tasks();
    for name in ["AdSelectDetectPc", "HardRowDetectPc", "VeryHardRowDetectPc"] {
        let task = find_task(&tasks, name);
        assert!(
            detect_conf(task, &reward).is_none(),
            "{name} must NOT match the reward-screen fixture (cross-screen false positive)"
        );
    }
    let result_task = find_task(&tasks, "ResultCongratsDetectPc");
    assert!(
        detect_conf(result_task, &ad_select).is_none(),
        "ResultCongratsDetectPc must NOT match the AD-select fixture"
    );
}
