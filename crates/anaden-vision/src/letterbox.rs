//! レターボックス（黒帯）検出・クロップ。
//!
//! PC版 Live キャプチャはウィンドウサイズ・DPI に依存して描画領域(16:9)の周囲に
//! 黒帯（ピラーボックス=左右 / レターボックス=上下）が入ることがある。
//! テンプレートは描画領域(16:9)の生ピクセル空間で定義されているため、
//! 黒帯込みでマッチさせるとスケールがズレて NoMatch になる。
//!
//! `crop_to_content` は上下左右の端から「行/列の平均輝度 < 閾値」の**連続する**端領域を
//! 検出し、中央の描画領域をクロップする。黒帯がなければ元画像をそのまま返す。
//!
//! 設計上の安全弁:
//! - 端から連続する黒行/列のみを削る（ゲーム画面内部の暗部は誤判定しない）。
//! - **2段階検出**: まず行平均で上下黒帯を検出し、その後「上下黒帯を除いた行範囲」で
//!   列平均を再計算して左右黒帯を検出する。これによりレターボックス時に左右端の列平均が
//!   上下黒帯で汚染される相互汚染を防ぐ（逆方向も同様）。
//! - 黒帯境界のアンチエイリアスを吸収するため、検出された黒帯幅から
//!   [`MARGIN_PX`] だけ内側へ縮める余白を設ける。
//! - クロップ結果が描画領域を破壊しないよう、最小寸法 [`MIN_CONTENT_PX`] を下回る場合は
//!   クロップを行わず元画像を返す（フォールバック）。
//! - **キャンバス アスペクト保護 (Issue #212)**: [`crop_to_canvas_with_info`] は
//!   クロップ結果のアスペクト比が 16:9 ([`CANVAS_ASPECT`]) から
//!   [`CANVAS_ASPECT_TOLERANCE`] 以内に収まるときのみクロップを受理する。
//!   「キャンバス内部の黒」(起動途中フレームの未描画領域・実測 1952x1098 の下 521 行)
//!   を黒帯と誤判定して幾何 (アスペクト → roi の Y マッピング) を破壊するのを防ぐ。

use image::{DynamicImage, GenericImageView, ImageBuffer, Rgb};

/// 黒帯判定の行/列平均輝度閾値。Y=0.299R+0.587G+0.114B。
/// 真っ黒(0)〜ほぼ黒(8)までを黒帯とみなす。ゲーム描画領域の暗部は
/// 「端から連続」でなければ検出されないので、この閾値で実用上十分。
pub const BLACK_BAR_LUMINANCE_THRESHOLD: f64 = 8.0;

/// 黒帯境界のアンチエイリアス吸収用余白（ピクセル）。
/// 検出された黒帯幅からこの分だけ内側へ縮めることで、半透明境界画素を描画領域に残さない。
pub const MARGIN_PX: u32 = 2;

/// クロップ後の最小寸法（ピクセル）。これを下回る黒帯検出は異常とみなしフォールバック。
pub const MIN_CONTENT_PX: u32 = 64;

/// 黒帯除去後の描画領域（コンテンツ）の、**元画像（黒帯込み生幅）空間における位置と寸法**。
///
/// `crop_to_content_with_info` が返す。`offset_x`/`offset_y` は元画像左上を原点とした
/// コンテンツ領域左上のピクセル座標、`width`/`height` はコンテンツ領域の寸法。
/// 黒帯が無ければ `offset=(0,0)`・`size=元画像寸法` になる（=クロップ対象なし）。
///
/// 発火座標の逆変換（normalize後1280空間 → 実機生画像）に用いる:
/// 入力画像は normalize でコンテンツ領域を 1280 幅へ拡大されるため、1280 空間の座標を
/// 元画像（黒帯込み）へ戻すには `x_real = x_1280 * width / 1280 + offset_x` のように
/// コンテンツ領域内でスケールした上で黒帯オフセット分を平行移動させる必要がある。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CropInfo {
    /// 元画像（黒帯込み）左上を原点とした、コンテンツ領域左上の X 座標。
    pub offset_x: u32,
    /// 元画像（黒帯込み）左上を原点とした、コンテンツ領域左上の Y 座標。
    pub offset_y: u32,
    /// コンテンツ領域（黒帯除去後描画領域）の幅。
    pub width: u32,
    /// コンテンツ領域（黒帯除去後描画領域）の高さ。
    pub height: u32,
}

impl CropInfo {
    /// コンテンツ領域が元画像全体と一致する（黒帯なし）CropInfo を作る。
    pub fn full(w: u32, h: u32) -> Self {
        Self {
            offset_x: 0,
            offset_y: 0,
            width: w,
            height: h,
        }
    }
}

/// `img` の上下左右の黒帯を検出し、中央の描画領域をクロップして返す。
///
/// 黒帯（平均輝度 < `BLACK_BAR_LUMINANCE_THRESHOLD` の端から連続する行/列）が
/// 上下・左右いずれにも存在しない場合は、元画像を複製して返す。
///
/// # 引数
/// - `img`: 入力画像（任意のピクセル形式。内部で RGB8 に変換して処理）。
///
/// # 戻り値
/// 黒帯が除去された描画領域画像。入力が空(0x0)の場合は入力の複製を返す。
///
/// 黒帯除去後の元画像空間における位置・寸法（[`CropInfo`]）も必要な場合は
/// [`crop_to_content_with_info`] を使うこと（発火座標の逆変換等）。
pub fn crop_to_content(img: &DynamicImage) -> DynamicImage {
    crop_to_content_with_info(img).0
}

/// [`crop_to_content`] に加え、クロップ後描画領域の元画像空間における位置・寸法（[`CropInfo`]）も返す。
///
/// 戻り値は `(クロップ後画像, CropInfo)`。`CropInfo` は:
/// - 黒帯あり → コンテンツ領域の左上オフセット（`offset_x`/`offset_y`）と寸法。
/// - 黒帯なし → `offset=(0,0)`, `size=元画像寸法`（[`CropInfo::full`]）。
/// - フォールバック（全面黒等で残りが `MIN_CONTENT_PX` 未満）→ 元画像寸法で `offset=(0,0)`。
/// - 入力が空(0x0) → `(0,0,0,0)`。
///
/// この `CropInfo` は発火座標の逆変換（normalize後1280空間 → 元画像＝黒帯込み生画像）に
/// 必要。入力画像は [`crate::scale::ScreenScaler::normalize`] でコンテンツ領域が1280幅へ
/// 拡大されるため、1280空間の座標を元画像へ戻すにはコンテンツ領域内でスケールした上で
/// `offset` 分の平行移動が必要となる。
pub fn crop_to_content_with_info(img: &DynamicImage) -> (DynamicImage, CropInfo) {
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return (img.clone(), CropInfo::default());
    }

    let rgb = img.to_rgb8();

    // 第1段階: 行平均（全幅）で上下黒帯を検出。
    let row_means = row_mean_luminance(&rgb, 0, w);
    let top = leading_black_count(&row_means, BLACK_BAR_LUMINANCE_THRESHOLD);
    let bottom = trailing_black_count(&row_means, BLACK_BAR_LUMINANCE_THRESHOLD);

    // 第2段階: 上下黒帯を除いた行範囲 [top..h-bottom) で列平均を再計算し、左右黒帯を検出。
    // これによりレターボックス時に左右端の列平均が上下黒帯で汚染されるのを防ぐ。
    let row_lo = top;
    let row_hi = h.saturating_sub(bottom);
    let col_means = col_mean_luminance(&rgb, row_lo, row_hi);
    let left = leading_black_count(&col_means, BLACK_BAR_LUMINANCE_THRESHOLD);
    let right = trailing_black_count(&col_means, BLACK_BAR_LUMINANCE_THRESHOLD);

    // 余白を内側へ寄せる（アンチエイリアス吸収）。黒帯が検出された場合のみ、その幅に
    // MARGIN_PX を足して境界の半透明画素を描画領域に残さない。黒帯未検出(0)なら 0。
    let crop_top = margin_offset(top, h);
    let crop_bottom = margin_offset(bottom, h);
    let crop_left = margin_offset(left, w);
    let crop_right = margin_offset(right, w);

    // 残り幅/高さが最小寸法を下回るなら異常（黒画面等）→ フォールバック。
    let remaining_w = w.saturating_sub(crop_left + crop_right);
    let remaining_h = h.saturating_sub(crop_top + crop_bottom);
    if remaining_w < MIN_CONTENT_PX || remaining_h < MIN_CONTENT_PX {
        return (img.clone(), CropInfo::full(w, h));
    }

    // 黒帯が一切検出されなければクロップ不要 → 複製を返す（アロケーション節約は呼び出し側で可）。
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

/// 各行の平均輝度を、列範囲 `[x_lo, x_hi)` に限定して計算する。
/// `x_lo=0, x_hi=w` で全幅の行平均になる。
fn row_mean_luminance(rgb: &ImageBuffer<Rgb<u8>, Vec<u8>>, x_lo: u32, x_hi: u32) -> Vec<f64> {
    let (_, h) = rgb.dimensions();
    let span = (x_hi.saturating_sub(x_lo)) as f64;
    if span <= 0.0 {
        return vec![0.0_f64; h as usize];
    }
    let mut means = vec![0.0_f64; h as usize];
    for y in 0..h {
        let mut sum = 0.0_f64;
        for x in x_lo..x_hi {
            let p = rgb.get_pixel(x, y);
            sum += 0.299 * p[0] as f64 + 0.587 * p[1] as f64 + 0.114 * p[2] as f64;
        }
        means[y as usize] = sum / span;
    }
    means
}

/// 各列の平均輝度を、行範囲 `[y_lo, y_hi)` に限定して計算する。
/// 上下黒帯を除外した行範囲で列平均を取ることで相互汚染を防ぐ。
fn col_mean_luminance(rgb: &ImageBuffer<Rgb<u8>, Vec<u8>>, y_lo: u32, y_hi: u32) -> Vec<f64> {
    let (w, _) = rgb.dimensions();
    let span = (y_hi.saturating_sub(y_lo)) as f64;
    if span <= 0.0 {
        return vec![0.0_f64; w as usize];
    }
    let mut means = vec![0.0_f64; w as usize];
    for x in 0..w {
        let mut sum = 0.0_f64;
        for y in y_lo..y_hi {
            let p = rgb.get_pixel(x, y);
            sum += 0.299 * p[0] as f64 + 0.587 * p[1] as f64 + 0.114 * p[2] as f64;
        }
        means[x as usize] = sum / span;
    }
    means
}

/// 先頭（インデックス 0..）から連続する黒要素の個数を返す。
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

/// 末尾（インデックス len-1..）から連続する黒要素の個数を返す。
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

/// 検出された黒帯幅 `bar` に [`MARGIN_PX`] の余白を加えたクロップ量を返す。
/// 黒帯が検出されていない（`bar == 0`）場合は余白も加えず 0 を返す（黒帯なし画像を誤って削らない）。
/// 結果は `max`（画像の高さ/幅）でクランプする。
fn margin_offset(bar: u32, max: u32) -> u32 {
    if bar == 0 {
        0
    } else {
        bar.saturating_add(MARGIN_PX).min(max)
    }
}

/// ゲーム描画領域 (キャンバス) の期待アスペクト比 (16:9)。
///
/// PC 版実測クライアント領域 1258x708 (=1.7768) も 16:9 (=1.7778) との差 0.06% で
/// 本定数の許容誤差内に収まる。
pub const CANVAS_ASPECT: f64 = 16.0 / 9.0;

/// [`CANVAS_ASPECT`] に対するクロップ受理許容誤差 (相対・5%)。
///
/// 実測の健全なクロップ結果は 0.6% 以内 (1952x1098 の左 8px 黒帯除去後 1942x1098 =
/// 1.7685・16:9 比 +0.52%) に収まる。一方 **キャンバス内部の黒** (起動途中の未描画
/// 領域・暗画面) を誤って黒帯扱いしたクロップは大きく崩れる (1952x1098 の下 521 行が
/// 黒 → 1952x577 = 3.373・+89.6%)。5% はこの 2 群を弁別する十分広い閾値。
pub const CANVAS_ASPECT_TOLERANCE: f64 = 0.05;

/// [`crop_to_content_with_info`] に **キャンバス アスペクト保護** を加えたクロップ。
///
/// Issue #212 の根因対策: `crop_to_content_with_info` は「端から連続する黒行/列」を
/// 黒帯として除去するが、黒帯と **キャンバス内部の黒** (起動途中フレームの未描画下半分、
/// 暗いロード画面の黒帯状余白等) を区別できない。内部の黒を除去すると:
///
/// - クロップ後寸法のアスペクト比が 16:9 から大きく崩れ (実測: 1952x1098 → 1952x577 = 3.373)、
/// - normalize 後の縦スケールが崩壊し (1280x720 になるべきが 1280x378)、
/// - `roi_to_normalized` の Y マッピングが約 0.53 倍に圧縮されて roi 窓が外れ、
/// - needle も縦に圧縮されるため全文探索でもマッチしない (NoMatch)。
///
/// 本関数はクロップ結果のアスペクト比が [`CANVAS_ASPECT`] から [`CANVAS_ASPECT_TOLERANCE`]
/// 以内に収まるときのみクロップを受理し、大きく外れる場合は **クロップを棄却して元画像
/// 全体** (=`CropInfo::full`) を返す。棄却時も座標系 (全面 = キャンバス) は保たれるため、
/// roi/needle のスケールは正しいまま「マッチしない」判定だけが残る (= 未描画フレームに
/// 対する正しい挙動。リトライ/リカバリに委ねる)。
///
/// 黒帯が検出されなかった・フォールバックしたケースは [`crop_to_content_with_info`]
/// と同一の結果 (元画像 + full) を返す。
pub fn crop_to_canvas_with_info(img: &DynamicImage) -> (DynamicImage, CropInfo) {
    let (w, h) = img.dimensions();
    let (cropped, info) = crop_to_content_with_info(img);
    // クロップが実質働いていない (黒帯なし / フォールバック / 空画像) はそのまま。
    if info.offset_x == 0 && info.offset_y == 0 && info.width == w && info.height == h {
        return (cropped, info);
    }
    // クロップ結果がキャンバスアスペクトを保っているか。保っていなければ内部黒の
    // 誤検出 (幾何破壊) として棄却し、元画像全体 (= キャンバス) を使う。
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
