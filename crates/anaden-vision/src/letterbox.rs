use image::{DynamicImage, GenericImageView, ImageBuffer, Rgb};

pub const BLACK_BAR_LUMINANCE_THRESHOLD: f64 = 8.0;

pub const MARGIN_PX: u32 = 2;

pub const MIN_CONTENT_PX: u32 = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CropInfo {
    pub offset_x: u32,
    pub offset_y: u32,
    pub width: u32,
    pub height: u32,
}

impl CropInfo {
    pub fn full(w: u32, h: u32) -> Self {
        Self {
            offset_x: 0,
            offset_y: 0,
            width: w,
            height: h,
        }
    }
}

pub fn crop_to_content(img: &DynamicImage) -> DynamicImage {
    crop_to_content_with_info(img).0
}

pub fn crop_to_content_with_info(img: &DynamicImage) -> (DynamicImage, CropInfo) {
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return (img.clone(), CropInfo::default());
    }

    let rgb = img.to_rgb8();

    let row_means = axis_mean_luminance(&rgb, Axis::Row, 0, w);
    let top = leading_black_count(&row_means, BLACK_BAR_LUMINANCE_THRESHOLD);
    let bottom = trailing_black_count(&row_means, BLACK_BAR_LUMINANCE_THRESHOLD);

    let row_lo = top;
    let row_hi = h.saturating_sub(bottom);
    let col_means = axis_mean_luminance(&rgb, Axis::Col, row_lo, row_hi);
    let left = leading_black_count(&col_means, BLACK_BAR_LUMINANCE_THRESHOLD);
    let right = trailing_black_count(&col_means, BLACK_BAR_LUMINANCE_THRESHOLD);

    let crop_top = margin_offset(top, h);
    let crop_bottom = margin_offset(bottom, h);
    let crop_left = margin_offset(left, w);
    let crop_right = margin_offset(right, w);

    let remaining_w = w.saturating_sub(crop_left + crop_right);
    let remaining_h = h.saturating_sub(crop_top + crop_bottom);
    if remaining_w < MIN_CONTENT_PX || remaining_h < MIN_CONTENT_PX {
        return (img.clone(), CropInfo::full(w, h));
    }

    if crop_top == 0 && crop_bottom == 0 && crop_left == 0 && crop_right == 0 {
        return (img.clone(), CropInfo::full(w, h));
    }

    let x = crop_left;
    let y = crop_top;
    let cw = remaining_w;
    let ch = remaining_h;

    let cropped: ImageBuffer<Rgb<u8>, Vec<u8>> =
        ImageBuffer::from_fn(cw, ch, |px, py| *rgb.get_pixel(x + px, y + py));
    let info = CropInfo {
        offset_x: x,
        offset_y: y,
        width: cw,
        height: ch,
    };
    (DynamicImage::ImageRgb8(cropped), info)
}

#[derive(Clone, Copy)]
enum Axis {
    Row,
    Col,
}

fn axis_mean_luminance(
    rgb: &ImageBuffer<Rgb<u8>, Vec<u8>>,
    axis: Axis,
    lo: u32,
    hi: u32,
) -> Vec<f64> {
    let (w, h) = rgb.dimensions();
    let outer = match axis {
        Axis::Row => h,
        Axis::Col => w,
    };
    let span = (hi.saturating_sub(lo)) as f64;
    if span <= 0.0 {
        return vec![0.0_f64; outer as usize];
    }
    let luminance = |x: u32, y: u32| -> f64 {
        let p = rgb.get_pixel(x, y);
        0.299 * p[0] as f64 + 0.587 * p[1] as f64 + 0.114 * p[2] as f64
    };
    (0..outer)
        .map(|o| {
            let sum: f64 = (lo..hi)
                .map(|i| match axis {
                    Axis::Row => luminance(i, o),
                    Axis::Col => luminance(o, i),
                })
                .sum();
            sum / span
        })
        .collect()
}

fn leading_black_count(means: &[f64], threshold: f64) -> u32 {
    let mut count = 0u32;
    for &m in means {
        if m < threshold {
            count += 1;
        } else {
            break;
        }
    }
    count
}

fn trailing_black_count(means: &[f64], threshold: f64) -> u32 {
    let mut count = 0u32;
    for &m in means.iter().rev() {
        if m < threshold {
            count += 1;
        } else {
            break;
        }
    }
    count
}

fn margin_offset(bar: u32, max: u32) -> u32 {
    if bar == 0 {
        0
    } else {
        bar.saturating_add(MARGIN_PX).min(max)
    }
}

pub const CANVAS_ASPECT: f64 = 16.0 / 9.0;

pub const CANVAS_ASPECT_TOLERANCE: f64 = 0.05;

pub fn crop_to_canvas_with_info(img: &DynamicImage) -> (DynamicImage, CropInfo) {
    let (w, h) = img.dimensions();
    let (cropped, info) = crop_to_content_with_info(img);
    if info.offset_x == 0 && info.offset_y == 0 && info.width == w && info.height == h {
        return (cropped, info);
    }
    if info.height == 0 {
        return (img.clone(), CropInfo::full(w, h));
    }
    let aspect = f64::from(info.width) / f64::from(info.height);
    let deviation = ((aspect - CANVAS_ASPECT) / CANVAS_ASPECT).abs();
    if deviation <= CANVAS_ASPECT_TOLERANCE {
        (cropped, info)
    } else {
        (img.clone(), CropInfo::full(w, h))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use image::{DynamicImage, ImageBuffer, Rgb, Rgba, RgbaImage};

    /// `w x h` のベース画像の外周に `bar` ピクセルの黒帯を追加した RGBA 画像を作る。
    /// 中央は輝度の高い（黒帯判定されない）灰色で埋める。
    fn synthetic_with_bars(w: u32, h: u32, bar: u32) -> DynamicImage {
        let full_w = w + 2 * bar;
        let full_h = h + 2 * bar;
        let mut img: RgbaImage = ImageBuffer::new(full_w, full_h);
        for y in 0..full_h {
            for x in 0..full_w {
                let in_content = x >= bar && x < bar + w && y >= bar && y < bar + h;
                let v = if in_content { 128u8 } else { 0u8 };
                img.put_pixel(x, y, Rgba([v, v, v, 255]));
            }
        }
        DynamicImage::ImageRgba8(img)
    }

    #[test]
    fn crop_removes_top_bottom_bars() {
        // 中央 200x112(16:9) ＋ 上下左右 15px 黒帯 → 230x142。
        // コンテンツ残り寸法が MIN_CONTENT_PX(64) を十分に超えるよう十分大きな画像。
        let img = synthetic_with_bars(200, 112, 15);
        assert_eq!(img.dimensions(), (230, 142));

        let out = crop_to_content(&img);
        // 上下左右それぞれ 15px 黒帯 ＋ MARGIN_PX(2) 内側寄せ。
        // remaining = 230 - 2*(15+2) = 196, 142 - 2*(15+2) = 108
        assert_eq!(out.dimensions(), (196, 108));
    }

    #[test]
    fn crop_removes_only_horizontal_bars_letterbox() {
        // レターボックス: 上下のみ黒帯。中央 160x90(16:9) ＋ 上下 20px。
        let mut img: RgbaImage = ImageBuffer::new(160, 130);
        for y in 0..130 {
            for x in 0..160 {
                let in_content = (20..110).contains(&y);
                let v = if in_content { 100u8 } else { 0u8 };
                img.put_pixel(x, y, Rgba([v, v, v, 255]));
            }
        }
        let dyn_img = DynamicImage::ImageRgba8(img);
        let out = crop_to_content(&dyn_img);
        // 左右は黒帯なし → crop_left/right = 0。上下 20 + MARGIN 2 = 22 ずつ。
        assert_eq!(out.width(), 160, "左右黒帯なしなので幅は維持");
        assert_eq!(out.height(), 130 - 2 * (20 + 2));
    }

    #[test]
    fn crop_removes_only_vertical_bars_pillarbox() {
        // ピラーボックス: 左右のみ黒帯。中央 100x75(4:3) ＋ 左右 15px。
        let mut img: RgbaImage = ImageBuffer::new(130, 75);
        for y in 0..75 {
            for x in 0..130 {
                let in_content = (15..115).contains(&x);
                let v = if in_content { 110u8 } else { 0u8 };
                img.put_pixel(x, y, Rgba([v, v, v, 255]));
            }
        }
        let dyn_img = DynamicImage::ImageRgba8(img);
        let out = crop_to_content(&dyn_img);
        assert_eq!(out.width(), 130 - 2 * (15 + 2));
        assert_eq!(out.height(), 75, "上下黒帯なしなので高さは維持");
    }

    #[test]
    fn crop_no_bars_returns_clone() {
        // 黒帯なし全体画像。クロップされず寸法維持。
        let img =
            DynamicImage::ImageRgb8(ImageBuffer::from_fn(200, 112, |_, _| Rgb([64, 96, 128])));
        let out = crop_to_content(&img);
        assert_eq!(out.dimensions(), (200, 112));
    }

    #[test]
    fn crop_zero_size_returns_clone() {
        let img = DynamicImage::ImageRgb8(ImageBuffer::new(0, 0));
        let out = crop_to_content(&img);
        assert_eq!(out.dimensions(), (0, 0));
    }

    #[test]
    fn crop_all_black_falls_back_to_clone() {
        // 全面黒画像。クロップ残りが MIN_CONTENT_PX を下回る → フォールバック。
        let img = DynamicImage::ImageRgb8(ImageBuffer::from_fn(100, 100, |_, _| Rgb([0, 0, 0])));
        let out = crop_to_content(&img);
        assert_eq!(
            out.dimensions(),
            (100, 100),
            "全面黒はフォールバックで元寸法"
        );
    }

    #[test]
    fn crop_dark_content_interior_not_mistaken_for_bars() {
        // 端は明るく、中央付近に暗部がある画像。端が黒くないので黒帯検出されない。
        let mut img: RgbaImage = ImageBuffer::new(80, 80);
        for y in 0..80 {
            for x in 0..80 {
                // 端 5px は明るい、中央 30..50 は暗い（連続黒ではない）。
                let edge = !(5..75).contains(&x) || !(5..75).contains(&y);
                let dark_center = (30..50).contains(&x) && (30..50).contains(&y);
                let v = if edge {
                    200u8
                } else if dark_center {
                    0u8
                } else {
                    100u8
                };
                img.put_pixel(x, y, Rgba([v, v, v, 255]));
            }
        }
        let dyn_img = DynamicImage::ImageRgba8(img);
        let out = crop_to_content(&dyn_img);
        assert_eq!(
            out.dimensions(),
            (80, 80),
            "端が明るければ黒帯検出されず寸法維持"
        );
    }

    // ---- 実画像検証（capture_probe_live.png = 1918x1048, 黒帯入り実測） ----
    //
    // ゲート(R1 三値化): デフォルト(`pc-e2e` feature OFF)では #[ignore]。
    // `cargo nextest run -p anaden-vision --features pc-e2e --run-ignored all` でのみ実行。
    // プローブ不在時は absence-skip せず image::open が fail-loud で panic する
    // (CI/fresh-clone が missing-probe を偽 green で報告しないための不変量)。

    #[test]
    #[cfg_attr(not(feature = "pc-e2e"), ignore)]
    fn crop_real_probe_image_removes_letterbox() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../capture_probe_live.png");
        let img = image::open(&path)
            .unwrap_or_else(|e| panic!("open capture_probe_live.png at {}: {e}", path.display()));
        let (w, h) = img.dimensions();
        assert_eq!((w, h), (1918, 1048), "実測キャプチャは 1918x1048");

        let out = crop_to_content(&img);
        let (cw, ch) = out.dimensions();
        eprintln!(
            "crop result: {w}x{h} -> {cw}x{ch} (aspect {:.3}, src aspect {:.3}, 16:9={:.3})",
            cw as f64 / ch as f64,
            w as f64 / h as f64,
            16.0 / 9.0
        );

        // クロップ後は元より小さくなるはず（黒帯が除去される）。
        assert!(cw <= w, "クロップ後幅は元以下");
        assert!(ch <= h, "クロップ後高さは元以下");
        // 最小寸法は維持（フォールバックでない）。
        assert!(cw >= MIN_CONTENT_PX);
        assert!(ch >= MIN_CONTENT_PX);

        // 1918x1048 のアスペクト比 1.831 は 16:9(1.778) より横長 = 左右に黒帯があるはず。
        // クロップ後は 16:9(1.778) に近づく、あるいは上下クロップで 1.778 前後になる。
        let src_aspect = w as f64 / h as f64;
        let out_aspect = cw as f64 / ch as f64;
        let target = 16.0 / 9.0;
        let src_err = (src_aspect - target).abs();
        let out_err = (out_aspect - target).abs();
        assert!(
            out_err <= src_err,
            "クロップ後アスペクト比({out_aspect:.3})は16:9({target:.3})へ近づくべき \
             (src_err={src_err:.3}, out_err={out_err:.3})"
        );
    }

    // ---- CropInfo: crop_to_content_with_info ----

    #[test]
    fn crop_info_no_bars_is_full_with_zero_offset() {
        // 黒帯なし画像 → CropInfo は offset=(0,0), size=元画像寸法。
        let img =
            DynamicImage::ImageRgb8(ImageBuffer::from_fn(200, 112, |_, _| Rgb([64, 96, 128])));
        let (out, info) = crop_to_content_with_info(&img);
        assert_eq!(out.dimensions(), (200, 112));
        assert_eq!(
            info,
            CropInfo::full(200, 112),
            "黒帯なし → CropInfo は元画像全体、オフセット無し"
        );
    }

    #[test]
    fn crop_info_with_bars_reports_content_offset_and_size() {
        // 中央 200x112 ＋ 上下左右 15px 黒帯 → 230x142。
        // crop_to_content は黒帯 15px + MARGIN_PX(2) = 17px ずつ内側へ寄せる。
        let img = synthetic_with_bars(200, 112, 15);
        assert_eq!(img.dimensions(), (230, 142));
        let (out, info) = crop_to_content_with_info(&img);
        // 画像寸法は crop_to_content と同一。
        assert_eq!(out.dimensions(), (196, 108));
        // CropInfo: offset = 17(=15+2), size = 残り寸法。
        assert_eq!(info.offset_x, 17);
        assert_eq!(info.offset_y, 17);
        assert_eq!(info.width, 196);
        assert_eq!(info.height, 108);
        // コンテンツ右端 = offset + size は元画像内に収まる（右黒帯17px分の余裕）。
        assert!(info.offset_x + info.width <= 230);
        assert!(info.offset_y + info.height <= 142);
        // コンテンツ右端 + 右黒帯(17) = 元画像寸法（境界整合）。
        assert_eq!(info.offset_x + info.width + 17, 230);
        assert_eq!(info.offset_y + info.height + 17, 142);
    }

    #[test]
    fn crop_info_letterbox_only_vertical_bars_zero_horizontal_offset() {
        // レターボックス（上下のみ）。左右黒帯なし → offset_x=0, width=元幅。
        let mut img: RgbaImage = ImageBuffer::new(160, 130);
        for y in 0..130 {
            for x in 0..160 {
                let in_content = (20..110).contains(&y);
                let v = if in_content { 100u8 } else { 0u8 };
                img.put_pixel(x, y, Rgba([v, v, v, 255]));
            }
        }
        let dyn_img = DynamicImage::ImageRgba8(img);
        let (_out, info) = crop_to_content_with_info(&dyn_img);
        assert_eq!(info.offset_x, 0, "左右黒帯なしなので offset_x=0");
        assert_eq!(info.width, 160, "左右黒帯なしなので width=元幅");
        // 上下は 20+2=22 ずつ。
        assert_eq!(info.offset_y, 22);
        assert_eq!(info.height, 130 - 2 * 22);
    }

    #[test]
    fn crop_info_fallback_all_black_is_full_with_zero_offset() {
        // 全面黒 → フォールバック。CropInfo は元画像全体（offset=0）。
        let img = DynamicImage::ImageRgb8(ImageBuffer::from_fn(100, 100, |_, _| Rgb([0, 0, 0])));
        let (out, info) = crop_to_content_with_info(&img);
        assert_eq!(out.dimensions(), (100, 100));
        assert_eq!(info, CropInfo::full(100, 100));
    }

    #[test]
    fn crop_info_zero_size_returns_default() {
        let img = DynamicImage::ImageRgb8(ImageBuffer::new(0, 0));
        let (out, info) = crop_to_content_with_info(&img);
        assert_eq!(out.dimensions(), (0, 0));
        assert_eq!(info, CropInfo::default());
    }

    // ---- crop_to_canvas_with_info: キャンバス アスペクト保護 (Issue #212) ----

    /// 中央に 16:9 コンテンツ + 左右黒帯のある画像 (真のピラーボックス) を作る。
    /// コンテンツ 320x180 (16:9) + 左右 bar px → 全面 (320+2*bar)x180。
    fn pillarbox_16_9(bar: u32) -> DynamicImage {
        let w = 320 + 2 * bar;
        let mut img: RgbaImage = ImageBuffer::new(w, 180);
        for y in 0..180 {
            for x in 0..w {
                let in_content = x >= bar && x < bar + 320;
                let v = if in_content { 128u8 } else { 0u8 };
                img.put_pixel(x, y, Rgba([v, v, v, 255]));
            }
        }
        DynamicImage::ImageRgba8(img)
    }

    /// 16:9 全面画像の下側 `bottom_black` 行だけ黒 (キャンバス内部の未描画領域を模擬)。
    fn canvas_with_unpainted_bottom(bottom_black: u32) -> DynamicImage {
        let mut img: RgbaImage = ImageBuffer::new(320, 180);
        for y in 0..180 {
            for x in 0..320 {
                let v = if y < 180 - bottom_black { 128u8 } else { 0u8 };
                img.put_pixel(x, y, Rgba([v, v, v, 255]));
            }
        }
        DynamicImage::ImageRgba8(img)
    }

    /// 真のピラーボックス (左右黒帯・中身は 16:9) はクロップが受理される。
    /// クロップ後 320x180 ≒ 16:9 (誤差 0) → guard を通過。
    #[test]
    fn canvas_guard_accepts_genuine_pillarbox() {
        let img = pillarbox_16_9(20);
        assert_eq!(img.dimensions(), (360, 180));
        let (out, info) = crop_to_canvas_with_info(&img);
        // 黒帯 20px + MARGIN_PX 2 = 左右 22px 除去 → 316x180 (アスペクト 1.7556・16:9 比 -1.2%)。
        assert_eq!(out.dimensions(), (316, 180));
        assert_eq!(info.offset_x, 22);
        assert_eq!(info.width, 316);
        assert_eq!(info.height, 180);
    }

    /// キャンバス内部の未描画黒 (下 90 行 = 50%) はクロップが棄却され元画像全体になる。
    /// crop_to_content だけなら 320x88 (アスペクト 3.636・+104%) になり幾何が破壊される。
    #[test]
    fn canvas_guard_rejects_unpainted_in_canvas_black() {
        let img = canvas_with_unpainted_bottom(90);
        // 素の crop_to_content は (Issue #212 実測と同じ罠) 幾何を壊す — その前提を pin。
        let (raw_cropped, _) = crop_to_content_with_info(&img);
        assert_eq!(
            raw_cropped.dimensions(),
            (320, 88),
            "素 crop は下 90+2 行を除去して非 16:9 になる (guard が必要な理由)"
        );
        // guard 付きは棄却して全面 (アスペクト 16:9 = 320/180) を返す。
        let (out, info) = crop_to_canvas_with_info(&img);
        assert_eq!(out.dimensions(), (320, 180), "幾何破壊クロップは棄却");
        assert_eq!(info, CropInfo::full(320, 180));
    }

    /// 黒帯なし画像は guard を通っても何も変わらない (恒等)。
    #[test]
    fn canvas_guard_no_bars_is_identity() {
        let img =
            DynamicImage::ImageRgb8(ImageBuffer::from_fn(320, 180, |_, _| Rgb([64, 96, 128])));
        let (out, info) = crop_to_canvas_with_info(&img);
        assert_eq!(out.dimensions(), (320, 180));
        assert_eq!(info, CropInfo::full(320, 180));
    }

    /// 全面黒フォールバックもそのまま透過する (CropInfo::full)。
    #[test]
    fn canvas_guard_all_black_falls_back_to_full() {
        let img = DynamicImage::ImageRgb8(ImageBuffer::from_fn(100, 100, |_, _| Rgb([0, 0, 0])));
        let (out, info) = crop_to_canvas_with_info(&img);
        assert_eq!(out.dimensions(), (100, 100));
        assert_eq!(info, CropInfo::full(100, 100));
    }

    /// 左右 8px 程度の小黒帯 (除去後も 16:9 に近い) は受理される。
    /// pillarbox_16_9(8): 全面 336x180 → 左右 8+2=10px 除去 → 316x180 (+2.5% 偏差)。
    #[test]
    fn canvas_guard_accepts_small_deviations_within_tolerance() {
        let img = pillarbox_16_9(8);
        assert_eq!(img.dimensions(), (336, 180));
        let (out, info) = crop_to_canvas_with_info(&img);
        assert_eq!(out.dimensions(), (316, 180));
        assert_eq!(info.offset_x, 10);
    }

    /// ゲート(R1 三値化): デフォルト(`pc-e2e` feature OFF)では #[ignore]。
    /// `cargo nextest run -p anaden-vision --features pc-e2e --run-ignored all` でのみ実行。
    /// プローブ不在時は absence-skip せず image::open が fail-loud で panic する。
    #[test]
    #[cfg_attr(not(feature = "pc-e2e"), ignore)]
    fn crop_info_real_probe_image_has_zero_or_nonzero_offset() {
        // 実画像（1918x1048 黒帯入り）。CropInfo が元画像空間の妥当な矩形を指すことだけ検証。
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../capture_probe_live.png");
        let img = image::open(&path)
            .unwrap_or_else(|e| panic!("open capture_probe_live.png at {}: {e}", path.display()));
        let (w, h) = img.dimensions();
        let (out, info) = crop_to_content_with_info(&img);
        // コンテンツ領域は元画像内に収まる。
        assert!(info.offset_x + info.width <= w, "offset_x+width <= 元幅");
        assert!(info.offset_y + info.height <= h, "offset_y+height <= 元高");
        assert_eq!((out.width(), out.height()), (info.width, info.height));
    }
}
