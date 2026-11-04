use std::path::{Path, PathBuf};

use anaden_vision::{CropInfo, ScreenScaler, TaskDef, crop_to_content_with_info, load_pipeline};

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

fn degenerate_fixture() -> PathBuf {
    fixtures_dir().join("title_boot_degenerate_1952x1098.png")
}

fn healthy_fixture() -> PathBuf {
    fixtures_dir().join("title_live_1952x1098.png")
}

fn login_tap_title_task() -> TaskDef {
    let login_dir = workspace_root()
        .join("templates")
        .join("pipelines")
        .join("login");
    let tasks = load_pipeline(&login_dir)
        .unwrap_or_else(|e| panic!("load login pipeline {}: {e}", login_dir.display()));
    tasks
        .into_iter()
        .find(|t| t.name == "LoginTapTitlePc")
        .expect("LoginTapTitlePc must exist in templates/pipelines/login")
}

#[test]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
fn degenerate_boot_frame_preserves_geometry_and_reports_nomatch() {
    let raw = image::open(degenerate_fixture()).unwrap_or_else(|e| {
        panic!(
            "open degenerate fixture {}: {e} (tests/fixtures/title_boot_degenerate_1952x1098.png \
             is tracked — a fresh clone must contain it)",
            degenerate_fixture().display()
        )
    });
    assert_eq!(
        (raw.width(), raw.height()),
        (1952, 1098),
        "degenerate fixture must be the 1952x1098 PrintWindow capture (Issue #212 evidence)"
    );

    let (raw_cropped, _) = crop_to_content_with_info(&raw);
    assert_eq!(
        (raw_cropped.width(), raw_cropped.height()),
        (1952, 577),
        "unguarded crop eats the unpainted in-canvas black (the Issue #212 trap)"
    );

    let (normalized, crop_info) = ScreenScaler::new().normalize_capture(&raw);
    assert_eq!(
        crop_info,
        CropInfo::full(1952, 1098),
        "aspect guard must reject the geometry-destroying crop"
    );
    assert_eq!(
        (normalized.width(), normalized.height()),
        (1280, 720),
        "geometry must be preserved: full 16:9 canvas normalizes to 1280x720, not 1280x378"
    );

    let task = login_tap_title_task();
    let matched = task
        .detect(&normalized, Path::new(""))
        .expect("LoginTapTitlePc detect must not error");
    assert!(
        matched.is_none(),
        "degenerate boot frame must honestly report NoMatch (needle absent: full-scan best \
         conf 0.31 < threshold {}). A match here would be a false positive.",
        task.threshold
    );
}

#[test]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
fn healthy_title_geometry_and_degenerate_geometry_both_green() {
    let task = login_tap_title_task();

    let healthy = image::open(healthy_fixture())
        .unwrap_or_else(|e| panic!("open healthy fixture {}: {e}", healthy_fixture().display()));
    let (healthy_norm, healthy_info) = ScreenScaler::new().normalize_capture(&healthy);
    assert_eq!((healthy_info.offset_x, healthy_info.offset_y), (10, 0));
    assert_eq!((healthy_info.width, healthy_info.height), (1942, 1098));
    assert_eq!((healthy_norm.width(), healthy_norm.height()), (1280, 724));
    let matched = task
        .detect(&healthy_norm, Path::new(""))
        .expect("detect on healthy fixture must not error")
        .expect("healthy title fixture must MATCH through normalize_capture");
    assert!(
        matched.confidence.0 >= task.threshold,
        "healthy fixture confidence {} must exceed threshold {}",
        matched.confidence.0,
        task.threshold
    );

    let degenerate = image::open(degenerate_fixture()).unwrap_or_else(|e| {
        panic!(
            "open degenerate fixture {}: {e}",
            degenerate_fixture().display()
        )
    });
    let (degenerate_norm, degenerate_info) = ScreenScaler::new().normalize_capture(&degenerate);
    assert_eq!(degenerate_info, CropInfo::full(1952, 1098));
    assert_eq!(
        (degenerate_norm.width(), degenerate_norm.height()),
        (1280, 720),
        "degenerate geometry must stay 16:9-normalized (not squeezed)"
    );
    assert!(
        task.detect(&degenerate_norm, Path::new(""))
            .expect("detect on degenerate fixture must not error")
            .is_none(),
        "degenerate frame must be NoMatch (needle absent)"
    );
}
