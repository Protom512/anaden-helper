//! 解像度正規化（720p 基準座標系）。TASK-009 の土台。
//!
//! MAA ControlScaleProxy（Wiki [[MAA-Resolution-Scaling]]）準拠:
//! **幅を基準(1280)にスケール**し、高さはアスペクト比で決まる。
//! これにより異なる解像度の端末（Pixel 7a 2400x1080 等）で同じ ROI/座標定義が使える。
//! テンプレート画像・ROI はすべてこの基準座標系で定義・保存する。

use image::{DynamicImage, imageops::FilterType};

/// 基準幅（MAA AsstTypes.h:28 WindowWidthDefault=1280 と同一）。
pub const BASE_WIDTH: u32 = 1280;
/// 基準高さ（MAA WindowHeightDefault=720）。横長端末では高さはアスペクト比で決まり 720 未満になりうる。
#[allow(dead_code)] // 文脈参照用。基準幅(1280)ベースのスケールで実質使用。
pub const BASE_HEIGHT: u32 = 720;

/// 画面を基準座標系へ正規化するスケーラ。
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
    /// 720p 基準(幅1280)のスケーラを作成する。
    pub fn new() -> Self {
        Self { base_w: BASE_WIDTH }
    }

    /// 基準幅に対するスケール倍率（元解像度 → 基準）。
    pub fn scale_factor(&self, src_width: u32) -> f32 {
        if src_width == 0 {
            1.0
        } else {
            self.base_w as f32 / src_width as f32
        }
    }

    /// 画像を基準幅に縮小する（高さはアスペクト比を保存）。
    /// 既に基準幅以下なら複製を返す（拡大はしない）。
    pub fn normalize(&self, img: &DynamicImage) -> DynamicImage {
        let sw = img.width();
        if sw <= self.base_w {
            return img.clone();
        }
        let s = self.scale_factor(sw);
        let new_h = ((img.height() as f32) * s).round().max(1.0) as u32;
        img.resize_exact(self.base_w, new_h, FilterType::Triangle)
    }

    /// 元画像座標 → 基準座標。
    pub fn to_base(&self, src_width: u32, v: u32) -> u32 {
        ((v as f32) * self.scale_factor(src_width)).round() as u32
    }

    /// 基準座標 → 元画像座標。
    pub fn from_base(&self, src_width: u32, v: u32) -> u32 {
        let s = self.scale_factor(src_width);
        if s == 0.0 {
            v
        } else {
            ((v as f32) / s).round() as u32
        }
    }
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
        // 2400x1080 (20:9) → 幅1280基準 → 高さ 576
        let out = scaler.normalize(&rgb(2400, 1080));
        assert_eq!(out.width(), 1280);
        assert_eq!(out.height(), 576);
    }

    #[test]
    fn already_small_image_not_upscaled() {
        let scaler = ScreenScaler::new();
        let out = scaler.normalize(&rgb(800, 600));
        assert_eq!((out.width(), out.height()), (800, 600));
    }

    #[test]
    fn coordinate_roundtrip_pixel7a() {
        let scaler = ScreenScaler::new();
        let src_w = 2400u32;
        // 元 1200px → 基準 640px
        assert_eq!(scaler.to_base(src_w, 1200), 640);
        // 基準 640px → 元 1200px
        assert_eq!(scaler.from_base(src_w, 640), 1200);
    }

    #[test]
    fn scale_factor_pixel7a() {
        let scaler = ScreenScaler::new();
        let s = scaler.scale_factor(2400);
        assert!((s - (1280.0 / 2400.0)).abs() < 1e-6);
    }
}
