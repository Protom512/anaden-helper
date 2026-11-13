use std::path::{Path, PathBuf};

use anaden_vision::{Action, ScreenScaler, TaskDef, load_pipeline, roi_to_normalized};

const CLICK_MARGIN_PX: u32 = 8;

const TITLE_FIXTURES: &[&str] = &[
    "title_live_1952x1098.png",
    "title_menu_live_20261111_3783x2128.png",
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

fn load(dir: &str) -> Vec<TaskDef> {
    let path = workspace_root()
        .join("templates")
        .join("pipelines")
        .join(dir);
    load_pipeline(&path).unwrap_or_else(|e| panic!("load {}: {e}", path.display()))
}

fn task<'a>(tasks: &'a [TaskDef], name: &str) -> &'a TaskDef {
    tasks
        .iter()
        .find(|t| t.name == name)
        .unwrap_or_else(|| panic!("{name} must exist"))
}

fn normalize_live_fixture(name: &str) -> image::DynamicImage {
    let path = fixture(name);
    let raw = image::open(&path).unwrap_or_else(|e| panic!("open fixture {}: {e}", path.display()));
    let (normalized, crop) = ScreenScaler::new().normalize_capture(&raw);
    assert_eq!(
        (crop.offset_x, crop.offset_y),
        (10, 0),
        "{name}: tracked live captures carry a 10px left black band"
    );
    normalized
}

fn click_center(task: &TaskDef, norm_w: u32, norm_h: u32) -> (u32, u32) {
    let action = task
        .action
        .as_ref()
        .unwrap_or_else(|| panic!("{} must declare an action", task.name));
    match action {
        Action::ClickRect { roi } => {
            let n = roi_to_normalized([roi.x, roi.y, roi.width, roi.height], norm_w, norm_h);
            (n[0] + n[2] / 2, n[1] + n[3] / 2)
        }
        other => panic!("{} must use click_rect, got {other:?}", task.name),
    }
}

#[test]
#[allow(clippy::unwrap_used)]
fn load_game_click_lands_inside_detected_load_label_on_live_titles() {
    let nav = load("nav_to_field_pc");
    let login = load("login");
    let load_game = task(&nav, "LoadGamePc");
    let label = task(&login, "LoginClickLoadPc");

    let mut checked = 0usize;
    for fixture_name in TITLE_FIXTURES {
        let normalized = normalize_live_fixture(fixture_name);
        let (w, h) = (normalized.width(), normalized.height());

        let label_match = label
            .detect(&normalized, Path::new(""))
            .unwrap_or_else(|e| panic!("{} detect error on {fixture_name}: {e}", label.name))
            .unwrap_or_else(|| panic!("{} must find the load label on {fixture_name}", label.name));
        let anchor_match = load_game
            .detect(&normalized, Path::new(""))
            .unwrap_or_else(|e| panic!("{} detect error on {fixture_name}: {e}", load_game.name))
            .unwrap_or_else(|| {
                panic!(
                    "{} must match {fixture_name} (roi={:?})",
                    load_game.name, load_game.roi
                )
            });
        assert!(
            anchor_match.confidence.0 >= load_game.threshold,
            "{}: conf {} below threshold {}",
            load_game.name,
            anchor_match.confidence.0,
            load_game.threshold
        );

        let (cx, cy) = click_center(load_game, w, h);
        let box_ = label_match.region;
        assert!(
            box_.contains(cx, cy),
            "{fixture_name}: {} clicks ({cx},{cy}) which is OUTSIDE the detected load-label \
             box [{},{},{},{}] ({}x{} frame)",
            load_game.name,
            box_.x,
            box_.y,
            box_.width,
            box_.height,
            w,
            h
        );

        let margins = [
            cx.saturating_sub(box_.x),
            box_.right().saturating_sub(cx),
            cy.saturating_sub(box_.y),
            box_.bottom().saturating_sub(cy),
        ];
        assert!(
            margins.iter().all(|m| *m >= CLICK_MARGIN_PX),
            "{fixture_name}: click ({cx},{cy}) is too close to the load-label edge \
             (margins {margins:?}, need >= {CLICK_MARGIN_PX}px) — the static click_rect \
             must stay inside the label under title drift"
        );
        checked += 1;
    }
    assert_eq!(
        checked,
        TITLE_FIXTURES.len(),
        "every title fixture must be exercised (vacuous-pass guard)"
    );
}
