//! テンプレート画像の構造検証 (無構造テンプレート検出 — Issue #184)。
//!
//! Issue #182 の実機 E2E で、`templates/scenes/title_pc/version_label.png`
//! (旧テンプレート) が輝度 stddev 3.76 のほぼ白一色だったことで
//! TM_CCOEFF_NORMED が原理的に match できず、65 iterations で発火 0 という
//! 恒久 NoMatch が発生した。この種の欠陥テンプレートは保存経路・loader の
//! いずれでも検証されていなかったため素通りした。
//!
//! 本モジュールは輝度 stddev の計算と閾値判定の **単一実装** を提供する。
//! GUI 保存経路 (anaden-studio) と pipeline loader ([`crate::pipeline::load_pipeline`])
//! はこの実装を呼び、二重実装による閾値・計算法の drift を防ぐ (Issue #184 受入基準)。

use std::path::Path;

use image::DynamicImage;

/// テンプレートが「構造を持つ」(= マッチ可能) と判定する輝度 stddev の下限閾値。
///
/// 根拠は Issue #182 の実測値:
///
/// | テンプレート | 輝度 stddev | 実機結果 |
/// |---|---|---|
/// | 旧 version_label.png (ほぼ白一色・無構造) | 3.76 | 65 iterations で発火 0 (恒久 NoMatch) |
/// | 再生成版 version_label.png (ID 表示帯 138x20) | 53.1 | 実機でマッチ成立・フィールド到達 |
///
/// 3.76 (無構造) と 53.1 (実構造) の中間で両側に十分なマージンを保てる値として
/// 20.0 を採用する (テスト `template_min_luma_stddev_sits_strictly_between_measured_values`
/// がこの根拠との乖離を検出する)。
pub const TEMPLATE_MIN_LUMA_STDDEV: f32 = 20.0;

/// テンプレート画像全体の輝度 (luma) 標準偏差を返す。
///
/// - 変換は [`DynamicImage::to_luma8`] (Rec.601) — テンプレートマッチの前処理と同一。
/// - ほぼ単色 (クロップ領域ミスで全面白/黑/単色になった場合など) は stddev ≈ 0 に
///   近い値を返し、[`template_is_structured`] が false になる。
/// - 0x0 画像 (幅・高さゼロ) は画素を持たないため 0.0 を返す (パニックしない)。
///
/// # Examples
///
/// ```
/// use anaden_vision::template_luma_stddev;
///
/// // 単色画像の stddev は 0。
/// let flat = image::DynamicImage::ImageLuma8(
///     image::GrayImage::from_pixel(32, 16, image::Luma([255u8])),
/// );
/// assert_eq!(template_luma_stddev(&flat), 0.0);
/// ```
#[must_use]
pub fn template_luma_stddev(img: &DynamicImage) -> f32 {
    let gray = img.to_luma8();
    let mut n: f64 = 0.0;
    let mut sum: f64 = 0.0;
    let mut sum_sq: f64 = 0.0;
    for p in gray.pixels() {
        let v = f64::from(p.0[0]);
        n += 1.0;
        sum += v;
        sum_sq += v * v;
    }
    if n == 0.0 {
        return 0.0;
    }
    let mean = sum / n;
    // 分散 = E[x^2] - E[x]^2。値域が u8 (<= 255^2 = 65,025) のため f64 の
    // 桁落ちは stddev 測定精度 (0.01 程度) に対して無視できる。
    // 浮動小数点誤差で負にならないよう clamp する。
    let var = (sum_sq / n - mean * mean).max(0.0);
    var.sqrt() as f32
}

/// テンプレート画像がマッチ可能な構造を持つか (stddev >= [`TEMPLATE_MIN_LUMA_STDDEV`])。
///
/// 無構造 (ほぼ単色) のテンプレートは TM_CCOEFF_NORMED の分母 (テンプレート分散)
/// がほぼ 0 になるため原理的にマッチできず、恒久 NoMatch になる
/// (Issue #182 実機証拠: stddev 3.76 → 65 iters 発火 0)。
///
/// # Examples
///
/// ```
/// use anaden_vision::template_is_structured;
///
/// // チェッカーボード (0/255) は stddev 約 127.5 → 構造あり。
/// let mut img = image::GrayImage::new(32, 32);
/// for y in 0..32 {
///     for x in 0..32 {
///         let v: u8 = if (x + y) % 2 == 0 { 0 } else { 255 };
///         img.put_pixel(x, y, image::Luma([v]));
///     }
/// }
/// assert!(template_is_structured(&image::DynamicImage::ImageLuma8(img)));
/// ```
#[must_use]
pub fn template_is_structured(img: &DynamicImage) -> bool {
    template_luma_stddev(img) >= TEMPLATE_MIN_LUMA_STDDEV
}

/// `path` のテンプレート画像が無構造の場合に loader 向け警告文を返す。
///
/// [`crate::pipeline::load_pipeline`] が needle PNG 読込時に呼ぶ (Issue #184)。
/// 戻り値:
/// - `Some(warning)`: ファイルが読め・かつ stddev が [`TEMPLATE_MIN_LUMA_STDDEV`]
///   未満 (無構造 = 恒久 NoMatch 想定)。呼出側は `tracing::warn!` で表示する。
/// - `None`: 構造あり、ファイル不在、デコード失敗のいずれか。
///   load を失敗させない (既存 pipeline 資産の後方互換 — GUI 保存時の
///   fail-visible 警告と異なり、loader は資産を弾かない)。
pub(crate) fn unstructured_warning_for_path(path: &Path) -> Option<String> {
    let img = image::open(path).ok()?;
    let stddev = template_luma_stddev(&img);
    if stddev >= TEMPLATE_MIN_LUMA_STDDEV {
        return None;
    }
    Some(format!(
        "template {} is nearly uniform (luma stddev {:.2} < {:.1}): it can never match \
         under TM_CCOEFF_NORMED — permanent NoMatch expected (Issue #182: an unstructured \
         template measured stddev 3.76 and never fired in 65 iterations)",
        path.display(),
        stddev,
        TEMPLATE_MIN_LUMA_STDDEV
    ))
}

/// needle (テンプレート PNG) が TaskDef の ROI に収まるか (Issue #187)。
///
/// needle の幅/高さがそれぞれ ROI の幅/高さ以下であることがマッチの必要条件。
/// [`crate::pipeline::TaskDef::detect`] は ROI と needle を同じ X/Y 比でスケールする
/// ([`crate::scale::roi_to_normalized`] / `crate::scale::needle_to_normalized`) ため、
/// 定義空間 (raw-1258 等) でのこの比較は正規化後も保たれる (両辺が同係数で
/// 単調変換される)。needle が ROI より大きいと cropping 済み haystack 内に
/// needle が置けず恒久 NoMatch になる (Issue #182: 再生成テンプレ 138px 幅 vs
/// 旧 roi 幅 121 で発火しなかった実例)。
///
/// - `roi = None` は全面走査 ([`crate::pipeline::TaskDef::roi`]) のため常に `true`。
/// - ROI の幅/高さが 0 の組み合わせも全面扱い (`true`) —
///   `crate::pipeline::TaskDef::roi_to_region` が w/h 0 を `None` (全面) に
///   正規化する挙動と揃える。
///
/// # Examples
///
/// ```
/// use anaden_vision::needle_fits_roi;
///
/// // needle ≤ roi → 収まる (等辺も含む)。
/// assert!(needle_fits_roi((100, 50), Some([10, 20, 100, 50])));
/// // roi None = 全面 → 常に収まる。
/// assert!(needle_fits_roi((4000, 2000), None));
/// // needle 幅超過 → 恒久 NoMatch。
/// assert!(!needle_fits_roi((138, 20), Some([8, 2, 121, 35])));
/// ```
#[must_use]
pub fn needle_fits_roi(needle: (u32, u32), roi: Option<[u32; 4]>) -> bool {
    let [_, _, rw, rh] = match roi {
        None => return true,
        Some(r) => r,
    };
    rw == 0 || rh == 0 || (needle.0 <= rw && needle.1 <= rh)
}

/// `path` のテンプレート画像が ROI に収まらない場合に loader 向け警告文を返す
/// (Issue #187・[`needle_fits_roi`] のパス版)。
///
/// [`crate::pipeline::load_pipeline`] が TaskDef の `template` × `roi` 検証に呼ぶ。
/// 画像寸法は [`image::image_dimensions`] (ヘッダのみ読む・デコードしない) で取得する。
///
/// - `Some(warning)`: ファイルが読め・かつ needle が roi に収まらない
///   (= 恒久 NoMatch 想定)。呼出側は `tracing::warn!` で表示する。
/// - `None`: 収まる、roi = 全面、ファイル不在、デコード失敗のいずれか。
///   [`unstructured_warning_for_path`] と同じく load は失敗させない (fail-visible)。
pub(crate) fn needle_exceeds_roi_warning_for_path(
    path: &Path,
    roi: Option<[u32; 4]>,
) -> Option<String> {
    let (nw, nh) = image::image_dimensions(path).ok()?;
    let [_, _, rw, rh] = roi?;
    if needle_fits_roi((nw, nh), roi) {
        return None;
    }
    Some(format!(
        "template {} is {nw}x{nh} but roi is {rw}x{rh}: the needle cannot fit inside the \
         roi — permanent NoMatch expected (Issue #182: a regenerated 138px-wide needle \
         against a 121px-wide roi never fired)",
        path.display()
    ))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use image::{GrayImage, Luma};

    fn dyn_img(img: GrayImage) -> DynamicImage {
        DynamicImage::ImageLuma8(img)
    }

    /// 単色画像 (stddev = 0) は無構造。
    #[test]
    fn flat_image_is_not_structured() {
        let flat = dyn_img(GrayImage::from_pixel(64, 32, Luma([255u8])));
        assert_eq!(template_luma_stddev(&flat), 0.0);
        assert!(!template_is_structured(&flat));
    }

    /// 低コントラスト画像 (stddev 約 1.2 — Issue #182 の旧テンプレート 3.76 相当の
    /// 「ほぼ単色」) も無構造。全面同色でなくても分散が閾値未満なら恒久 NoMatch。
    #[test]
    fn near_flat_low_contrast_image_is_not_structured() {
        let mut img = GrayImage::new(64, 32);
        for y in 0..32 {
            for x in 0..64 {
                // 値域 100..=104 → stddev 約 1.2 (一様分布の √(var)).
                let v = 100 + ((x + y) % 5) as u8;
                img.put_pixel(x, y, Luma([v]));
            }
        }
        let img = dyn_img(img);
        let stddev = template_luma_stddev(&img);
        assert!(
            (1.0..2.0).contains(&stddev),
            "near-flat stddev should be ~1.2, got {stddev}"
        );
        assert!(!template_is_structured(&img));
    }

    /// 実構造テンプレート相当 (0..=199 の勾配 → stddev 約 57.7。Issue #182 再生成品
    /// の実測 53.1 と同水準) は構造ありと判定される (偽陽性ゼロの受入基準)。
    #[test]
    fn structured_gradient_image_is_structured() {
        let mut gray = GrayImage::new(64, 32);
        for y in 0..32 {
            for x in 0..64 {
                let v = ((x * 2 + y * 3) % 200) as u8;
                gray.put_pixel(x, y, Luma([v]));
            }
        }
        let img = dyn_img(gray);
        let stddev = template_luma_stddev(&img);
        assert!(
            stddev > TEMPLATE_MIN_LUMA_STDDEV,
            "gradient stddev {stddev:.2} must exceed threshold"
        );
        assert!(template_is_structured(&img));
    }

    /// RGBA 画像でも luma 変換後に判定される (GUI crop は RGBA を返しうる)。
    #[test]
    fn rgba_image_is_judged_after_luma_conversion() {
        let img = DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            64,
            32,
            image::Rgba([255u8, 255, 255, 255]),
        ));
        assert!(!template_is_structured(&img));
    }

    /// 0x0 画像は 0.0 を返しパニックしない (エッジケース)。
    #[test]
    fn empty_image_returns_zero_stddev() {
        let empty = dyn_img(GrayImage::new(0, 0));
        assert_eq!(template_luma_stddev(&empty), 0.0);
        assert!(!template_is_structured(&empty));
    }

    /// 閾値定数は Issue #182 の実測値 (無構造 3.76 / 実構造 53.1) の中間に
    /// 厳密に位置すること。const block によるコンパイル時検証 (条件が定数のため
    /// clippy::assertions_on_constants の推奨形式) — 定数を実測値の外へ動かすと
    /// ビルドが壊れ、doc comment の根拠との乖離を検出できる。
    #[test]
    fn template_min_luma_stddev_sits_strictly_between_measured_values() {
        /// Issue #182 旧 version_label.png (ほぼ白一色) の実測 stddev。
        const ISSUE_182_UNSTRUCTURED_MEASURED: f32 = 3.76;
        /// Issue #182 再生成版 version_label.png (ID 表示帯 138x20) の実測 stddev。
        const ISSUE_182_STRUCTURED_MEASURED: f32 = 53.1;
        const _: () = assert!(
            ISSUE_182_UNSTRUCTURED_MEASURED < TEMPLATE_MIN_LUMA_STDDEV
                && TEMPLATE_MIN_LUMA_STDDEV < ISSUE_182_STRUCTURED_MEASURED,
            "TEMPLATE_MIN_LUMA_STDDEV must sit strictly between the Issue #182 measured \
             values (unstructured 3.76 / structured 53.1)"
        );
    }

    /// loader 向けパス判定: 無構造 PNG ファイルは Some (警告文に stddev と閾値を含む)。
    #[test]
    fn unstructured_warning_for_path_flags_flat_png() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("flat.png");
        dyn_img(GrayImage::from_pixel(64, 32, Luma([250u8])))
            .save(&path)
            .expect("save flat png");

        let warn = unstructured_warning_for_path(&path).expect("flat png must be flagged");
        assert!(
            warn.contains("stddev") && warn.contains("0.00"),
            "warning should cite the measured stddev: {warn}"
        );
        assert!(
            warn.contains("TM_CCOEFF_NORMED"),
            "warning should explain the permanent-NoMatch consequence: {warn}"
        );
    }

    /// loader 向けパス判定: 構造あり PNG ファイルは None (偽陽性ゼロ)。
    #[test]
    fn unstructured_warning_for_path_passes_structured_png() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("structured.png");
        let mut img = GrayImage::new(64, 32);
        for y in 0..32 {
            for x in 0..64 {
                let v = ((x * 2 + y * 3) % 200) as u8;
                img.put_pixel(x, y, Luma([v]));
            }
        }
        dyn_img(img).save(&path).expect("save structured png");
        assert!(unstructured_warning_for_path(&path).is_none());
    }

    /// loader 向けパス判定: ファイル不在・デコード不能は None
    /// (load を失敗させない — 存在検証は detect の責務)。
    #[test]
    fn unstructured_warning_for_path_tolerates_missing_and_invalid_files() {
        let missing = unstructured_warning_for_path(Path::new("/nonexistent/no-such.png"));
        assert!(missing.is_none(), "missing file must not be flagged");

        let tmp = tempfile::tempdir().expect("tempdir");
        let not_png = tmp.path().join("not-a-png.png");
        std::fs::write(&not_png, b"this is not a png").expect("write junk");
        assert!(
            unstructured_warning_for_path(&not_png).is_none(),
            "undecodable file must not be flagged"
        );
    }

    // ---- Issue #187: needle/roi 幅検証 ----

    /// needle ≤ roi は収まる。等辺 (needle 寸法 == roi 寸法) も収まる。
    #[test]
    fn needle_fits_roi_when_dims_do_not_exceed_roi() {
        // needle が roi より小さい。
        assert!(needle_fits_roi((80, 40), Some([820, 470, 200, 90])));
        // 等辺 (crop == roi から作った needle の典型形)。
        assert!(needle_fits_roi((100, 50), Some([10, 20, 100, 50])));
        // 幅だけ等しい・高さだけ等しい。
        assert!(needle_fits_roi((100, 40), Some([10, 20, 100, 50])));
        assert!(needle_fits_roi((90, 50), Some([10, 20, 100, 50])));
    }

    /// roi None = 全面走査 → どんな needle でも収まる。
    #[test]
    fn needle_fits_roi_none_means_fullscreen() {
        assert!(needle_fits_roi((4000, 2000), None));
        assert!(needle_fits_roi((0, 0), None));
    }

    /// needle 幅 or 高さが roi を超えると収まらない (恒久 NoMatch)。
    /// Issue #182 の実例 (再生成テンプレ 138x20 vs 旧 roi [8,2,121,35]) を pin。
    #[test]
    fn needle_exceeding_roi_does_not_fit() {
        // 幅超過 (Issue #182 の 138 > 121)。
        assert!(!needle_fits_roi((138, 20), Some([8, 2, 121, 35])));
        // 高さ超過。
        assert!(!needle_fits_roi((80, 91), Some([820, 470, 200, 90])));
        // 幅・高さとも超過。
        assert!(!needle_fits_roi((300, 200), Some([820, 470, 200, 90])));
    }

    /// roi の幅/高さ 0 は [`crate::pipeline::TaskDef::roi_to_region`] と同じく
    /// 全面扱い (true) — 検出側の正規化挙動と揃える。
    #[test]
    fn needle_fits_roi_treats_zero_wh_as_fullscreen() {
        assert!(needle_fits_roi((100, 50), Some([10, 20, 0, 90])));
        assert!(needle_fits_roi((100, 50), Some([10, 20, 100, 0])));
        assert!(needle_fits_roi((100, 50), Some([10, 20, 0, 0])));
    }

    /// loader 向けパス判定: needle 寸法 > roi 寸法の PNG は Some
    /// (警告文に両寸法を含む)。
    #[test]
    fn needle_exceeds_roi_warning_for_path_flags_oversized_png() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("big.png");
        // 138x20 の構造あり PNG (stddev warn には掛からないサイズ検証のみの対象)。
        dyn_img(GrayImage::new(138, 20))
            .save(&path)
            .expect("save png");

        let warn = needle_exceeds_roi_warning_for_path(&path, Some([8, 2, 121, 35]))
            .expect("oversized needle must be flagged");
        assert!(
            warn.contains("138x20") && warn.contains("121x35"),
            "warning should cite needle and roi dims: {warn}"
        );
        assert!(
            warn.contains("permanent NoMatch"),
            "warning should explain the permanent-NoMatch consequence: {warn}"
        );
    }

    /// loader 向けパス判定: 収まる needle・roi None・ファイル不在は None
    /// (偽陽性ゼロ・load を失敗させない)。
    #[test]
    fn needle_exceeds_roi_warning_for_path_passes_fitting_cases() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("fit.png");
        dyn_img(GrayImage::new(100, 50))
            .save(&path)
            .expect("save png");

        // 収まる (等辺含む)。
        assert!(needle_exceeds_roi_warning_for_path(&path, Some([10, 20, 100, 50])).is_none());
        assert!(needle_exceeds_roi_warning_for_path(&path, Some([10, 20, 200, 90])).is_none());
        // roi None = 全面。
        assert!(needle_exceeds_roi_warning_for_path(&path, None).is_none());
        // ファイル不在。
        assert!(
            needle_exceeds_roi_warning_for_path(
                Path::new("/nonexistent/no-such.png"),
                Some([10, 20, 1, 1])
            )
            .is_none(),
            "missing file must not be flagged"
        );
    }
}
