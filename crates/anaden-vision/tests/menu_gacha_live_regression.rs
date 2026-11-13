use std::path::{Path, PathBuf};

use anaden_vision::{ScreenScaler, TaskDef, load_pipeline};

const FIXTURE: &str = "menu_panel_live_2880x1620.png";
const TARGET: &str = "MenuPcGacha";

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

fn menu_pc_tasks() -> Vec<TaskDef> {
    let dir = workspace_root()
        .join("templates")
        .join("scenes")
        .join("menu_pc");
    load_pipeline(&dir).unwrap_or_else(|e| panic!("load menu_pc templates {}: {e}", dir.display()))
}

fn gacha_task(tasks: &[TaskDef]) -> TaskDef {
    tasks
        .iter()
        .find(|t| t.name == TARGET)
        .unwrap_or_else(|| panic!("{TARGET} must exist in menu_pc templates"))
        .clone()
}

fn detect_conf(task: &TaskDef, normalized: &image::DynamicImage) -> Option<f32> {
    task.detect(normalized, Path::new(""))
        .unwrap_or_else(|e| panic!("{} detect error: {e}", task.name))
        .map(|m| m.confidence.0)
}

fn other_tiles(tasks: &[TaskDef]) -> Vec<TaskDef> {
    tasks.iter().filter(|t| t.name != TARGET).cloned().collect()
}

fn normalize_live_fixture(name: &str) -> image::DynamicImage {
    let path = fixture(name);
    let raw = image::open(&path)
        .unwrap_or_else(|e| panic!("open fixture {}: {e} (tracked file)", path.display()));
    let (normalized, crop) = ScreenScaler::new().normalize_capture(&raw);
    assert_eq!(
        (crop.offset_x, crop.offset_y),
        (10, 0),
        "{name}: 2026-11-03 captures carry a 10px left black band"
    );
    assert_eq!(normalized.width(), 1280);
    normalized
}

#[test]
#[allow(clippy::unwrap_used)]
fn gacha_anchor_matches_live_menu_fixture_through_production_preprocessing() {
    let normalized = normalize_live_fixture(FIXTURE);
    let task = gacha_task(&menu_pc_tasks());
    let conf = detect_conf(&task, &normalized).unwrap_or_else(|| {
        panic!(
            "{TARGET} must MATCH {FIXTURE} (roi={:?} thr={})",
            task.roi, task.threshold
        )
    });
    assert!(
        conf >= 0.95,
        "{TARGET}: conf {conf:.4} below the 0.95 quality buffer (threshold {})",
        task.threshold
    );
}

#[test]
#[allow(clippy::unwrap_used)]
fn gacha_anchor_does_not_cross_match_neighbor_tile_rois() {
    let normalized = normalize_live_fixture(FIXTURE);
    let tasks = menu_pc_tasks();
    let task = gacha_task(&tasks);
    for other in other_tiles(&tasks) {
        let mut swapped = task.clone();
        swapped.roi = other.roi;
        let conf = detect_conf(&swapped, &normalized);
        assert!(
            conf.is_none(),
            "{TARGET} must NOT match the tile ROI of {} (got conf {conf:?})",
            other.name
        );
    }
}

#[test]
#[allow(clippy::unwrap_used)]
fn gacha_anchor_does_not_match_other_live_screens() {
    let task = gacha_task(&menu_pc_tasks());
    for fixture_name in [
        "record_open_live_2880x1620.png",
        "world_map_live_2880x1620.png",
        "field_live_2880x1620.png",
        "field_hud_live_2880x1620.png",
        "neko_diary_live_2880x1620.png",
    ] {
        let normalized = normalize_live_fixture(fixture_name);
        assert!(
            detect_conf(&task, &normalized).is_none(),
            "{TARGET} must NOT match the {fixture_name} fixture (cross-screen false positive)"
        );
    }
}
