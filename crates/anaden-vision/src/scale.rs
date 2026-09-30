//! 解像度正規化（720p 基準座標系）。TASK-009 の土台。
//!
//! MAA ControlScaleProxy（Wiki [[MAA-Resolution-Scaling]]）準拠:
//! **幅を基準(1280)にスケール**し、高さはアスペクト比で決まる。
//! これにより異なる解像度の端末（Pixel 7a 2400x1080 等）で同じ ROI/座標定義が使える。
//! テンプレート画像・ROI はすべてこの基準座標系で定義・保存する。

use crate::letterbox::CropInfo;
use image::{DynamicImage, imageops::FilterType};

/// 基準幅（MAA AsstTypes.h:28 WindowWidthDefault=1280 と同一）。
pub const BASE_WIDTH: u32 = 1280;
/// 基準高さ（MAA WindowHeightDefault=720）。横長端末では高さはアスペクト比で決まり 720 未満になりうる。
#[allow(dead_code)] // 文脈参照用。基準幅(1280)ベースのスケールで実質使用。
pub const BASE_HEIGHT: u32 = 720;

/// テンプレート(needle) のスケール補間方式。Lanczos3 は高品質で、1258→1280 等の
/// 微細な拡大でテンプレートマッチの conf 低下を防ぐ（review 指摘 Lanczos3 検討の具現化）。
pub const NEEDLE_FILTER: FilterType = FilterType::Lanczos3;

/// PC版実測クライアント幅(1258)。GetClientRect 実測値(capture_probe.png = 1258x708)。
///
/// PC版テンプレート/ROI はこの raw-1258x708 空間で定義されている。
/// 実行時キャプチャはサイズが異なりうるため、`roi_to_normalized` / `needle_to_normalized` で
/// raw-1258 空間の定義を実キャプチャ寸法へスケールする。
pub const PC_CLIENT_WIDTH_MEASURED: u32 = 1258;
/// PC版実測クライアント高さ(708)。GetClientRect 実測値(capture_probe.png = 1258x708)。
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

    /// 画像を基準幅(1280)へリサイズする（高さはアスペクト比を保存）。
    ///
    /// 元解像度が基準幅より小さくても常に1280基準へ統一する（PC版1258x708も1280へ拡大）。
    /// 黒帯クロップ後のキャプチャは描画領域(16:9)に揃うため、ここで一律1280幅へ正規化し、
    /// その後 raw-1258 空間のテンプレ/ROI を `roi_to_normalized`/`needle_to_normalized` で
    /// 1280空間へスケールしてマッチさせる。幅0の画像は複製を返す。
    pub fn normalize(&self, img: &DynamicImage) -> DynamicImage {
        let sw = img.width();
        if sw == 0 {
            return img.clone();
        }
        let s = self.scale_factor(sw);
        let new_h = ((img.height() as f32) * s).round().max(1.0) as u32;
        img.resize_exact(self.base_w, new_h, FilterType::Triangle)
    }

    /// 生キャプチャを本番認識フレームへ前処理する (Issue #212: 前処理の単一情報源)。
    ///
    /// `crop_to_canvas_with_info` (黒帯クロップ + キャンバス アスペクト保護) →
    /// [`Self::normalize`] (基準幅 1280) をこの順で 1 つの関数に束ねたもので、
    /// **engine live 経路 (`anaden_engine::PipelineDriver::run_once`) と
    /// `anaden-tool run-pipeline` 経路の双方がこれを呼ぶ前提で単一化**されている
    /// (両経路の前処理乖離禁止。Issue #212 の受け入れ基準)。
    ///
    /// 戻り値は `(認識フレーム, CropInfo)`。`CropInfo` は発火座標の逆変換
    /// (normalize後1280空間 → 黒帯込み生キャプチャ) に必要なコンテンツ領域位置。
    pub fn normalize_capture(&self, raw: &DynamicImage) -> (DynamicImage, CropInfo) {
        let (cropped, info) = crate::letterbox::crop_to_canvas_with_info(raw);
        (self.normalize(&cropped), info)
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
/// スケール（Lanczos3 補間）して返す。
///
/// `roi_to_normalized` と同じ X/Y 別比でリサイズする。キャプチャ寸法が 0 の場合は
/// 入力画像を複製して返す。
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
        // 2400x1080 (20:9) → 幅1280基準 → 高さ 576
        let out = scaler.normalize(&rgb(2400, 1080));
        assert_eq!(out.width(), 1280);
        assert_eq!(out.height(), 576);
    }

    #[test]
    fn small_image_normalized_to_base_width() {
        // normalize は常に1280幅基準へリサイズする（早期 return 廃止）。
        // 800x600 → 幅1280基準 → 高さ 960（アスペクト比 4:3 保存）。
        let scaler = ScreenScaler::new();
        let out = scaler.normalize(&rgb(800, 600));
        assert_eq!(out.width(), 1280);
        assert_eq!(out.height(), 960);
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

    // ---- T2 (Issue #5) 改訂: PC版(1258x708) の1280基準正規化 ----
    //
    // PC版キャプチャは GetClientRect 実測で 1258x708。かつては normalize が
    // 「1258 <= 1280 で RAW パススルー」していたが、黒帯入りキャプチャ(1918x1048等)対応のため
    // normalize は常に1280幅基準へリサイズするよう変更された（早期 return 廃止）。
    //
    // これにより PC版テンプレート/ROI は引き続き raw-1258x708 空間で定義されるが、
    // マッチ時には detect が `roi_to_normalized` / `needle_to_normalized` で
    // raw-1258 空間の定義を1280正規化空間へスケールする（コミット4で実装）。
    //
    // このテストは「1258x708 は1280基準へリサイズされる（パススルーしない）」ことを固定化する。
    //
    // 注意(定数昇格): PC_CLIENT_WIDTH_MEASURED / PC_CLIENT_HEIGHT_MEASURED はモジュール直下へ
    // pub 昇格済み。`use super::*` で本 mod から参照する。

    #[test]
    fn pc_capture_1258_normalized_to_1280_base() {
        let scaler = ScreenScaler::new();
        // PC版実測サイズ 1258x708 は常に1280幅基準へリサイズされる（早期 return 廃止）。
        // 高さ = 708 * (1280/1258) = 720.4 → 720（アスペクト比 1258:708 ≈ 16:9 を保存）。
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
        // normalize が常にリサイズへ変わったため、PC幅1258 の scale_factor は 1280/1258 > 1.0（拡大）。
        // これは normalize が1258→1280へ拡大リサイズすることを意味し、早期 return は廃止された。
        let scaler = ScreenScaler::new();
        let s = scaler.scale_factor(PC_CLIENT_WIDTH_MEASURED);
        // 1280/1258 = 1.017... > 1.0 → 拡大リサイズ。
        assert!(
            s > 1.0,
            "PC幅1258 の scale_factor は >1.0 (拡大)。normalize は常に1280基準へリサイズ"
        );
    }

    #[test]
    fn pc_roi_in_raw_space_fits_1258x708_bounds() {
        // templates/scenes/field/*.toml の [roi] テーブル(diary/map/template_01 等)は
        // raw-1258x708 空間で定義されている。代表例として diary の ROI を検証:
        //   diary.toml: x=337 y=604 width=89 height=94
        // ROI 右下端 = (337+89, 604+94) = (426, 698) <= (1258, 708) → 収まる。
        // 注意: y=604 は 20:9 正規化高さ(576)を超えており、これが raw-1258 空間の決定的証拠。
        // もしこれが 1280-base 正規化空間なら y+height=698 > 576 で画面外にはみ出す。
        //
        // normalize 変更（常時1280基準リサイズ）との整合: ROI 定義自体は raw-1258 空間のまま
        // 変更しない。normalize 後のキャプチャが 1280x720 になっても、detect が
        // roi_to_normalized(roi, 1280, 720) で raw-1258 → 1280 へスケールするため、
        // テンプレとROIの相対位置関係は保存される。このテストは ROI の raw-1258 性（定義空間）
        // を維持確認するもので、normalize 経路変更後も成立する。
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

    // ---- normalize_capture: 前処理の単一情報源 (Issue #212) ----
    //
    // 生キャプチャ → crop_to_canvas_with_info (アスペクト保護付き黒帯クロップ) →
    // normalize (1280 基準) の合成。engine live 経路と tool run-pipeline 経路が
    // 同一の前処理を使うことの単位。

    /// 左右黒帯つき 16:9 コンテンツ (真のピラーボックス) はクロップが受理され、
    /// normalize で 1280 幅 × 16:9 高さ (720 前後) になる。
    #[test]
    fn normalize_capture_pillarbox_yields_1280_16x9() {
        let scaler = ScreenScaler::new();
        // コンテンツ 320x180 (16:9) + 左右 20px 黒帯 → 全面 360x180。
        let mut img = image::RgbaImage::new(360, 180);
        for y in 0..180 {
            for x in 0..360 {
                let v = if (20..340).contains(&x) { 128u8 } else { 0u8 };
                img.put_pixel(x, y, image::Rgba([v, v, v, 255]));
            }
        }
        let raw = DynamicImage::ImageRgba8(img);
        let (frame, info) = scaler.normalize_capture(&raw);
        // クロップ受理 (左右 22px 除去 → 316x180) → 1280 x round(180*1280/316)=729。
        assert_eq!(info.offset_x, 22);
        assert_eq!((info.width, info.height), (316, 180));
        assert_eq!(frame.width(), 1280);
        let h = frame.height();
        assert!(
            (715..=735).contains(&h),
            "normalize 後高さは 16:9 前後 (720±15) になるべき: {h}"
        );
    }

    /// キャンバス内部の未描画黒 (下半分黒) はクロップ棄却 → 全面がそのまま 1280x720 に
    /// 正規化される (幾何保存)。Issue #212 実測 (1952x1098 → 誤 1280x378) の縮小再現。
    #[test]
    fn normalize_capture_unpainted_black_preserves_geometry() {
        let scaler = ScreenScaler::new();
        // 全面 320x180 (16:9)・下 90 行だけ黒 (未描画)。
        let mut img = image::RgbaImage::new(320, 180);
        for y in 0..180 {
            for x in 0..320 {
                let v = if y < 90 { 128u8 } else { 0u8 };
                img.put_pixel(x, y, image::Rgba([v, v, v, 255]));
            }
        }
        let raw = DynamicImage::ImageRgba8(img);
        let (frame, info) = scaler.normalize_capture(&raw);
        // guard がクロップを棄却 → CropInfo は全面 → 1280x720 (16:9 のまま)。
        assert_eq!(info, CropInfo::full(320, 180));
        assert_eq!((frame.width(), frame.height()), (1280, 720));
    }
}
