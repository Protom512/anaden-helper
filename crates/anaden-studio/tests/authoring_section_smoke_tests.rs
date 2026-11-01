use anaden_studio::app::{AppMode, StudioApp};
use eframe::egui;

fn test_ui(ctx: &egui::Context, id: &str) -> egui::Ui {
    egui::Ui::new(
        ctx.clone(),
        egui::Id::new(id),
        egui::UiBuilder::new().max_rect(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(800.0, 600.0),
        )),
    )
}

fn render_all_modes_headless() {
    let ctx = egui::Context::default();
    for (mode, label) in [
        (AppMode::Authoring, "authoring"),
        (AppMode::LiveAuthoring, "live-authoring"),
        (AppMode::Batch, "batch"),
    ] {
        let mut app = StudioApp::default();
        app.set_mode(mode);
        ctx.begin_pass(egui::RawInput::default());
        app.render_modebar(&mut test_ui(&ctx, label));
        app.render_body(&mut test_ui(&ctx, label));
        let _ = ctx.end_pass();
    }
}

#[test]
fn render_authoring_all_sections_headless_completes() {
    render_all_modes_headless();
}

#[test]
fn render_authoring_two_frames_headless_completes() {
    let ctx = egui::Context::default();
    let mut app = StudioApp::default();
    app.set_mode(AppMode::Authoring);
    for frame in 0..2 {
        ctx.begin_pass(egui::RawInput::default());
        app.render_modebar(&mut test_ui(&ctx, &format!("frame-{frame}")));
        app.render_body(&mut test_ui(&ctx, &format!("frame-{frame}")));
        let _ = ctx.end_pass();
    }
}

#[test]
fn render_live_authoring_canvas_headless_completes() {
    let ctx = egui::Context::default();
    let mut app = StudioApp::default();
    app.set_mode(AppMode::LiveAuthoring);
    ctx.begin_pass(egui::RawInput::default());
    app.render_modebar(&mut test_ui(&ctx, "live"));
    app.render_body(&mut test_ui(&ctx, "live"));
    let _ = ctx.end_pass();
}
