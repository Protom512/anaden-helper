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

fn neko_tasks() -> Vec<TaskDef> {
    let dir = workspace_root()
        .join("templates")
        .join("scenes")
        .join("neko_pc");
    load_pipeline(&dir).unwrap_or_else(|e| panic!("load neko_pc templates {}: {e}", dir.display()))
}

fn find_task<'a>(tasks: &'a [TaskDef], name: &str) -> &'a TaskDef {
    tasks
        .iter()
        .find(|t| t.name == name)
        .unwrap_or_else(|| panic!("{name} must exist in neko_pc templates"))
}

fn detect_conf(task: &TaskDef, normalized: &image::DynamicImage) -> Option<f32> {
    task.detect(normalized, Path::new(""))
        .unwrap_or_else(|e| panic!("{} detect error: {e}", task.name))
        .map(|m| m.confidence.0)
}

fn normalize_diary_fixture() -> image::DynamicImage {
    let path = fixture("neko_diary_live_2880x1620.png");
    let raw = image::open(&path)
        .unwrap_or_else(|e| panic!("open fixture {}: {e} (tracked file)", path.display()));
    assert_eq!(
        (raw.width(), raw.height()),
        (2880, 1620),
        "fixture must be the 2026-11-03 PrintWindow capture (Issue #260 E5 evidence)"
    );
    let (normalized, crop) = ScreenScaler::new().normalize_capture(&raw);
    assert_eq!(
        (crop.offset_x, crop.offset_y),
        (10, 0),
        "2026-11-03 captures carry a 10px left black band; template generation depends on it"
    );
    assert_eq!(normalized.width(), 1280);
    normalized
}

#[test]
#[allow(clippy::unwrap_used)]
fn hint_text_anchor_matches_live_diary_fixture_through_production_preprocessing() {
    let normalized = normalize_diary_fixture();
    let tasks = neko_tasks();
    let task = find_task(&tasks, "NekoHintMiparuPc");
    let conf = detect_conf(task, &normalized).unwrap_or_else(|| {
        panic!(
            "NekoHintMiparuPc must MATCH the diary fixture (roi={:?} thr={})",
            task.roi, task.threshold
        )
    });
    assert!(
        conf >= 0.95,
        "NekoHintMiparuPc: conf {conf:.4} below the 0.95 quality buffer (threshold {})",
        task.threshold
    );
}

#[test]
#[allow(clippy::unwrap_used)]
fn hint_text_anchor_does_not_match_second_diary_row() {
    let normalized = normalize_diary_fixture();
    let tasks = neko_tasks();
    let mut row2 = find_task(&tasks, "NekoHintMiparuPc").clone();
    row2.roi = Some([470, 205, 340, 100]);
    let conf = detect_conf(&row2, &normalized);
    assert!(
        conf.is_none(),
        "NekoHintMiparuPc must NOT match diary row 2 text band (got conf {conf:?})"
    );
}

#[test]
#[allow(clippy::unwrap_used)]
fn hint_text_anchor_does_not_match_ad_select_fixture() {
    let raw = image::open(fixture("ad_select_live_1918x1048.png"))
        .unwrap_or_else(|e| panic!("open fixture: {e}"));
    let (normalized, _crop) = ScreenScaler::new().normalize_capture(&raw);
    let tasks = neko_tasks();
    let task = find_task(&tasks, "NekoHintMiparuPc");
    let conf = detect_conf(task, &normalized);
    assert!(
        conf.is_none(),
        "NekoHintMiparuPc must NOT match the AD-select fixture (got conf {conf:?})"
    );
}
