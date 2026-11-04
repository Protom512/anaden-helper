#![allow(clippy::unwrap_used)]
#![allow(clippy::panic)]
#![allow(clippy::expect_used)]

use anaden_studio::app::{AppMode, StudioApp};
use anaden_studio::authoring_session::GestureOutcome;
use anaden_studio::authoring_ui::{AuthoringPanel, PanelGesture, RecordingInjector};
use image::{DynamicImage, GrayImage, Luma};

fn gradient_frame(w: u32, h: u32, seed: u32) -> DynamicImage {
    let mut img = GrayImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let v = ((x * 2 + y * 3 + seed) % 200) as u8;
            img.put_pixel(x, y, Luma([v]));
        }
    }
    DynamicImage::ImageLuma8(img)
}

fn started_panel() -> AuthoringPanel {
    let mut panel = AuthoringPanel::new();
    panel.start_session();
    panel.push_frame(&gradient_frame(1280, 720, 0), Some((1258, 708)));
    panel
}

#[test]
fn inject_toggle_defaults_off() {
    let panel = AuthoringPanel::new();
    assert!(!panel.inject_enabled());
}

#[test]
fn set_inject_enabled_updates_state() {
    let mut panel = AuthoringPanel::new();
    panel.set_inject_enabled(true);
    assert!(panel.inject_enabled());
    panel.set_inject_enabled(false);
    assert!(!panel.inject_enabled());
}

#[test]
fn canvas_tap_with_toggle_on_injects_once_with_mapped_coords() {
    let mut panel = started_panel();
    panel.set_inject_enabled(true);
    let mut rec = RecordingInjector::new();

    let gesture = panel.canvas_tap((640, 360), &mut rec);

    let PanelGesture::Handled {
        outcome,
        record_error,
        injected,
        inject_error,
    } = gesture
    else {
        panic!("started session must handle the tap: {gesture:?}")
    };
    assert!(matches!(outcome, GestureOutcome::Pending));
    assert!(record_error.is_none());
    assert!(injected, "toggle on must inject");
    assert!(inject_error.is_none(), "inject_error: {inject_error:?}");
    assert_eq!(rec.clicks, vec![(629, 354)], "mapped client coords");
    assert_eq!(panel.session().unwrap().pending_tap(), Some((640, 360)));
}

#[test]
fn multiple_taps_record_and_inject_in_order() {
    let mut panel = started_panel();
    panel.set_inject_enabled(true);
    let mut rec = RecordingInjector::new();

    panel.canvas_region([10, 20, 100, 50]);
    let _ = panel.canvas_tap((60, 45), &mut rec);
    let _ = panel.canvas_tap((200, 100), &mut rec);
    panel.canvas_region([150, 80, 120, 60]);

    assert_eq!(rec.clicks, vec![(59, 44), (197, 98)]);
    assert_eq!(
        panel.session().unwrap().steps().len(),
        2,
        "2 steps confirmed"
    );
}

#[test]
fn region_then_tap_confirms_step_via_panel() {
    let mut panel = started_panel();

    let region = panel.canvas_region([10, 20, 100, 50]);
    let PanelGesture::Handled {
        outcome, injected, ..
    } = region
    else {
        panic!("region must be handled: {region:?}")
    };
    assert!(matches!(outcome, GestureOutcome::Pending));
    assert!(!injected, "region gesture must not inject");

    let tap = panel.canvas_tap((60, 45), &mut RecordingInjector::new());
    let PanelGesture::Handled { outcome, .. } = tap else {
        panic!("tap must be handled: {tap:?}")
    };
    let GestureOutcome::Confirmed { warnings } = outcome else {
        panic!("both gestures must confirm: {outcome:?}")
    };
    assert!(warnings.is_empty(), "warnings: {warnings:?}");
    assert_eq!(panel.session().unwrap().steps().len(), 1);
    assert_eq!(
        panel.session().unwrap().steps()[0].task.roi,
        Some([10, 20, 100, 50])
    );
}

#[test]
fn undo_via_panel_clears_pending_then_last_step() {
    let mut panel = started_panel();
    panel.canvas_region([10, 20, 100, 50]);
    assert!(panel.session().unwrap().pending_region().is_some());

    assert!(panel.undo(), "pending gesture cleared");
    assert!(panel.session().unwrap().pending_region().is_none());

    panel.canvas_region([10, 20, 100, 50]);
    let _ = panel.canvas_tap((60, 45), &mut RecordingInjector::new());
    assert_eq!(panel.session().unwrap().steps().len(), 1);
    assert!(panel.undo(), "last step removed");
    assert_eq!(panel.session().unwrap().steps().len(), 0);
    assert!(!panel.undo(), "empty session undo is false");
}

#[test]
fn panel_save_roundtrips_scenario() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("pipelines");

    let mut panel = started_panel();
    panel.canvas_region([10, 20, 100, 50]);
    let _ = panel.canvas_tap((60, 45), &mut RecordingInjector::new());

    let outcome = panel
        .save(&root)
        .expect("session started")
        .expect("save ok");
    assert_eq!(outcome.dir, root.join("MyFirstAuthoredRun"));
    assert!(outcome.dir.join("AuthoredStep01.png").exists());
    assert!(outcome.dir.join("AuthoredStep01.toml").exists());
    assert!(
        panel.status().contains("保存しました"),
        "{}",
        panel.status()
    );
}

#[test]
fn push_frame_feeds_session() {
    let mut panel = AuthoringPanel::new();
    assert!(panel.session().is_none());
    panel.push_frame(&gradient_frame(64, 48, 1), None);
    panel.start_session();
    panel.push_frame(&gradient_frame(1280, 720, 2), Some((1258, 708)));
    panel.canvas_region([0, 0, 40, 30]);
    let tap = panel.canvas_tap((10, 10), &mut RecordingInjector::new());
    assert!(
        matches!(
            tap,
            PanelGesture::Handled {
                outcome: GestureOutcome::Confirmed { .. },
                ..
            }
        ),
        "supplied frame must enable confirmation: {tap:?}"
    );
}

#[test]
fn start_session_uses_input_name_and_fallback() {
    let mut named = AuthoringPanel::new();
    named.set_name("  demo_live  ");
    named.start_session();
    assert!(named.is_started());
    assert_eq!(named.session().unwrap().name(), "demo_live");

    let mut blank = AuthoringPanel::new();
    blank.set_name("   ");
    blank.start_session();
    assert_eq!(blank.session().unwrap().name(), "MyFirstAuthoredRun");

    let panel = AuthoringPanel::new();
    assert!(!panel.is_started());
    assert_eq!(panel.name(), "MyFirstAuthoredRun");
}

#[test]
fn toggle_off_yields_zero_injections() {
    let mut panel = started_panel();
    assert!(!panel.inject_enabled(), "default off");
    let mut rec = RecordingInjector::new();

    let gesture = panel.canvas_tap((640, 360), &mut rec);
    let PanelGesture::Handled { injected, .. } = gesture else {
        panic!("must be handled: {gesture:?}")
    };
    assert!(!injected);
    assert!(rec.clicks.is_empty(), "no injection while disabled");
    assert_eq!(panel.session().unwrap().pending_tap(), Some((640, 360)));
}

#[test]
fn gesture_without_session_is_discarded() {
    let mut panel = AuthoringPanel::new();
    let mut rec = RecordingInjector::new();
    assert_eq!(
        panel.canvas_tap((10, 10), &mut rec),
        PanelGesture::Discarded
    );
    assert_eq!(panel.canvas_region([0, 0, 10, 10]), PanelGesture::Discarded);
    assert!(rec.clicks.is_empty());
    assert!(panel.status().contains("記録開始"), "{}", panel.status());
}

#[test]
fn injection_failure_is_fail_visible() {
    let mut panel = started_panel();
    panel.set_inject_enabled(true);
    panel.canvas_region([10, 20, 100, 50]);
    let mut rec = RecordingInjector {
        fail_next: true,
        ..RecordingInjector::new()
    };

    let gesture = panel.canvas_tap((60, 45), &mut rec);
    let PanelGesture::Handled {
        outcome,
        injected,
        inject_error,
        ..
    } = gesture
    else {
        panic!("must be handled: {gesture:?}")
    };
    assert!(!injected);
    let Some(err) = inject_error else {
        panic!("injection failure must be reported")
    };
    assert!(err.contains("注入失敗"), "{err}");
    assert!(panel.status().contains("注入失敗"), "{}", panel.status());
    assert!(
        matches!(outcome, GestureOutcome::Confirmed { .. }),
        "recording must still confirm: {outcome:?}"
    );
    assert_eq!(panel.session().unwrap().steps().len(), 1);
    assert!(
        rec.clicks.is_empty(),
        "failed click is not recorded as sent"
    );
}

#[test]
fn injection_without_client_dims_reports_mapping_error() {
    let mut panel = AuthoringPanel::new();
    panel.start_session();
    panel.push_frame(&gradient_frame(1280, 720, 0), None);
    panel.set_inject_enabled(true);
    let mut rec = RecordingInjector::new();

    let gesture = panel.canvas_tap((640, 360), &mut rec);
    let PanelGesture::Handled {
        injected,
        inject_error,
        ..
    } = gesture
    else {
        panic!("must be handled: {gesture:?}")
    };
    assert!(!injected);
    let Some(err) = inject_error else {
        panic!("missing client dims must be reported")
    };
    assert!(err.contains("注入座標"), "{err}");
    assert!(rec.clicks.is_empty());
}

#[test]
fn region_outside_frame_is_rejected_with_record_error() {
    let mut panel = started_panel();
    let gesture = panel.canvas_region([1300, 0, 50, 50]);
    let PanelGesture::Handled { record_error, .. } = gesture else {
        panic!("must be handled: {gesture:?}")
    };
    let Some(err) = record_error else {
        panic!("out-of-frame region must report record_error")
    };
    assert!(err.contains("does not intersect"), "{err}");
    assert!(panel.session().unwrap().pending_region().is_none());
}

#[test]
fn save_without_session_returns_none() {
    let mut panel = AuthoringPanel::new();
    let tmp = tempfile::tempdir().expect("tempdir");
    assert!(panel.save(tmp.path()).is_none());
    assert!(panel.status().contains("未開始"), "{}", panel.status());
}

#[test]
fn render_live_authoring_headless_completes_without_panic() {
    let ctx = eframe::egui::Context::default();
    let mut app = StudioApp::default();
    app.set_mode(AppMode::LiveAuthoring);
    assert_eq!(app.mode(), AppMode::LiveAuthoring);

    let child = |ctx: &eframe::egui::Context| {
        eframe::egui::Ui::new(
            ctx.clone(),
            eframe::egui::Id::new("authoring-ui-test"),
            eframe::egui::UiBuilder::new().max_rect(eframe::egui::Rect::from_min_size(
                eframe::egui::Pos2::ZERO,
                eframe::egui::vec2(800.0, 600.0),
            )),
        )
    };
    ctx.begin_pass(eframe::egui::RawInput::default());
    app.render_modebar(&mut child(&ctx));
    app.render_body(&mut child(&ctx));
    let _ = ctx.end_pass();
}
