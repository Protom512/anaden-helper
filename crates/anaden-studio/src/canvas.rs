//! 画像表示 + ドラッグROI選択 + ヒートマップオーバーレイウィジェット。
//!
//! スクリーンショット上でマウスドラッグすると矩形ROIを選択できる。
//! 座標は egui 表示座標 ↔ 元画像ピクセル座標 を相互変換する。
//! ヒートマップは score_map（テンプレートが画面のどこにマッチするか）を可視化する。

use eframe::egui;

use anaden_core::ScreenRegion;

/// ドラッグ中のROI編集状態（元画像ピクセル座標）。
#[derive(Clone, Copy, Debug, Default)]
pub struct RoiEdit {
    /// ドラッグ開始点。
    pub anchor: Option<(u32, u32)>,
    /// 現在のポインタ位置。
    pub current: Option<(u32, u32)>,
    /// 今フレームでドラッグ中か。
    pub dragging: bool,
}

impl RoiEdit {
    /// 確定したROI矩形。両端が揃っていて面積が正の場合のみ。
    pub fn rect(&self) -> Option<ScreenRegion> {
        let (a, c) = (self.anchor?, self.current?);
        let x0 = a.0.min(c.0);
        let y0 = a.1.min(c.1);
        let x1 = a.0.max(c.0);
        let y1 = a.1.max(c.1);
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        Some(ScreenRegion::new(x0, y0, x1 - x0, y1 - y0))
    }
}

/// ヒートマップオーバーレイの描画データ。
pub struct HeatmapView {
    /// 着色済みスコアマップのテクスチャID。
    pub tex: egui::TextureId,
    /// スコアマップが対応する探索領域（元画像座標）。
    pub search: ScreenRegion,
}

/// 画像を表示し、ドラッグでROIを選択する。ヒートマップと最良マッチ位置も重ね描き。
pub fn show(
    ui: &mut egui::Ui,
    tex: &egui::TextureHandle,
    img_w: u32,
    img_h: u32,
    roi: &mut RoiEdit,
    heatmap: Option<&HeatmapView>,
    best_match: Option<ScreenRegion>,
) {
    let avail = ui.available_size();
    let aspect = img_w as f32 / img_h as f32;
    // アスペクト比を保って available size に収める
    let mut display = avail;
    if display.x / display.y > aspect {
        display.x = display.y * aspect;
    } else {
        display.y = display.x / aspect;
    }

    // Image widget で画像を描画（uv=[0,0]..[1,1] の安全な経路）。
    // fit_to_exact_size(display) + maintain_aspect_ratio(false) で、
    // 既存手計算の display 矩形に一致させ、オーバーレイ座標(img_to_screen)とピクセル単位で合わせる。
    // sense(drag) で ROIドラッグの response を得る。
    let img_widget = egui::Image::from_texture(egui::load::SizedTexture::new(
        tex.id(),
        [img_w as f32, img_h as f32],
    ))
    .fit_to_exact_size(display)
    .maintain_aspect_ratio(false)
    .tint(egui::Color32::WHITE)
    .sense(egui::Sense::drag());
    let response = ui.add(img_widget);
    let rect = response.rect;
    let painter = ui.painter_at(rect);

    roi.dragging = response.dragged();

    // egui 表示座標 → 元画像ピクセル座標
    let to_img = |p: egui::Pos2| -> Option<(u32, u32)> {
        if !rect.contains(p) {
            return None;
        }
        let fx = ((p.x - rect.left()) / rect.width()).clamp(0.0, 1.0) * img_w as f32;
        let fy = ((p.y - rect.top()) / rect.height()).clamp(0.0, 1.0) * img_h as f32;
        Some((
            fx.min(img_w as f32 - 1.0) as u32,
            fy.min(img_h as f32 - 1.0) as u32,
        ))
    };

    if response.drag_started() {
        roi.anchor = response.interact_pointer_pos().and_then(to_img);
        roi.current = roi.anchor;
    }
    if response.dragged()
        && let Some(pos) = response.interact_pointer_pos().and_then(to_img)
    {
        roi.current = Some(pos);
    }

    // ヒートマップオーバーレイ（探索領域に合わせて引き伸ばし）
    // uv は [0,0]..[1,1] を明示的に渡す（Rect::EVERYTHING は絶対NG）。
    if let Some(hm) = heatmap {
        let tl = img_to_screen(rect, img_w, img_h, (hm.search.x, hm.search.y));
        let br = img_to_screen(rect, img_w, img_h, (hm.search.right(), hm.search.bottom()));
        painter.image(
            hm.tex,
            egui::Rect::from_two_pos(tl, br),
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
    }

    // 最良マッチ位置をシアンの枠でマーク
    if let Some(bm) = best_match {
        let tl = img_to_screen(rect, img_w, img_h, (bm.x, bm.y));
        let br = img_to_screen(rect, img_w, img_h, (bm.right(), bm.bottom()));
        painter.rect_stroke(
            egui::Rect::from_two_pos(tl, br),
            0.0,
            egui::Stroke::new(2.0, egui::Color32::CYAN),
            egui::StrokeKind::Outside,
        );
    }

    // 選択矩形を描画（黄）
    if let Some(r) = roi.rect() {
        let tl = img_to_screen(rect, img_w, img_h, (r.x, r.y));
        let br = img_to_screen(rect, img_w, img_h, (r.right(), r.bottom()));
        painter.rect_stroke(
            egui::Rect::from_two_pos(tl, br),
            0.0,
            egui::Stroke::new(2.0, egui::Color32::YELLOW),
            egui::StrokeKind::Outside,
        );
    }
}

/// 元画像ピクセル座標 → egui 表示座標。
fn img_to_screen(rect: egui::Rect, iw: u32, ih: u32, p: (u32, u32)) -> egui::Pos2 {
    egui::pos2(
        rect.left() + (p.0 as f32 / iw as f32) * rect.width(),
        rect.top() + (p.1 as f32 / ih as f32) * rect.height(),
    )
}

/// スコアマップ(GrayImage, 0-255) をヒートマップ(ColorImage RGBA)に変換する。
///
/// 値が高いほど白く不透明（赤→橙→黄→白）、低いほど透明。
pub fn score_map_to_heatmap(map: &image::GrayImage) -> egui::ColorImage {
    let (w, h) = (map.width() as usize, map.height() as usize);
    let mut pixels = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        for x in 0..w {
            let v = map.get_pixel(x as u32, y as u32)[0];
            let t = v as f32 / 255.0;
            let r = 255u8;
            let g = ((t - 0.3).max(0.0) / 0.7 * 255.0).min(255.0) as u8;
            let b = ((t - 0.6).max(0.0) / 0.4 * 255.0).min(255.0) as u8;
            // 高スコアほど不透明。低スコアは背景が透けて見える。
            let a = (t * t * 170.0) as u8;
            pixels.extend_from_slice(&[r, g, b, a]);
        }
    }
    egui::ColorImage::from_rgba_unmultiplied([w, h], &pixels)
}
