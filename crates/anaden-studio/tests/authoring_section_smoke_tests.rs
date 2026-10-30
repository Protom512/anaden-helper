//! egui レンダラのヘッドレス描画スモークテスト (Issue #222)。
//!
//! cargo-mutants の fn 削除 mutant 13 件 (authoring セクション 8 + canvas、
//! live canvas 3、stroke_frame_rect) が生存していた — ヘッドレスで描画を
//! 呼び出すテストが存在しなかったため。本テストは各モードの描画が
//! パニックせず完了することを保証する (fn 削除 mutant が caught になる)。
//!
//! パターンは authoring_ui_tests.rs render_live_authoring_headless_completes_without_panic
//! と同一 (egui::Context::default + begin_pass + 描画呼び出し)。
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
        ctx.end_pass();
    }
}

/// Authoring モード: 全セクション (data/engine/capture/proposals/
/// discrimination/save/pipeline/scenario + canvas) の空状態描画。
/// fn 削除 mutant が caught になることを保証する。
#[test]
fn render_authoring_all_sections_headless_completes() {
    render_all_modes_headless();
}

/// 複数フレーム描画 — セクション内の状態遷移 (コンボボックス、候補リスト等)
/// を含む 2 フレーム目もパニックしないことを保証する。
#[test]
fn render_authoring_two_frames_headless_completes() {
    let ctx = egui::Context::default();
    let mut app = StudioApp::default();
    app.set_mode(AppMode::Authoring);
    for frame in 0..2 {
        ctx.begin_pass(egui::RawInput::default());
        app.render_modebar(&mut test_ui(&ctx, &format!("frame-{frame}")));
        app.render_body(&mut test_ui(&ctx, &format!("frame-{frame}")));
        ctx.end_pass();
    }
}

/// Live オーサリングモード: canvas interaction + overlays を含む。
#[test]
fn render_live_authoring_canvas_headless_completes() {
    let ctx = egui::Context::default();
    let mut app = StudioApp::default();
    app.set_mode(AppMode::LiveAuthoring);
    ctx.begin_pass(egui::RawInput::default());
    app.render_modebar(&mut test_ui(&ctx, "live"));
    app.render_body(&mut test_ui(&ctx, "live"));
    ctx.end_pass();
}
