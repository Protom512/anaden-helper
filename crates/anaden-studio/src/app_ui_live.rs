//! StudioApp の実演オーサリングモード本体描画 (Issue #190 Shard 2/3)。
//!
//! ドメイン状態は [`crate::authoring_ui::AuthoringPanel`] (egui 非依存)・
//! 座標変換は [`crate::authoring_coords`]・モードディスパッチは
//! [`crate::app_state::StudioApp::render_body`]。本モジュールは配線のみ:
//!
//! - フレーム供給: 既存ライブキャプチャ (`self.live`) の最新フレームを正規化して
//!   `screenshot` へ置くと同時にセッションへ push する (新規 capture スレッド無し)。
//! - ライブビュー: クリック → タップ記録 + トグル有効時の入力注入・
//!   ドラッグ → 認識領域選択 ([`crate::canvas::show`] の選択パターン踏襲)。
//! - ステップリスト・undo・保存 (警告表示付き)。

use std::sync::Arc;

use eframe::egui;

use crate::app_state::StudioApp;
use crate::authoring_coords::{self, ViewRect};
use crate::authoring_ui::AuthoringInputInjector;
#[cfg(not(windows))]
use crate::authoring_ui::UnavailableAuthoringInjector;
use crate::source::LiveCapture;

impl StudioApp {
    /// 実演オーサリングモードの本体を描画する (render_body の dispatch 先)。
    pub(crate) fn render_live_authoring(&mut self, ui: &mut egui::Ui) {
        // ライブキャプチャの最新フレーム取り込み (作成モードと同一経路・共有 1 スレッド)。
        // 正規化して screenshot へ置くと同時にオーサリングセッションへ供給し、
        // 生キャプチャ寸法 (= PC版クライアント領域寸法) も注入座標変換用に記録する。
        if let Some(live) = &self.live
            && let Some(frame) = live.latest()
        {
            let client_dims = (frame.width(), frame.height());
            let normalized = self.scaler.normalize(&frame);
            self.screenshot = Some(Arc::new(normalized.clone()));
            self.screenshot_tex = None;
            self.live_authoring
                .push_frame(&normalized, Some(client_dims));
        }

        // 左サイドパネル: キャプチャ制御 + セッション + ステップ一覧。
        egui::Panel::left("live_authoring_controls")
            .resizable(true)
            .default_size(320.0)
            .show_inside(ui, |ui| {
                self.live_authoring_side(ui);
            });

        // 中央: ライブビュー (クリック/ドラッグのジェスチャ受付)。
        egui::CentralPanel::default().show_inside(ui, |ui| {
            self.live_authoring_canvas(ui);
        });
    }

    /// 実演オーサリング用の入力注入器を作る。
    ///
    /// Windows ビルドのみ実注入 (SendInput)。非 Windows ビルドは常にエラーを返す
    /// スタブ (fail-visible)。Issue #188 で Android 取得元は削除済み。
    fn make_authoring_injector(&self) -> Box<dyn AuthoringInputInjector> {
        #[cfg(windows)]
        {
            Box::new(crate::authoring_ui::Win32AuthoringInjector::new(
                self.win_exe.trim(),
            ))
        }
        #[cfg(not(windows))]
        {
            Box::new(UnavailableAuthoringInjector::new(
                "入力注入は Windows(PC版) キャプチャでのみ対応しています",
            ))
        }
    }

    /// 左サイドパネル (キャプチャ制御・セッション操作・ステップ一覧)。
    fn live_authoring_side(&mut self, ui: &mut egui::Ui) {
        ui.heading("anaden-studio");
        ui.label("実演オーサリング");
        ui.separator();

        // -- ライブキャプチャ (作成モードと同じ self.live / target / serial / exe を共有。
        //    コンパクト版 — 接続チェック等のフル制御は作成タブ) --
        ui.heading("ライブキャプチャ");
        ui.horizontal(|ui| {
            ui.label("取得元:");
            ui.label("Windows(PC版) — Win32Capture");
        });
        ui.horizontal(|ui| {
            ui.label("exe名:");
            ui.add(egui::TextEdit::singleline(&mut self.win_exe).desired_width(160.0));
        });
        if self.live.is_some() {
            if ui.button("停止（この画面で固定）").clicked() {
                self.live = None;
                self.status = "ライブ停止: 現在の画面で固定しました".to_string();
            }
        } else {
            let can_start = !self.win_exe.trim().is_empty();
            ui.add_enabled_ui(can_start, |ui| {
                if ui.button("ライブ開始").clicked() {
                    self.live = Some(LiveCapture::start(800, self.win_exe.trim()));
                    self.status = "PC版キャプチャ中…".to_string();
                }
            });
        }
        ui.separator();

        // -- セッション (名前入力 → 記録開始 → 注入トグル → undo / 保存) --
        ui.heading("セッション");
        let started = self.live_authoring.is_started();
        ui.add_enabled_ui(!started, |ui| {
            ui.horizontal(|ui| {
                ui.label("名前:");
                ui.text_edit_singleline(&mut self.live_authoring.name);
            });
            if ui.button("記録開始").clicked() {
                self.live_authoring.start_session();
            }
        });
        if !started {
            ui.label("保存先: templates/pipelines/<名前>/");
        }
        let mut inject = self.live_authoring.inject_enabled();
        ui.checkbox(&mut inject, "オーサリングモード有効 (クリックを実機へ注入)");
        if inject != self.live_authoring.inject_enabled() {
            self.live_authoring.set_inject_enabled(inject);
        }
        if self.live_authoring.inject_enabled() {
            ui.colored_label(
                egui::Color32::from_rgb(220, 60, 60),
                "警告: ライブビューのクリックがゲームを操作します",
            );
        }
        if ui.button("元に戻す").clicked() {
            self.live_authoring.undo();
        }
        let can_save = self
            .live_authoring
            .session()
            .is_some_and(|s| !s.steps().is_empty());
        ui.add_enabled_ui(can_save, |ui| {
            if ui.button("シナリオ保存").clicked() {
                let root = Self::workspace_root().join("templates/pipelines");
                self.live_authoring.save(&root);
            }
        });
        ui.separator();

        // -- status + セッション警告 (fail-visible) --
        ui.label(&self.live_authoring.status);
        if let Some(session) = self.live_authoring.session() {
            if !session.warnings().is_empty() {
                ui.colored_label(
                    egui::Color32::from_rgb(230, 160, 30),
                    format!("警告 ({} 件):", session.warnings().len()),
                );
                for w in session.warnings() {
                    ui.small(w);
                }
            }
            ui.separator();

            // -- ステップ一覧 (順番・サムネイル・警告は上の累積リスト参照) --
            ui.heading(format!("ステップ ({} 件)", session.steps().len()));
            for (i, step) in session.steps().iter().enumerate() {
                egui::CollapsingHeader::new(format!("[{}] {}", i + 1, step.task.name))
                    .default_open(i + 1 == session.steps().len())
                    .show(ui, |ui| {
                        if let Some([x, y, w, h]) = step.task.roi {
                            ui.label(format!("roi: ({x},{y}) {w}x{h}"));
                        }
                        ui.label(format!("tap: ({},{})", step.tap.0, step.tap.1));
                        // テンプレートサムネイル (開いているステップのみ毎フレーム生成)。
                        let rgba = step.template.to_rgba8();
                        let size = [rgba.width() as usize, rgba.height() as usize];
                        let image = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
                        let tex = ui.ctx().load_texture(
                            format!("authored-step-{i}"),
                            image,
                            egui::TextureOptions::default(),
                        );
                        let scale = thumbnail_scale(size, 160.0);
                        ui.add(
                            egui::Image::from_texture(egui::load::SizedTexture::new(
                                tex.id(),
                                [size[0] as f32 * scale, size[1] as f32 * scale],
                            ))
                            .maintain_aspect_ratio(true),
                        );
                    });
            }
            // 未確定ジェスチャのガイド。
            match (session.pending_tap(), session.pending_region()) {
                (Some(t), None) => {
                    ui.label(format!(
                        "タップ ({},{}) 記録済み — ドラッグで領域を確定",
                        t.0, t.1
                    ));
                }
                (None, Some(r)) => {
                    ui.label(format!("領域 {r:?} 記録済み — クリックでタップを確定"));
                }
                _ => {}
            }
        }
    }

    /// 中央ライブビュー (クリック = タップ + 注入 / ドラッグ = 認識領域)。
    fn live_authoring_canvas(&mut self, ui: &mut egui::Ui) {
        let (Some(tex), Some(img)) = (&self.screenshot_tex, &self.screenshot) else {
            ui.heading("「ライブ開始」でキャプチャを始めてください");
            return;
        };
        let (w, h) = (img.width(), img.height());
        let frame_dims = (w, h);
        let avail = ui.available_size();
        let aspect = w as f32 / h as f32;
        // アスペクト比を保って available size に収める (canvas.rs と同一式)。
        let mut display = avail;
        if display.x / display.y > aspect {
            display.x = display.y * aspect;
        } else {
            display.y = display.x / aspect;
        }

        let img_widget = egui::Image::from_texture(egui::load::SizedTexture::new(
            tex.id(),
            [w as f32, h as f32],
        ))
        .fit_to_exact_size(display)
        .maintain_aspect_ratio(false)
        .tint(egui::Color32::WHITE)
        .sense(egui::Sense::click_and_drag());
        let response = ui.add(img_widget);
        let rect = response.rect;
        let view = ViewRect {
            left: rect.left(),
            top: rect.top(),
            width: rect.width(),
            height: rect.height(),
        };
        let painter = ui.painter_at(rect);
        let to_frame = |p: egui::Pos2| -> Option<(u32, u32)> {
            authoring_coords::widget_to_frame((p.x, p.y), view, frame_dims)
        };

        // ドラッグ → 認識領域選択 (canvas::show の RoiEdit 更新パターン踏襲)。
        {
            let drag = self.live_authoring.region_drag();
            drag.dragging = response.dragged();
            if response.drag_started() {
                drag.anchor = response.interact_pointer_pos().and_then(to_frame);
                drag.current = drag.anchor;
            }
            if response.dragged()
                && let Some(pos) = response.interact_pointer_pos().and_then(to_frame)
            {
                drag.current = Some(pos);
            }
        }
        // ドラッグ確定 (リリース) → セッションへ領域を記録。
        if response.drag_stopped()
            && let Some(region) = self.live_authoring.region_drag().rect()
        {
            self.live_authoring
                .canvas_region([region.x, region.y, region.width, region.height]);
        }

        // クリック → タップ記録 + トグル有効時の実機注入。
        if response.clicked()
            && let Some(pos) = response.interact_pointer_pos().and_then(to_frame)
        {
            let mut injector = self.make_authoring_injector();
            self.live_authoring.canvas_tap(pos, &mut *injector);
        }

        // ドラッグ中/直近の選択矩形 (黄) — canvas.rs と同じ強調色。
        if let Some(r) = self.live_authoring.region_drag().rect() {
            stroke_frame_rect(
                &painter,
                view,
                frame_dims,
                (r.x, r.y),
                (r.right(), r.bottom()),
                egui::Color32::YELLOW,
            );
        }
        if let Some(session) = self.live_authoring.session() {
            // 未確定の認識領域 (シアン)。
            if let Some([x, y, w, h]) = session.pending_region() {
                stroke_frame_rect(
                    &painter,
                    view,
                    frame_dims,
                    (x, y),
                    (x + w, y + h),
                    egui::Color32::CYAN,
                );
            }
            // 未確定のタップ位置 (赤丸)。
            if let Some((tx, ty)) = session.pending_tap() {
                let c = authoring_coords::frame_to_widget((tx, ty), view, frame_dims);
                painter.circle_stroke(
                    egui::pos2(c.0, c.1),
                    8.0,
                    egui::Stroke::new(2.0, egui::Color32::RED),
                );
            }
        }
    }
}

/// フレームピクセル座標 2 点で囲む矩形をウィジェット上に描く (オーバーレイ共通)。
fn stroke_frame_rect(
    painter: &egui::Painter,
    view: ViewRect,
    frame: (u32, u32),
    tl: (u32, u32),
    br: (u32, u32),
    color: egui::Color32,
) {
    let a = authoring_coords::frame_to_widget(tl, view, frame);
    let b = authoring_coords::frame_to_widget(br, view, frame);
    painter.rect_stroke(
        egui::Rect::from_two_pos(egui::pos2(a.0, a.1), egui::pos2(b.0, b.1)),
        0.0,
        egui::Stroke::new(2.0, color),
        egui::StrokeKind::Outside,
    );
}

/// サムネイルの表示倍率 (長辺を `max_edge` ポイントに収める)。
fn thumbnail_scale(size: [usize; 2], max_edge: f32) -> f32 {
    let long = size[0].max(size[1]) as f32;
    if long <= 0.0 {
        1.0
    } else {
        (max_edge / long).min(1.0)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::app_state::AppMode;
    use image::{DynamicImage, GrayImage, Luma};

    /// 構造あり合成フレーム (Shard 1 テスト流用)。
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

    /// ヘッドレス egui コンテキスト内に子 Ui を作る (app_ui_body テスト流用)。
    fn child_ui(ctx: &egui::Context) -> egui::Ui {
        egui::Ui::new(
            ctx.clone(),
            egui::Id::new("app-ui-live-test"),
            egui::UiBuilder::new().max_rect(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800.0, 600.0),
            )),
        )
    }

    /// スクリーンショット + セッション開始 + 未確定ジェスチャ (領域のみ) の状態で
    /// ライブビュー描画 (オーバーレイ含む) がパニックせず完了すること。
    #[test]
    fn render_live_authoring_with_frame_and_pending_gestures_completes() {
        let ctx = egui::Context::default();
        let mut app = StudioApp {
            screenshot: Some(Arc::new(gradient_frame(1280, 720, 3))),
            ..StudioApp::default()
        };
        app.live_authoring.start_session();
        app.live_authoring
            .push_frame(&gradient_frame(1280, 720, 3), Some((1258, 708)));
        assert!(matches!(
            app.live_authoring.canvas_region([10, 20, 100, 50]),
            crate::authoring_ui::PanelGesture::Handled { .. }
        ));
        app.set_mode(AppMode::LiveAuthoring);

        // 2 パス描画 (テクスチャ生成パス + 描画パス)。
        for _ in 0..2 {
            ctx.begin_pass(egui::RawInput::default());
            app.render_modebar(&mut child_ui(&ctx));
            app.render_body(&mut child_ui(&ctx));
            let _ = ctx.end_pass();
        }
    }

    /// thumbnail_scale: 長辺を max_edge へ縮め、小さい画像は等倍のまま。
    #[test]
    fn thumbnail_scale_fits_long_edge() {
        assert!((thumbnail_scale([320, 60], 160.0) - 0.5).abs() < 1e-6);
        assert!((thumbnail_scale([60, 320], 160.0) - 0.5).abs() < 1e-6);
        assert!((thumbnail_scale([100, 40], 160.0) - 1.0).abs() < 1e-6);
        assert!((thumbnail_scale([0, 0], 160.0) - 1.0).abs() < 1e-6);
    }
}
