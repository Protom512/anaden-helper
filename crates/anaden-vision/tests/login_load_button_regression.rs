use std::path::{Path, PathBuf};

use anaden_vision::{Action, ScreenScaler, TaskDef, load_pipeline};

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

fn login_task(name: &str) -> TaskDef {
    let dir = workspace_root()
        .join("templates")
        .join("pipelines")
        .join("login");
    load_pipeline(&dir)
        .unwrap_or_else(|e| panic!("load login pipeline {}: {e}", dir.display()))
        .into_iter()
        .find(|t| t.name == name)
        .unwrap_or_else(|| panic!("{name} must exist in templates/pipelines/login"))
}

fn match_load_task(screenshot: &str) -> ((f32, u32, u32), TaskDef) {
    let raw = image::open(fixture(screenshot))
        .unwrap_or_else(|e| panic!("open fixture {screenshot}: {e}"));
    let (normalized, _info) = ScreenScaler::new().normalize_capture(&raw);
    let task = login_task("LoginClickLoadPc");
    let matched = task
        .detect(&normalized, Path::new(""))
        .unwrap_or_else(|e| panic!("LoginClickLoadPc detect error: {e}"))
        .unwrap_or_else(|| {
            panic!(
                "LoginClickLoadPc must match {screenshot} through production preprocessing \
                 (roi={:?} threshold={})",
                task.roi, task.threshold
            )
        });
    let region = matched.region;
    (
        (
            matched.confidence.0,
            region.x + region.width / 2,
            region.y + region.height / 2,
        ),
        task,
    )
}

#[test]
#[allow(clippy::panic)]
fn load_button_template_matches_the_tracked_title_fixture() {
    let ((confidence, cx, cy), task) = match_load_task("title_live_1952x1098.png");
    assert!(
        confidence >= 0.95,
        "confidence {confidence} below the 0.95 floor for the tracked title fixture"
    );
    assert!(
        (600..=690).contains(&cx) && (520..=600).contains(&cy),
        "load button must be found on the 'ロードゲーム' label, got center ({cx},{cy})"
    );
    assert_eq!(task.threshold, 0.80);
    assert!(
        matches!(task.action, Some(Action::ClickSelf)),
        "the load step must click the matched label itself, not a stale fixed rect (DEF-011)"
    );
}

#[test]
#[allow(clippy::panic)]
fn load_button_template_matches_the_live_4k_frame_from_the_def012_run() {
    let ((confidence, cx, cy), _task) = match_load_task("title_menu_live_20261111_3783x2128.png");
    assert!(
        confidence >= 0.90,
        "confidence {confidence} below the 0.90 floor on the real 3783x2128 live frame"
    );
    assert!(
        (600..=690).contains(&cx) && (510..=590).contains(&cy),
        "load button center ({cx},{cy}) is not where the live frame shows 'ロードゲーム'"
    );
}
