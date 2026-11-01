use std::path::{Path, PathBuf};

use anaden_vision::{ScreenScaler, TaskDef, load_pipeline};

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

fn title_live_fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("title_live_1952x1098.png")
}

#[test]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
fn title_live_shot_matches_login_task_through_production_preprocessing() {
    let fixture = title_live_fixture();
    let raw = image::open(&fixture).unwrap_or_else(|e| {
        panic!(
            "open live title fixture {}: {e} (tests/fixtures/title_live_1952x1098.png is \
             tracked — a fresh clone must contain it)",
            fixture.display()
        )
    });

    assert_eq!(
        (raw.width(), raw.height()),
        (1952, 1098),
        "live title fixture must be the 1952x1098 PrintWindow capture (Issue #208 evidence)"
    );

    let (normalized, crop_info) = ScreenScaler::new().normalize_capture(&raw);
    assert_eq!(
        (crop_info.offset_x, crop_info.offset_y),
        (10, 0),
        "letterbox crop must remove the 10px left inset (8px black bar + MARGIN_PX 2) \
         on this fixture"
    );
    assert_eq!((crop_info.width, crop_info.height), (1942, 1098));
    assert_eq!(
        (normalized.width(), normalized.height()),
        (1280, 724),
        "normalize must produce the 1280-base frame for this fixture"
    );

    let login_dir = workspace_root()
        .join("templates")
        .join("pipelines")
        .join("login");
    let tasks = load_pipeline(&login_dir)
        .unwrap_or_else(|e| panic!("load login pipeline {}: {e}", login_dir.display()));
    let task: &TaskDef = tasks
        .iter()
        .find(|t| t.name == "LoginTapTitlePc")
        .expect("LoginTapTitlePc must exist in templates/pipelines/login");

    let matched = task
        .detect(&normalized, Path::new(""))
        .unwrap_or_else(|e| panic!("LoginTapTitlePc detect error: {e}"))
        .unwrap_or_else(|| {
            panic!(
                "LoginTapTitlePc must MATCH the live title fixture through the production \
                 preprocessing (crop_to_content -> normalize -> roi detect). Issue #208 \
                 regression: roi={:?} threshold={}",
                task.roi, task.threshold
            )
        });
    assert!(
        matched.confidence.0 >= task.threshold,
        "confidence {} must exceed the TOML threshold {} on the live title fixture",
        matched.confidence.0,
        task.threshold
    );
    assert!(
        matched.confidence.0 >= 0.95,
        "confidence {} degraded well below the measured 0.9944 — the template asset or \
         preprocessing may have regressed (Issue #208)",
        matched.confidence.0
    );
    assert_eq!(
        (matched.region.x, matched.region.y),
        (637, 268),
        "best placement of the logo_corner needle on this fixture must stay at (637,268) \
         (measured Issue #208); got ({},{})",
        matched.region.x,
        matched.region.y
    );
    assert_eq!(
        (matched.region.width, matched.region.height),
        (122, 123),
        "needle 120x120 scales to 122x123 in the 1280x724 normalized space"
    );
}

#[test]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
fn version_label_template_is_free_of_full_width_dark_rows() {
    let tpl_path = workspace_root()
        .join("templates")
        .join("scenes")
        .join("title_pc")
        .join("version_label.png");
    let tpl = image::open(&tpl_path).unwrap_or_else(|e| panic!("open {}: {e}", tpl_path.display()));
    let luma = tpl.to_luma8();
    let w = luma.width();

    const DARK_LUMA: u8 = 100;
    const MAX_DARK_ROW_FRACTION: f64 = 0.90;
    for y in 0..luma.height() {
        let dark = (0..w)
            .filter(|&x| luma.get_pixel(x, y)[0] < DARK_LUMA)
            .count();
        let fraction = dark as f64 / w as f64;
        assert!(
            fraction < MAX_DARK_ROW_FRACTION,
            "version_label.png row {y} is {:.0}% dark pixels — the template appears to \
             contain a full-width dark row (a screen-specific separator line). Issue #208: \
             such rows exist only on non-title screens and collapse ccoeff on the pure \
             title screen (0.94 -> 0.47). Regenerate the template from a real title \
             capture (resize_crop_template --letterbox 10 --roi 8,2,138,20)",
            fraction * 100.0
        );
    }
}
