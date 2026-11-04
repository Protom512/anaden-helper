use crate::letterbox::CropInfo;
use image::{DynamicImage, imageops::FilterType};

pub const BASE_WIDTH: u32 = 1280;
#[allow(dead_code)]
pub const BASE_HEIGHT: u32 = 720;

pub const NEEDLE_FILTER: FilterType = FilterType::Lanczos3;

pub const PC_CLIENT_WIDTH_MEASURED: u32 = 1258;
pub const PC_CLIENT_HEIGHT_MEASURED: u32 = 708;

#[derive(Debug, Clone, Copy)]
pub struct ScreenScaler {
    base_w: u32,
}

impl Default for ScreenScaler {
    fn default() -> Self {
        Self::new()
    }
}

impl ScreenScaler {
    pub fn new() -> Self {
        Self { base_w: BASE_WIDTH }
    }

    pub fn scale_factor(&self, src_width: u32) -> f32 {
        if src_width == 0 {
            1.0
        } else {
            self.base_w as f32 / src_width as f32
        }
    }

    pub fn normalize(&self, img: &DynamicImage) -> DynamicImage {
        let sw = img.width();
        if sw == 0 {
            return img.clone();
        }
        let s = self.scale_factor(sw);
        let new_h = ((img.height() as f32) * s).round().max(1.0) as u32;
        img.resize_exact(self.base_w, new_h, FilterType::Triangle)
    }

    pub fn normalize_capture(&self, raw: &DynamicImage) -> (DynamicImage, CropInfo) {
        let (cropped, info) = crate::letterbox::crop_to_canvas_with_info(raw);
        (self.normalize(&cropped), info)
    }

    pub fn to_base(&self, src_width: u32, v: u32) -> u32 {
        ((v as f32) * self.scale_factor(src_width)).round() as u32
    }

    pub fn from_base(&self, src_width: u32, v: u32) -> u32 {
        let s = self.scale_factor(src_width);
        if s == 0.0 {
            v
        } else {
            ((v as f32) / s).round() as u32
        }
    }
}

pub fn roi_to_normalized(roi: [u32; 4], norm_w: u32, norm_h: u32) -> [u32; 4] {
    if norm_w == 0 || norm_h == 0 {
        return roi;
    }
    let sx = norm_w as f32 / PC_CLIENT_WIDTH_MEASURED as f32;
    let sy = norm_h as f32 / PC_CLIENT_HEIGHT_MEASURED as f32;
    let scale_w = |v: u32| ((v as f32) * sx).round() as u32;
    let scale_h = |v: u32| ((v as f32) * sy).round() as u32;
    [
        scale_w(roi[0]),
        scale_h(roi[1]),
        scale_w(roi[2]),
        scale_h(roi[3]),
    ]
}

pub fn needle_to_normalized(needle: &DynamicImage, norm_w: u32, norm_h: u32) -> DynamicImage {
    if norm_w == 0 || norm_h == 0 {
        return needle.clone();
    }
    let nw = ((needle.width() as f32) * (norm_w as f32 / PC_CLIENT_WIDTH_MEASURED as f32)).round()
        as u32;
    let nh = ((needle.height() as f32) * (norm_h as f32 / PC_CLIENT_HEIGHT_MEASURED as f32)).round()
        as u32;
    if nw == 0 || nh == 0 {
        return needle.clone();
    }
    needle.resize_exact(nw, nh, NEEDLE_FILTER)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, RgbImage};

    fn rgb(w: u32, h: u32) -> DynamicImage {
        DynamicImage::ImageRgb8(RgbImage::new(w, h))
    }

    #[test]
    fn normalize_landscape_pixel7a_to_base_width() {
        let scaler = ScreenScaler::new();
        let out = scaler.normalize(&rgb(2400, 1080));
        assert_eq!(out.width(), 1280);
        assert_eq!(out.height(), 576);
    }

    #[test]
    fn small_image_normalized_to_base_width() {
        let scaler = ScreenScaler::new();
        let out = scaler.normalize(&rgb(800, 600));
        assert_eq!(out.width(), 1280);
        assert_eq!(out.height(), 960);
    }

    #[test]
    fn coordinate_roundtrip_pixel7a() {
        let scaler = ScreenScaler::new();
        let src_w = 2400u32;
        assert_eq!(scaler.to_base(src_w, 1200), 640);
        assert_eq!(scaler.from_base(src_w, 640), 1200);
    }

    #[test]
    fn scale_factor_pixel7a() {
        let scaler = ScreenScaler::new();
        let s = scaler.scale_factor(2400);
        assert!((s - (1280.0 / 2400.0)).abs() < 1e-6);
    }

    #[test]
    fn pc_capture_1258_normalized_to_1280_base() {
        let scaler = ScreenScaler::new();
        let out = scaler.normalize(&rgb(PC_CLIENT_WIDTH_MEASURED, PC_CLIENT_HEIGHT_MEASURED));
        assert_eq!(
            (out.width(), out.height()),
            (1280, 720),
            "PC版キャプチャ(1258x708) は normalize で 1280x720 へリサイズされる。\
             raw-1258 空間のテンプレ/ROI は detect で roi_to_normalized/needle_to_normalized \
             により1280空間へスケールされる"
        );
    }

    #[test]
    fn pc_capture_scale_factor_is_upscale() {
        let scaler = ScreenScaler::new();
        let s = scaler.scale_factor(PC_CLIENT_WIDTH_MEASURED);
        assert!(
            s > 1.0,
            "PC幅1258 の scale_factor は >1.0 (拡大)。normalize は常に1280基準へリサイズ"
        );
    }

    #[test]
    fn pc_roi_in_raw_space_fits_1258x708_bounds() {
        let (x, y, w, h): (u32, u32, u32, u32) = (337, 604, 89, 94);
        let right = x + w;
        let bottom = y + h;
        assert!(
            right <= PC_CLIENT_WIDTH_MEASURED,
            "diary ROI right={right} <= 1258"
        );
        assert!(
            bottom <= PC_CLIENT_HEIGHT_MEASURED,
            "diary ROI bottom={bottom} <= 708"
        );
        assert!(
            bottom > 576,
            "diary ROI bottom={bottom} > 576(20:9正規化高さ) → raw-1258空間でなければ画面外"
        );
    }

    #[test]
    fn roi_to_normalized_1258_to_1280_scales_xy_separately() {
        let roi = [337, 604, 89, 94];
        let out = roi_to_normalized(roi, 1280, 720);
        assert_eq!(out[0], 343, "x は幅比でスケール");
        assert_eq!(out[1], 614, "y は高さ比でスケール");
        assert_eq!(out[2], 91);
        assert_eq!(out[3], 96);
    }

    #[test]
    fn roi_to_normalized_1258_to_1920_scales_xy_separately() {
        let roi = [100, 200, 50, 60];
        let out = roi_to_normalized(roi, 1920, 1080);
        assert_eq!(out[0], 153);
        assert_eq!(out[1], 305);
        assert_eq!(out[2], 76);
        assert_eq!(out[3], 92);
    }

    #[test]
    fn roi_to_normalized_identity_when_norm_equals_measured() {
        let roi = [337, 604, 89, 94];
        let out = roi_to_normalized(roi, PC_CLIENT_WIDTH_MEASURED, PC_CLIENT_HEIGHT_MEASURED);
        assert_eq!(out, roi);
    }

    #[test]
    fn roi_to_normalized_zero_norm_returns_input_unchanged() {
        let roi = [337, 604, 89, 94];
        assert_eq!(
            roi_to_normalized(roi, 0, 720),
            roi,
            "norm_w=0 は入力そのまま"
        );
        assert_eq!(
            roi_to_normalized(roi, 1280, 0),
            roi,
            "norm_h=0 は入力そのまま"
        );
    }

    #[test]
    fn needle_to_normalized_1258_to_1280_resizes() {
        let needle = rgb(89, 94);
        let out = needle_to_normalized(&needle, 1280, 720);
        assert_eq!((out.width(), out.height()), (91, 96));
    }

    #[test]
    fn needle_to_normalized_identity_when_norm_equals_measured() {
        let needle = rgb(89, 94);
        let out =
            needle_to_normalized(&needle, PC_CLIENT_WIDTH_MEASURED, PC_CLIENT_HEIGHT_MEASURED);
        assert_eq!((out.width(), out.height()), (89, 94));
    }

    #[test]
    fn needle_to_normalized_zero_norm_returns_clone() {
        let needle = rgb(89, 94);
        let out_w = needle_to_normalized(&needle, 0, 720);
        assert_eq!((out_w.width(), out_w.height()), (89, 94));
        let out_h = needle_to_normalized(&needle, 1280, 0);
        assert_eq!((out_h.width(), out_h.height()), (89, 94));
    }

    #[test]
    fn normalize_capture_pillarbox_yields_1280_16x9() {
        let scaler = ScreenScaler::new();
        let mut img = image::RgbaImage::new(360, 180);
        for y in 0..180 {
            for x in 0..360 {
                let v = if (20..340).contains(&x) { 128u8 } else { 0u8 };
                img.put_pixel(x, y, image::Rgba([v, v, v, 255]));
            }
        }
        let raw = DynamicImage::ImageRgba8(img);
        let (frame, info) = scaler.normalize_capture(&raw);
        assert_eq!(info.offset_x, 22);
        assert_eq!((info.width, info.height), (316, 180));
        assert_eq!(frame.width(), 1280);
        let h = frame.height();
        assert!(
            (715..=735).contains(&h),
            "normalize 後高さは 16:9 前後 (720±15) になるべき: {h}"
        );
    }

    #[test]
    fn normalize_capture_unpainted_black_preserves_geometry() {
        let scaler = ScreenScaler::new();
        let mut img = image::RgbaImage::new(320, 180);
        for y in 0..180 {
            for x in 0..320 {
                let v = if y < 90 { 128u8 } else { 0u8 };
                img.put_pixel(x, y, image::Rgba([v, v, v, 255]));
            }
        }
        let raw = DynamicImage::ImageRgba8(img);
        let (frame, info) = scaler.normalize_capture(&raw);
        assert_eq!(info, CropInfo::full(320, 180));
        assert_eq!((frame.width(), frame.height()), (1280, 720));
    }
}
