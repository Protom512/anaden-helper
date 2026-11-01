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
        .unwrap_or_else(|e| panic!("load ticket_digest templates {}: {e}", dir.display()))
}

fn find_task<'a>(tasks: &'a [TaskDef], name: &str) -> &'a TaskDef {
    tasks
        .iter()
        .find(|t| t.name == name)
        .unwrap_or_else(|| panic!("{name} must exist in ticket_digest templates"))
}

fn detect_conf(task: &TaskDef, normalized: &image::DynamicImage) -> Option<f32> {
    task.detect(normalized, Path::new(""))
        .unwrap_or_else(|e| panic!("{} detect error: {e}", task.name))
        .map(|m| m.confidence.0)
}

fn normalize_live_2880(name: &str) -> image::DynamicImage {
    let path = fixture(name);
    let raw = image::open(&path)
        .unwrap_or_else(|e| panic!("open fixture {}: {e} (tracked file)", path.display()));
    assert_eq!(
        (raw.width(), raw.height()),
        (2880, 1620),
        "{name} must be a 2026-11-03 PrintWindow capture (Issue #268/#248 evidence)"
    );
    let (normalized, crop) = ScreenScaler::new().normalize_capture(&raw);
    assert_eq!(
        (crop.offset_x, crop.offset_y),
        (10, 0),
        "{name}: 2026-11-03 captures carry a 10px left black band; template generation depends on it"
    );
    assert_eq!(normalized.width(), 1280);
    normalized
}

fn normalize_live_1918(name: &str) -> image::DynamicImage {
    let path = fixture(name);
    let raw = image::open(&path)
        .unwrap_or_else(|e| panic!("open fixture {}: {e} (tracked file)", path.display()));
    assert_eq!(
        (raw.width(), raw.height()),
        (1918, 1048),
        "{name} must be the 2026-11-01 PrintWindow capture (Issue #260 E4 evidence)"
    );
    let (normalized, _crop) = ScreenScaler::new().normalize_capture(&raw);
    assert_eq!(normalized.width(), 1280);
    normalized
}

#[test]
#[allow(clippy::unwrap_used)]
fn world_map_anchor_matches_live_fixture_through_production_preprocessing() {
    let normalized = normalize_live_2880("world_map_live_2880x1620.png");
    let tasks = ticket_digest_tasks();
    let task = find_task(&tasks, "WorldMapDetectPc");
    let conf = detect_conf(task, &normalized).unwrap_or_else(|| {
        panic!(
            "WorldMapDetectPc must MATCH the world-map fixture (roi={:?} thr={})",
            task.roi, task.threshold
        )
    });
    assert!(
        conf >= 0.95,
        "WorldMapDetectPc: conf {conf:.4} below the 0.95 quality buffer (threshold {})",
        task.threshold
    );
}

#[test]
#[allow(clippy::unwrap_used)]
fn world_map_anchor_does_not_match_other_live_screens() {
    let tasks = ticket_digest_tasks();
    let task = find_task(&tasks, "WorldMapDetectPc");
    for fixture_name in [
        "menu_panel_live_2880x1620.png",
        "record_open_live_2880x1620.png",
        "field_hud_live_2880x1620.png",
        "neko_diary_live_2880x1620.png",
    ] {
        let normalized = normalize_live_2880(fixture_name);
        assert!(
            detect_conf(task, &normalized).is_none(),
            "WorldMapDetectPc must NOT match the {fixture_name} fixture (cross-screen false positive)"
        );
    }
    for fixture_name in [
        "ad_select_live_1918x1048.png",
        "reward_congrats_live_1918x1048.png",
    ] {
        let normalized = normalize_live_1918(fixture_name);
        assert!(
            detect_conf(task, &normalized).is_none(),
            "WorldMapDetectPc must NOT match the {fixture_name} fixture (cross-screen false positive)"
        );
    }
}
