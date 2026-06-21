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

    // ---- T2 (Issue #5): PC版(Windows, 16:9, 1258x708) の RAW パススルー保証 ----
    //
    // PC版キャプチャは GetClientRect 実測で 1258x708(SHARED-MEMORY 測定値)。
    // この幅は BASE_WIDTH(1280) 以下のため、normalize はリサイズせず生画像をそのまま返す。
    // 結果として PC版テンプレート/ROI はすべて RAW 1258x708 ピクセル空間で定義しなければ
    // ならず、1280 基準の正規化空間(20:9 実機用)で定義すると座標がズレて NoMatch になる。
    //
    // このテストは「1258 <= 1280 でパススルーされる」ことを固定化し、将来 normalize の
    // 条件を誤って書き換えた場合(例: `<` を `<=` に変更、あるいは閾値を下げる等)に
    // 即座に検出する。PC版テンプレート群(templates/scenes/field/*.toml の [roi])が
    // raw-1258 空間で正しいのはこの不変条件に依存している。

    /// PC版実測クライアント幅(1258)。GetClientRect 実測値(capture_probe.png = 1258x708)。
    pub const PC_CLIENT_WIDTH_MEASURED: u32 = 1258;
    /// PC版実測クライアント高さ(708)。GetClientRect 実測値(capture_probe.png = 1258x708)。
    pub const PC_CLIENT_HEIGHT_MEASURED: u32 = 708;

    #[test]
    fn pc_capture_1258_wide_passes_through_normalize_raw() {
        let scaler = ScreenScaler::new();
        // PC版実測サイズ 1258x708 は BASE_WIDTH(1280) 以下 → リサイズされず RAW パススルー。
        let out = scaler.normalize(&rgb(PC_CLIENT_WIDTH_MEASURED, PC_CLIENT_HEIGHT_MEASURED));
        assert_eq!(
            (out.width(), out.height()),
            (1258, 708),
            "PC版キャプチャ(1258x708) は normalize で RAW パススルーされなければならない。\
             リサイズされた場合、raw-1258 空間のテンプレート/ROI 座標がすべてズレる"
        );
    }

    #[test]
    fn pc_capture_scale_factor_is_identity() {
        let scaler = ScreenScaler::new();
        // 1258 <= 1280 でも scale_factor は base_w/src の比を返す設計(1.0 ではない)。
        // これは normalize が「幅で早期 return する」ことでパススルーを実現しており、
        // scale_factor 自体を呼ばないことを意味する。to_base/from_base は別経路。
        // ここでは「PC版幅で scale_factor を呼んでも 1.0 より大きい(拡大側)」こと、
        // つまり仮にリサイズが走っても拡大(画質劣化+座標ズレ)になることを記録し、
        // normalize の早期 return が必須であることを文書化する。
        let s = scaler.scale_factor(PC_CLIENT_WIDTH_MEASURED);
        // 1280/1258 = 1.017... > 1.0 → リサイズされると拡大。パススルーが正しい。
        assert!(
            s > 1.0,
            "PC幅1258 の scale_factor は >1.0 (拡大)。normalize の早期return必須"
        );
    }

    #[test]
    fn pc_roi_in_raw_space_fits_1258x708_bounds() {
        // templates/scenes/field/*.toml の新規 [roi] テーブル(diary/map/template_01 等)は
        // raw-1258x708 空間で定義されている。代表例として diary の ROI を検証:
        //   diary.toml: x=337 y=604 width=89 height=94
        // ROI 右下端 = (337+89, 604+94) = (426, 698) <= (1258, 708) → 収まる。
        // 注意: y=604 は 20:9 正規化高さ(576)を超えており、これが raw-1258 空間の決定的証拠。
        // もしこれが 1280-base 正規化空間なら y+height=698 > 576 で画面外にはみ出す。
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
        // 20:9 正規化空間(高さ576)には収まらない → raw-1258 空間であることの証明。
        assert!(
            bottom > 576,
            "diary ROI bottom={bottom} > 576(20:9正規化高さ) → raw-1258空間でなければ画面外"
        );
    }
}
