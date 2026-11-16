#![allow(clippy::unwrap_used)]
#![allow(clippy::expect_used)]
#![allow(clippy::panic)]

use std::path::{Path, PathBuf};

use anaden_vision::{ScreenScaler, TaskDef, load_pipeline};

const MARGIN_OVER_THRESHOLD: f32 = 0.05;

const FIELD_FIXTURE: &str = "field_live_2880x1620.png";
const FIELD_HUD_FIXTURE: &str = "field_hud_live_2880x1620.png";
const MENU_PANEL_FIXTURE: &str = "menu_panel_live_2880x1620.png";
const DIARY_FIXTURE: &str = "neko_diary_live_2880x1620.png";
const AD_SELECT_FIXTURE: &str = "ad_select_live_1918x1048.png";
const WORLD_MAP_FIXTURE: &str = "world_map_live_2880x1620.png";

const ERA_TASKS: [&str; 3] = [
    "TicketPcWorldmapEraAncient",
    "TicketPcWorldmapEraModern",
    "TicketPcWorldmapEraFuture",
];

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

fn load_scene_tasks(category: &str) -> Vec<TaskDef> {
    let dir = workspace_root()
        .join("templates")
        .join("scenes")
        .join(category);
    load_pipeline(&dir)
        .unwrap_or_else(|e| panic!("load {category} templates {}: {e}", dir.display()))
}

fn find_task<'a>(tasks: &'a [TaskDef], name: &str) -> &'a TaskDef {
    tasks
        .iter()
        .find(|t| t.name == name)
        .unwrap_or_else(|| panic!("{name} must exist in the loaded scene templates"))
}

fn normalize_capture_fixture(name: &str) -> image::DynamicImage {
    let path = fixture(name);
    let raw = image::open(&path).unwrap_or_else(|e| panic!("open fixture {}: {e}", path.display()));
    let (normalized, _crop) = ScreenScaler::new().normalize_capture(&raw);
    normalized
}

fn normalize_2880_fixture(name: &str) -> image::DynamicImage {
    let path = fixture(name);
    let raw = image::open(&path).unwrap_or_else(|e| panic!("open fixture {}: {e}", path.display()));
    assert_eq!(
        (raw.width(), raw.height()),
        (2880, 1620),
        "fixture {name} must be the 2026-11-03 PrintWindow capture series"
    );
    let (normalized, crop) = ScreenScaler::new().normalize_capture(&raw);
    assert_eq!(
        (crop.offset_x, crop.offset_y),
        (10, 0),
        "2026-11-03 captures carry a 10px left black band; anchor crops depend on it"
    );
    normalized
}

fn detect_conf(task: &TaskDef, frame: &image::DynamicImage) -> Option<f32> {
    task.detect(frame, Path::new(""))
        .unwrap_or_else(|e| panic!("{} detect error: {e}", task.name))
        .map(|m| m.confidence.0)
}

fn assert_positive(task: &TaskDef, frame: &image::DynamicImage, fixture_name: &str) {
    let conf = detect_conf(task, frame).unwrap_or_else(|| {
        panic!(
            "{} must MATCH {fixture_name} (roi={:?} threshold={})",
            task.name, task.roi, task.threshold
        )
    });
    assert!(
        conf >= task.threshold + MARGIN_OVER_THRESHOLD,
        "{}: conf {conf:.4} on {fixture_name} is below threshold {} + margin {}",
        task.name,
        task.threshold,
        MARGIN_OVER_THRESHOLD
    );
}

fn assert_negative(task: &TaskDef, frame: &image::DynamicImage, fixture_name: &str) {
    let conf = detect_conf(task, frame);
    assert!(
        conf.is_none(),
        "{} must NOT match {fixture_name} (got conf {conf:?})",
        task.name
    );
}

#[test]
fn neko_menu_btn_matches_both_field_fixtures_with_margin() {
    let tasks = load_scene_tasks("neko_pc");
    let task = find_task(&tasks, "NekoMenuBtnPc");
    assert_positive(task, &normalize_2880_fixture(FIELD_FIXTURE), FIELD_FIXTURE);
    assert_positive(
        task,
        &normalize_2880_fixture(FIELD_HUD_FIXTURE),
        FIELD_HUD_FIXTURE,
    );
}

#[test]
fn neko_menu_btn_no_match_on_town_diary_ad_and_worldmap() {
    let tasks = load_scene_tasks("neko_pc");
    let task = find_task(&tasks, "NekoMenuBtnPc");
    assert_negative(
        task,
        &normalize_capture_fixture(MENU_PANEL_FIXTURE),
        MENU_PANEL_FIXTURE,
    );
    assert_negative(
        task,
        &normalize_capture_fixture(DIARY_FIXTURE),
        DIARY_FIXTURE,
    );
    assert_negative(
        task,
        &normalize_capture_fixture(AD_SELECT_FIXTURE),
        AD_SELECT_FIXTURE,
    );
    assert_negative(
        task,
        &normalize_capture_fixture(WORLD_MAP_FIXTURE),
        WORLD_MAP_FIXTURE,
    );
}

#[test]
fn neko_diary_tab_matches_both_field_fixtures_with_margin() {
    let tasks = load_scene_tasks("neko_pc");
    let task = find_task(&tasks, "NekoDiaryTabPc");
    assert_positive(task, &normalize_2880_fixture(FIELD_FIXTURE), FIELD_FIXTURE);
    assert_positive(
        task,
        &normalize_2880_fixture(FIELD_HUD_FIXTURE),
        FIELD_HUD_FIXTURE,
    );
}

#[test]
fn neko_diary_tab_no_match_on_town_diary_ad_and_worldmap() {
    let tasks = load_scene_tasks("neko_pc");
    let task = find_task(&tasks, "NekoDiaryTabPc");
    assert_negative(
        task,
        &normalize_capture_fixture(MENU_PANEL_FIXTURE),
        MENU_PANEL_FIXTURE,
    );
    assert_negative(
        task,
        &normalize_capture_fixture(DIARY_FIXTURE),
        DIARY_FIXTURE,
    );
    assert_negative(
        task,
        &normalize_capture_fixture(AD_SELECT_FIXTURE),
        AD_SELECT_FIXTURE,
    );
    assert_negative(
        task,
        &normalize_capture_fixture(WORLD_MAP_FIXTURE),
        WORLD_MAP_FIXTURE,
    );
}

#[test]
fn neko_worldmap_era_tabs_match_world_map_fixture_with_margin() {
    let tasks = load_scene_tasks("ticket_pc");
    for name in ERA_TASKS {
        assert_positive(
            find_task(&tasks, name),
            &normalize_2880_fixture(WORLD_MAP_FIXTURE),
            WORLD_MAP_FIXTURE,
        );
    }
}

#[test]
fn neko_worldmap_era_tabs_no_match_on_field_town_diary_and_ad() {
    let tasks = load_scene_tasks("ticket_pc");
    let negatives = [
        FIELD_FIXTURE,
        FIELD_HUD_FIXTURE,
        MENU_PANEL_FIXTURE,
        DIARY_FIXTURE,
        AD_SELECT_FIXTURE,
    ];
    for name in ERA_TASKS {
        let task = find_task(&tasks, name);
        for fixture_name in negatives {
            assert_negative(task, &normalize_capture_fixture(fixture_name), fixture_name);
        }
    }
}

#[test]
fn neko_diary_slot_frame_matches_diary_fixture_with_margin() {
    let tasks = load_scene_tasks("neko_pc");
    let task = find_task(&tasks, "NekoDiarySlotFramePc");
    assert_positive(task, &normalize_2880_fixture(DIARY_FIXTURE), DIARY_FIXTURE);
}

#[test]
fn neko_diary_slot_frame_no_match_on_field_town_ad_and_worldmap() {
    let tasks = load_scene_tasks("neko_pc");
    let task = find_task(&tasks, "NekoDiarySlotFramePc");
    let negatives = [
        FIELD_FIXTURE,
        FIELD_HUD_FIXTURE,
        MENU_PANEL_FIXTURE,
        AD_SELECT_FIXTURE,
        WORLD_MAP_FIXTURE,
    ];
    for fixture_name in negatives {
        assert_negative(task, &normalize_capture_fixture(fixture_name), fixture_name);
    }
}
