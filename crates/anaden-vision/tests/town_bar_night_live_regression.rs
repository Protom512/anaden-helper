use std::path::{Path, PathBuf};

use anaden_vision::{ScreenScaler, TaskDef, load_pipeline, load_watch_defs};

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

fn fixture_path(name: &str) -> PathBuf {
    workspace_root()
        .join("crates")
        .join("anaden-vision")
        .join("tests")
        .join("fixtures")
        .join(name)
}

fn normalize_live_fixture(name: &str, expected: (u32, u32)) -> image::DynamicImage {
    let raw = image::open(fixture_path(name)).unwrap_or_else(|e| {
        panic!("open tracked fixture {name}: {e} — the night-town capture must ship with the repo")
    });
    assert_eq!(
        (raw.width(), raw.height()),
        expected,
        "{name}: unexpected dims (fixture must be the 4K PrintWindow capture it claims to be)"
    );
    let (normalized, _crop) = ScreenScaler::new().normalize_capture(&raw);
    assert_eq!(normalized.width(), 1280);
    normalized
}

fn watch_and_chain_tasks(pipeline: &str) -> Vec<TaskDef> {
    let dir = workspace_root()
        .join("templates")
        .join("pipelines")
        .join(pipeline);
    let mut tasks = load_pipeline(&dir).unwrap_or_else(|e| panic!("load {pipeline} chain: {e}"));
    tasks.extend(load_watch_defs(&dir).unwrap_or_else(|e| panic!("load {pipeline} watch: {e}")));
    tasks
}

fn sentinel(tasks: &[TaskDef], name: &str) -> TaskDef {
    tasks
        .iter()
        .find(|t| t.name == name)
        .unwrap_or_else(|| panic!("{name} must ship"))
        .clone()
}

fn detect_conf(task: &TaskDef, normalized: &image::DynamicImage) -> Option<f32> {
    task.detect(normalized, Path::new(""))
        .unwrap_or_else(|e| panic!("{} detect error: {e}", task.name))
        .map(|m| m.confidence.0)
}

#[test]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
fn night_town_fixture_keeps_both_town_bar_sentinels_above_threshold() {
    let normalized = normalize_live_fixture("town_night_live_20261111_3783x2128.png", (3783, 2128));

    for (pipeline, name) in [
        ("nav_to_field_pc", "FieldTownBarPc"),
        ("login", "LoginFieldTownBarPc"),
    ] {
        let tasks = watch_and_chain_tasks(pipeline);
        let task = sentinel(&tasks, name);
        let conf = detect_conf(&task, &normalized).unwrap_or_else(|| {
            panic!(
                "{name} must MATCH the 2026-11-11 night town: the sentinel threshold {} is \
                 tuned for the day town fixture only and misses live night scenes",
                task.threshold
            )
        });
        assert!(
            conf >= task.threshold,
            "{name}: conf {conf:.4} below the TOML threshold {}",
            task.threshold
        );
        assert!(
            conf >= 0.75,
            "{name}: conf {conf:.4} degraded below the 0.75 night-scene quality floor \
             (live sweep measured the night town in [0.79, 0.80))"
        );
    }
}

#[test]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
fn day_town_live_fixture_keeps_both_town_bar_sentinels_above_threshold() {
    let normalized = normalize_live_fixture("town_live_20261112_3783x2128.png", (3783, 2128));

    for (pipeline, name) in [
        ("nav_to_field_pc", "FieldTownBarPc"),
        ("login", "LoginFieldTownBarPc"),
    ] {
        let tasks = watch_and_chain_tasks(pipeline);
        let task = sentinel(&tasks, name);
        let conf = detect_conf(&task, &normalized).unwrap_or_else(|| {
            panic!(
                "{name} must MATCH the 2026-11-12 live town (roi={:?} threshold={}): the needle \
                 must survive a town whose terrain differs from the tracked fixtures",
                task.roi, task.threshold
            )
        });
        assert!(
            conf >= task.threshold,
            "{name}: conf {conf:.4} below the TOML threshold {}",
            task.threshold
        );
        assert!(
            conf >= 0.85,
            "{name}: conf {conf:.4} degraded below the 0.85 quality buffer on a live town frame"
        );
    }
}

#[test]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
fn clipped_bottom_capture_still_matches_both_arrival_sentinels() {
    let raw = image::open(fixture_path("town_live_20261112_3783x2128.png"))
        .unwrap_or_else(|e| panic!("open tracked fixture: {e}"));
    let mut clipped = raw.to_rgba8();
    let height = clipped.height();
    for y in (height - 100)..height {
        for x in 0..clipped.width() {
            clipped.put_pixel(x, y, image::Rgba([0, 0, 0, 255]));
        }
    }
    let (normalized, _crop) =
        ScreenScaler::new().normalize_capture(&image::DynamicImage::ImageRgba8(clipped));

    for (pipeline, name) in [
        ("nav_to_field_pc", "FieldTownBarPc"),
        ("login", "LoginFieldTownBarPc"),
    ] {
        let tasks = watch_and_chain_tasks(pipeline);
        let task = sentinel(&tasks, name);
        let conf = detect_conf(&task, &normalized).unwrap_or_else(|| {
            panic!(
                "{name} must MATCH an arrival frame whose bottom 100 px are black (the live \
                 2026-11-12 frames clip the bottom UI row): roi={:?} threshold={}",
                task.roi, task.threshold
            )
        });
        assert!(
            conf >= task.threshold,
            "{name}: conf {conf:.4} below the TOML threshold {}",
            task.threshold
        );
        assert!(
            conf >= 0.85,
            "{name}: conf {conf:.4} degraded below the 0.85 buffer on a clipped frame"
        );
    }
}

#[test]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
fn night_tuned_threshold_stays_dark_on_menu_title_and_login_chain_screens() {
    let menu_probe = normalize_live_fixture("menu_panel_live_2880x1620.png", (2880, 1620));
    let title_probe = normalize_live_fixture("title_live_1952x1098.png", (1952, 1098));
    let title_menu_probe =
        normalize_live_fixture("title_menu_live_20261111_3783x2128.png", (3783, 2128));

    for (pipeline, name) in [
        ("nav_to_field_pc", "FieldTownBarPc"),
        ("login", "LoginFieldTownBarPc"),
    ] {
        let tasks = watch_and_chain_tasks(pipeline);
        let task = sentinel(&tasks, name);
        for (screen, probe) in [
            ("menu", &menu_probe),
            ("title", &title_probe),
            ("title_menu", &title_menu_probe),
        ] {
            let false_conf = detect_conf(&task, probe);
            assert!(
                false_conf.is_none(),
                "{name} must NOT match the {screen} fixture at threshold {} (cross-screen \
                 false positive after the night retune, conf={:?})",
                task.threshold,
                false_conf
            );
        }
    }
}
