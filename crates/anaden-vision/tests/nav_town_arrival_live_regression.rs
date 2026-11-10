use std::path::{Path, PathBuf};

use anaden_vision::{ScreenScaler, TaskDef, load_pipeline, load_watch_defs};

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

fn normalize_live_fixture(name: &str, expected: (u32, u32)) -> image::DynamicImage {
    let path = fixture_path(name);
    let raw = image::open(&path)
        .unwrap_or_else(|e| panic!("open fixture {}: {e} (tracked file)", path.display()));
    assert_eq!(
        (raw.width(), raw.height()),
        expected,
        "{name}: unexpected dims (the fixture must be the PrintWindow capture it claims to be)"
    );
    let (normalized, _crop) = ScreenScaler::new().normalize_capture(&raw);
    assert_eq!(normalized.width(), 1280);
    normalized
}

fn nav_dir() -> PathBuf {
    workspace_root()
        .join("templates")
        .join("pipelines")
        .join("nav_to_field_pc")
}

fn nav_tasks() -> Vec<TaskDef> {
    let mut tasks =
        load_pipeline(&nav_dir()).unwrap_or_else(|e| panic!("load nav_to_field_pc pipeline: {e}"));
    tasks
        .extend(load_watch_defs(&nav_dir()).unwrap_or_else(|e| panic!("load nav watch defs: {e}")));
    tasks
}

fn find_task<'a>(tasks: &'a [TaskDef], name: &str) -> &'a TaskDef {
    tasks
        .iter()
        .find(|t| t.name == name)
        .unwrap_or_else(|| panic!("{name} must exist in the pipeline"))
}

fn detect_conf(task: &TaskDef, normalized: &image::DynamicImage) -> Option<f32> {
    task.detect(normalized, Path::new(""))
        .unwrap_or_else(|e| panic!("{} detect error: {e}", task.name))
        .map(|m| m.confidence.0)
}

#[test]
#[allow(clippy::unwrap_used)]
fn field_town_bar_watch_matches_live_town_fixture_through_production_preprocessing() {
    let normalized = normalize_live_fixture("field_live_20261110_3783x2128.png", (3783, 2128));
    let tasks = nav_tasks();

    let task = find_task(&tasks, "FieldTownBarPc");
    let conf = detect_conf(task, &normalized).unwrap_or_else(|| {
        panic!(
            "FieldTownBarPc must MATCH the live town fixture (roi={:?} threshold={}) — the \
             town arrival watch needle or ROI is stale for the current capture mode",
            task.roi, task.threshold
        )
    });
    assert!(
        conf >= task.threshold,
        "FieldTownBarPc: conf {conf:.4} below the TOML threshold {}",
        task.threshold
    );
    assert!(
        conf >= 0.85,
        "FieldTownBarPc: conf {conf:.4} degraded below the 0.85 quality buffer — the \
         town arrival watch needle or ROI may have regressed"
    );
}

#[test]
#[allow(clippy::unwrap_used)]
fn field_town_bar_watch_does_not_match_other_live_screens() {
    let tasks = nav_tasks();

    let menu_probe = normalize_live_fixture("menu_panel_live_2880x1620.png", (2880, 1620));
    let title_probe = normalize_live_fixture("title_live_1952x1098.png", (1952, 1098));

    let task = find_task(&tasks, "FieldTownBarPc");
    for (screen, probe) in [("menu", &menu_probe), ("title", &title_probe)] {
        let false_conf = detect_conf(task, probe);
        assert!(
            false_conf.is_none(),
            "FieldTownBarPc must NOT match the {screen} fixture (cross-screen false positive, \
             conf={:?} vs threshold {})",
            false_conf,
            task.threshold
        );
    }
}

#[test]
#[allow(clippy::unwrap_used)]
fn currency_bar_arrival_anchor_does_not_match_town_fixture() {
    let normalized = normalize_live_fixture("field_live_20261110_3783x2128.png", (3783, 2128));
    let tasks = nav_tasks();

    let task = find_task(&tasks, "FieldHudTopPc");
    let conf = detect_conf(task, &normalized);
    assert!(
        conf.is_none(),
        "FieldHudTopPc (currency bar) must NOT match the town fixture: the currency bar is \
         absent in town field (DEF-008), which is why the town watch exists"
    );
}
