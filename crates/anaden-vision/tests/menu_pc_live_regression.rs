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

fn menu_pc_tasks() -> Vec<TaskDef> {
    let dir = workspace_root()
        .join("templates")
        .join("scenes")
        .join("menu_pc");
    load_pipeline(&dir).unwrap_or_else(|e| panic!("load menu_pc templates {}: {e}", dir.display()))
}

fn find_task<'a>(tasks: &'a [TaskDef], name: &str) -> &'a TaskDef {
    tasks
        .iter()
        .find(|t| t.name == name)
        .unwrap_or_else(|| panic!("{name} must exist in menu_pc templates"))
}

const REBUILT: &[&str] = &[
    "MenuPcParty",
    "MenuPcGrasta",
    "MenuPcBoard",
    "MenuPcBag",
    "MenuPcRecord",
    "MenuPcInfo",
];

fn detect_conf(task: &TaskDef, normalized: &image::DynamicImage) -> Option<f32> {
    task.detect(normalized, Path::new(""))
        .unwrap_or_else(|e| panic!("{} detect error: {e}", task.name))
        .map(|m| m.confidence.0)
}

fn normalize_live_fixture(name: &str) -> image::DynamicImage {
    let path = fixture(name);
    let raw = image::open(&path)
        .unwrap_or_else(|e| panic!("open fixture {}: {e} (tracked file)", path.display()));
    assert_eq!(
        (raw.width(), raw.height()),
        (2880, 1620),
        "{name} must be a 2026-11-03 PrintWindow capture (Issue #268 evidence)"
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

#[test]
#[allow(clippy::unwrap_used)]
fn rebuilt_anchors_match_live_menu_fixture_through_production_preprocessing() {
    let normalized = normalize_live_fixture("menu_panel_live_2880x1620.png");
    let tasks = menu_pc_tasks();
    for name in REBUILT {
        let task = find_task(&tasks, name);
        let conf = detect_conf(task, &normalized).unwrap_or_else(|| {
            panic!(
                "{name} must MATCH the menu fixture (roi={:?} thr={})",
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
fn rebuilt_anchors_do_not_cross_match_neighbor_tile_rois() {
    let normalized = normalize_live_fixture("menu_panel_live_2880x1620.png");
    let tasks = menu_pc_tasks();
    for name in REBUILT {
        let task = find_task(&tasks, name);
        for other in REBUILT {
            if other == name {
                continue;
            }
            let neighbor_roi = find_task(&tasks, other).roi;
            let mut swapped = task.clone();
            swapped.roi = neighbor_roi;
            let conf = detect_conf(&swapped, &normalized);
            assert!(
                conf.is_none(),
                "{name} must NOT match the tile ROI of {other:?} (got conf {conf:?})"
            );
        }
    }
}

#[test]
#[allow(clippy::unwrap_used)]
fn rebuilt_anchors_do_not_match_other_live_screens() {
    let tasks = menu_pc_tasks();
    for fixture_name in [
        "record_open_live_2880x1620.png",
        "world_map_live_2880x1620.png",
        "field_live_2880x1620.png",
        "field_hud_live_2880x1620.png",
        "neko_diary_live_2880x1620.png",
    ] {
        let normalized = normalize_live_fixture(fixture_name);
        for name in REBUILT {
            let task = find_task(&tasks, name);
            assert!(
                detect_conf(task, &normalized).is_none(),
                "{name} must NOT match the {fixture_name} fixture (cross-screen false positive)"
            );
        }
    }
}
