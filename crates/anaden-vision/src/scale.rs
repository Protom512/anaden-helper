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

/// PC版実測クライアント幅(1258)。GetClientRect 実測値(capture_probe.png = 1258x708)。
///
/// PC版テンプレート/ROI はこの raw-1258x708 空間で定義されている。
/// 実行時キャプチャはサイズが異なりうるため、`roi_to_normalized` / `needle_to_normalized` で
/// raw-1258 空間の定義を実キャプチャ寸法へスケールする。
#[allow(dead_code)] // pipeline detect (コミット4) で使用開始。段階的コミットのため一時許可。
pub const PC_CLIENT_WIDTH_MEASURED: u32 = 1258;
/// PC版実測クライアント高さ(708)。GetClientRect 実測値(capture_probe.png = 1258x708)。
#[allow(dead_code)] // pipeline detect (コミット4) で使用開始。段階的コミットのため一時許可。
pub const PC_CLIENT_HEIGHT_MEASURED: u32 = 708;

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

/// raw-1258x708 空間の ROI を、正規化後キャプチャ寸法 `(norm_w, norm_h)` へスケールする。
///
/// X/Y 別々の比（`sx = norm_w / 1258`, `sy = norm_h / 708`）でスケールする。
/// 既存の `ScreenScaler::to_base` は幅ベースの単一ファクタで縦横非分離のため、
/// アスペクト比が異なるキャプチャ（黒帯クロップ後等）には使えない。
///
/// `roi = [x, y, width, height]`。各要素を round で整数化して返す。
/// キャプチャ寸法が 0 の場合は入力 ROI をそのまま返す（ゼロ除算回避）。
#[allow(dead_code)] // pipeline detect (コミット4) で使用開始。段階的コミットのため一時許可。
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

/// raw-1258x708 空間のテンプレート画像を、正規化後キャプチャ寸法 `(norm_w, norm_h)` へ
/// スケール（Triangle 補間）して返す。
///
/// `roi_to_normalized` と同じ X/Y 別比でリサイズする。キャプチャ寸法が 0 の場合は
/// 入力画像を複製して返す。
#[allow(dead_code)] // pipeline detect (コミット4) で使用開始。段階的コミットのため一時許可。
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
    needle.resize_exact(nw, nh, FilterType::Triangle)
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
    //
    // 注意(定数昇格): PC_CLIENT_WIDTH_MEASURED / PC_CLIENT_HEIGHT_MEASURED はモジュール直下へ
    // pub 昇格済み。`use super::*` で本 mod から参照する。

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

    // ---- ファクタ API: roi_to_normalized / needle_to_normalized ----
    // raw-1258x708 空間の定義を実キャプチャ寸法へ X/Y 別比でスケールする。

    #[test]
    fn roi_to_normalized_1258_to_1280_scales_xy_separately() {
        // 1258x708 → 1280x720。sx = 1280/1258, sy = 720/708。
        let roi = [337, 604, 89, 94]; // diary ROI（raw-1258 空間）
        let out = roi_to_normalized(roi, 1280, 720);
        // x: 337 * 1280/1258 = 342.9 → 343
        assert_eq!(out[0], 343, "x は幅比でスケール");
        // y: 604 * 720/708 = 614.2 → 614
        assert_eq!(out[1], 614, "y は高さ比でスケール");
        // width: 89 * 1280/1258 = 90.6 → 91
        assert_eq!(out[2], 91);
        // height: 94 * 720/708 = 95.6 → 96
        assert_eq!(out[3], 96);
    }

    #[test]
    fn roi_to_normalized_1258_to_1920_scales_xy_separately() {
        // 1258x708 → 1920x1080。sx = 1920/1258, sy = 1080/708。
        let roi = [100, 200, 50, 60];
        let out = roi_to_normalized(roi, 1920, 1080);
        // x: 100 * 1920/1258 = 152.6 → 153
        assert_eq!(out[0], 153);
        // y: 200 * 1080/708 = 305.1 → 305
        assert_eq!(out[1], 305);
        // width: 50 * 1920/1258 = 76.3 → 76
        assert_eq!(out[2], 76);
        // height: 60 * 1080/708 = 91.5 → 92
        assert_eq!(out[3], 92);
    }

    #[test]
    fn roi_to_normalized_identity_when_norm_equals_measured() {
        // norm == 1258x708 なら恒等（sx=sy=1.0）。
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
        // 89x94 のテンプレを 1280x720 空間へ。X/Y 別比。
        let needle = rgb(89, 94);
        let out = needle_to_normalized(&needle, 1280, 720);
        // 89 * 1280/1258 = 90.6 → 91, 94 * 720/708 = 95.6 → 96
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
}
