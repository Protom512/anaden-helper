use eframe::egui;

use anaden_core::ScreenRegion;

#[derive(Clone, Copy, Debug, Default)]
pub struct RoiEdit {
    pub anchor: Option<(u32, u32)>,
    pub current: Option<(u32, u32)>,
    pub dragging: bool,
}

impl RoiEdit {
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

pub struct HeatmapView {
    pub tex: egui::TextureId,
    pub search: ScreenRegion,
}

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
    let mut display = avail;
    if display.x / display.y > aspect {
        display.x = display.y * aspect;
    } else {
        display.y = display.x / aspect;
    }

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

fn img_to_screen(rect: egui::Rect, iw: u32, ih: u32, p: (u32, u32)) -> egui::Pos2 {
    egui::pos2(
        rect.left() + (p.0 as f32 / iw as f32) * rect.width(),
        rect.top() + (p.1 as f32 / ih as f32) * rect.height(),
    )
}

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
            let a = (t * t * 170.0) as u8;
            pixels.extend_from_slice(&[r, g, b, a]);
        }
    }
    egui::ColorImage::from_rgba_unmultiplied([w, h], &pixels)
}
