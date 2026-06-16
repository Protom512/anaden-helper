//! グレースケール `TM_CCOEFF_NORMED` によるテンプレートマッチエンジン。
//!
//! SSE（[`crate::matcher`]）が絶対輝度差を測るため輝度シフト（明るさの一律変動）に弱い
//! 弱点を、テンプレート・窓双方の平均を引くことで補う。OpenCV の
//! `TM_CCOEFF_NORMED` と等価な相関係数を純Rustで計算する。
//!
//! # 数式
//!
//! 各走査位置 `(x, y)` に対する相関係数 `r(x,y) ∈ [-1, 1]`:
//!
//! ```text
//! r(x,y) = Σ(T' · I'_patch) / sqrt( Σ T'² · Σ I'_patch² )
//! ```
//!
//! ここで:
//! - `T'[x',y'] = T(x',y') - μT`（平均除去テンプレート、`μT` は位置非依存で1回計算）
//! - `I'_patch = I(x+x', y+y') - μI(x,y)`（窓画像の平均除去）
//! - `μI(x,y) = (1/N) Σ I(x+x', y+y')`（位置依存の窓平均）
//! - `N = w · h`
//!
//! # 計算手順
//!
//! 1. 前処理（テンプレート側、1回）: `μT`, `T'`, `denomT = sqrt(Σ T'²)`。
//!    `denomT == 0`（一様テンプレート）なら全位置 `0` を返す。
//! 2. haystack 側の積分図を2本構築（`Σ I`, `Σ I²`）。
//! 3. 各位置 `(x,y)`: 窓和 `sumI`, `sumI2` を O(1) で取り、
//!    `μI = sumI / N`、`denomI = sqrt(sumI2 - N·μI²)` を計算。
//!    分子は明示的に `Σ T' · (I - μI) = Σ T'·I - μI·ΣT'` で計算（`ΣT'=0`
//!    の恒等式に依存せず、MAA 等価性を確実に保つ）。
//!    `denomI ≈ 0`（一様窓）のとき `0` を返す（ゼロ割り回避）。
//!
//! # confidence 写像
//!
//! `r ∈ [-1, 1]` を `[0, 1]` に `clamp(r, 0.0, 1.0)` で写像する。
//! 負の相関（反転パターン）は不一致扱いで `0`。MAA の `minMaxLoc` が
//! 最大値を取る挙動と整合する。
//!
//! 詳細は Wiki [[Vision-Engine-Design]] / [[MAA-Matching-Algorithms]] 参照。

use image::{DynamicImage, GrayImage, Luma};
use imageproc::integral_image::{integral_image, integral_squared_image, sum_image_pixels};
use tracing::debug;

use anaden_core::{MatchConfidence, ScreenRegion};

use crate::engine::VisionEngine;
use crate::matcher::MatchResult;

/// 積分図の画素型。`u64` で `255² · W · H` を十分に保持できる。
type IntegralImage = image::ImageBuffer<Luma<u64>, Vec<u64>>;

/// `TM_CCOEFF_NORMED` によるテンプレートマッチエンジン。
///
/// [`crate::engine::SseVisionEngine`] と同じ [`VisionEngine`] trait を実装し、
/// 輝度シフトに強い相関ベースのマッチングを提供する。
pub struct CcoeffVisionEngine {
    threshold: MatchConfidence,
    downscale_factor: u32,
}

impl CcoeffVisionEngine {
    /// 閾値とダウンスケール倍率を指定して作成する。
    pub fn new(threshold: MatchConfidence, downscale_factor: u32) -> Self {
        let factor = if downscale_factor == 0 { 1 } else { downscale_factor };
        Self {
            threshold,
            downscale_factor: factor,
        }
    }

    /// デフォルト設定（閾値95%、1/2ダウンスケール）で作成する。
    /// [`crate::engine::SseVisionEngine::with_defaults`] と同値の方針。
    pub fn with_defaults() -> Self {
        Self::new(MatchConfidence::DEFAULT_THRESHOLD, 2)
    }

    /// ダウンスケールなし（テスト用）。[`crate::matcher::TemplateMatcher::threshold_only`] 相当。
    pub fn threshold_only(threshold: MatchConfidence) -> Self {
        Self::new(threshold, 1)
    }

    /// 両画像をダウンスケールする。テンプレートが画面より大きい場合は `None`。
    ///
    /// [`crate::matcher::TemplateMatcher`] と同じロジック（独立に保持して SSE 側は触らない）。
    fn downscale_images(
        &self,
        haystack: &DynamicImage,
        needle: &DynamicImage,
    ) -> Option<(DynamicImage, DynamicImage)> {
        if self.downscale_factor > 1 {
            let f = self.downscale_factor;
            let h = haystack.resize_exact(
                haystack.width() / f,
                haystack.height() / f,
                image::imageops::FilterType::Triangle,
            );
            let n = needle.resize_exact(
                needle.width() / f,
                needle.height() / f,
                image::imageops::FilterType::Triangle,
            );

            if n.width() > h.width() || n.height() > h.height() {
                debug!(
                    "Template ({}x{}) larger than screen ({}x{}), skipping",
                    n.width(),
                    n.height(),
                    h.width(),
                    h.height()
                );
                return None;
            }

            Some((h, n))
        } else {
            if needle.width() > haystack.width() || needle.height() > haystack.height() {
                return None;
            }
            Some((haystack.clone(), needle.clone()))
        }
    }
}

impl VisionEngine for CcoeffVisionEngine {
    fn match_template(&self, haystack: &DynamicImage, needle: &DynamicImage) -> Option<MatchResult> {
        let (h, n) = self.downscale_images(haystack, needle)?;
        let hg = h.to_luma8();
        let ng = n.to_luma8();

        let nw = ng.width();
        let nh = ng.height();
        let rw = hg.width().saturating_sub(nw) + 1;
        let rh = hg.height().saturating_sub(nh) + 1;

        let (t_prime, sum_t_prime, denom_t) = preprocess_template(&ng);
        let ii = integral_image::<_, u64>(&hg);
        let ii2 = integral_squared_image::<_, u64>(&hg);

        // 1パスで最大を発見（Vec 確保なし）。
        let mut best_x = 0u32;
        let mut best_y = 0u32;
        let mut best_r = f32::MIN;

        for y in 0..rh {
            for x in 0..rw {
                let r = ccoeff_at(&hg, &t_prime, sum_t_prime, denom_t, &ii, &ii2, x, y, nw, nh);
                if r > best_r {
                    best_r = r;
                    best_x = x;
                    best_y = y;
                }
            }
        }

        let confidence = MatchConfidence::new(best_r.max(0.0));
        if !confidence.exceeds_threshold(&self.threshold) {
            debug!(
                "CCOEFF best match below threshold: confidence {:.4} < {:.4}",
                confidence.0, self.threshold.0
            );
            return None;
        }

        let orig_x = best_x * self.downscale_factor;
        let orig_y = best_y * self.downscale_factor;
        Some(MatchResult {
            region: ScreenRegion::new(orig_x, orig_y, needle.width(), needle.height()),
            confidence,
        })
    }

    fn score_map(&self, haystack: &DynamicImage, needle: &DynamicImage) -> Option<GrayImage> {
        let (h, n) = self.downscale_images(haystack, needle)?;
        let hg = h.to_luma8();
        let ng = n.to_luma8();

        let nw = ng.width();
        let nh = ng.height();
        let rw = hg.width().saturating_sub(nw) + 1;
        let rh = hg.height().saturating_sub(nh) + 1;

        let (t_prime, sum_t_prime, denom_t) = preprocess_template(&ng);
        let ii = integral_image::<_, u64>(&hg);
        let ii2 = integral_squared_image::<_, u64>(&hg);

        let mut out = GrayImage::new(rw, rh);
        for y in 0..rh {
            for x in 0..rw {
                let r = ccoeff_at(&hg, &t_prime, sum_t_prime, denom_t, &ii, &ii2, x, y, nw, nh);
                let v = (r.clamp(0.0, 1.0) * 255.0) as u8;
                out.put_pixel(x, y, Luma([v]));
            }
        }
        Some(out)
    }

    fn match_all(&self, haystack: &DynamicImage, needle: &DynamicImage) -> Vec<MatchResult> {
        let (h, n) = match self.downscale_images(haystack, needle) {
            Some(imgs) => imgs,
            None => return vec![],
        };
        let hg = h.to_luma8();
        let ng = n.to_luma8();

        let nw = ng.width();
        let nh = ng.height();
        let rw = hg.width().saturating_sub(nw) + 1;
        let rh = hg.height().saturating_sub(nh) + 1;

        let (t_prime, sum_t_prime, denom_t) = preprocess_template(&ng);
        let ii = integral_image::<_, u64>(&hg);
        let ii2 = integral_squared_image::<_, u64>(&hg);

        let mut matches: Vec<MatchResult> = Vec::new();
        for y in 0..rh {
            for x in 0..rw {
                let r = ccoeff_at(&hg, &t_prime, sum_t_prime, denom_t, &ii, &ii2, x, y, nw, nh);
                let confidence = MatchConfidence::new(r.max(0.0));
                if confidence.exceeds_threshold(&self.threshold) {
                    let orig_x = x * self.downscale_factor;
                    let orig_y = y * self.downscale_factor;
                    matches.push(MatchResult {
                        region: ScreenRegion::new(orig_x, orig_y, needle.width(), needle.height()),
                        confidence,
                    });
                }
            }
        }

        matches.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        matches
    }
}

/// テンプレートの前処理: 平均除去テンプレート `T'` と `ΣT'`、`denomT` を返す。
///
/// `denomT == 0`（一様テンプレート）のとき `T'` は全 `0`（`denom_t` も `0`）となり、
/// [`ccoeff_at`] は全位置で `0` を返す。累算は f64 で行い桁落ちを防ぐ。
fn preprocess_template(ng: &GrayImage) -> (Vec<f32>, f64, f64) {
    let w = ng.width() as usize;
    let h = ng.height() as usize;
    let n = (w * h) as f64;

    // μT
    let mut sum_t: f64 = 0.0;
    for p in ng.pixels() {
        sum_t += p[0] as f64;
    }
    let mu_t = sum_t / n;

    // T' と ΣT'、Σ T'²
    let mut t_prime = Vec::with_capacity(w * h);
    let mut sum_t_prime: f64 = 0.0;
    let mut sum_t_prime_sq: f64 = 0.0;
    for p in ng.pixels() {
        let t = p[0] as f64 - mu_t;
        sum_t_prime += t;
        sum_t_prime_sq += t * t;
        t_prime.push(t as f32);
    }

    let denom_t = sum_t_prime_sq.sqrt();
    (t_prime, sum_t_prime, denom_t)
}

/// 1走査位置 `(x, y)` の相関係数 `r ∈ [-1, 1]` を計算する。
///
/// 分子は明示的に `Σ T'·(I - μI) = Σ T'·I - μI·ΣT'` で計算（`ΣT'` を f64 で渡し、
/// 数値誤差による位置バイアスを排除する）。窓平均 `μI` は積分図から O(1)。
/// `denom_t == 0` または `denomI ≈ 0` のとき `0` を返す。NaN/Inf も `0` に弾く。
#[allow(clippy::too_many_arguments)]
fn ccoeff_at(
    hg: &GrayImage,
    t_prime: &[f32],
    sum_t_prime: f64,
    denom_t: f64,
    ii: &IntegralImage,
    ii2: &IntegralImage,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
) -> f32 {
    // 一様テンプレート: CCOEFF 未定義 → 0
    if denom_t == 0.0 {
        return 0.0;
    }

    let n = (w as u64) * (h as u64);
    let nf = n as f64;

    // O(1) 窓和。sum_image_pixels は [left,right]*[top,bottom] 閉区間。
    let sum_i = sum_image_pixels(ii, x, y, x + w - 1, y + h - 1)[0] as f64;
    let sum_i2 = sum_image_pixels(ii2, x, y, x + w - 1, y + h - 1)[0] as f64;
    let mu_i = sum_i / nf;

    // denomI = sqrt(Σ I² - N·μI²)
    let var_i = sum_i2 - nf * mu_i * mu_i;
    if var_i <= 0.0 {
        return 0.0; // 一様窓: ゼロ割り回避
    }
    let denom_i = var_i.sqrt();

    // 分子: Σ T'·I - μI·ΣT'（f64 累算）
    let mut dot: f64 = 0.0;
    let tw = w as usize;
    for j in 0..h as usize {
        let yoff = y + j as u32;
        let trow = &t_prime[j * tw..(j + 1) * tw];
        for (i, tval) in trow.iter().enumerate() {
            let pix = hg.get_pixel(x + i as u32, yoff)[0] as f64;
            dot += *tval as f64 * pix;
        }
    }
    let numerator = dot - mu_i * sum_t_prime;

    let denom = denom_t * denom_i;
    if denom == 0.0 {
        return 0.0;
    }
    let r = numerator / denom;
    // NaN/Inf 防御（f32 計算では稀だが MAA inRange 相当の二重防御）
    if !r.is_finite() {
        return 0.0;
    }
    r as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::SseVisionEngine;
    use crate::matcher::TemplateMatcher;
    use image::{DynamicImage, GrayImage, Luma};

    /// `(x+y) mod 64` の勾配パターン（濃淡あり, denomT≠0 を保証）。
    fn gradient_needle(w: u32, h: u32) -> GrayImage {
        let mut img = GrayImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let v = ((x + y) % 64) as u8;
                img.put_pixel(x, y, Luma([v]));
            }
        }
        img
    }

    /// 照明不変性テスト用の高コントラストパターン。値域 0..=150 の広いダイナミックレンジで、
    /// denomT≠0 を保ちつつ一律シフトが SSE に大きな絶対差として現れるようにする。
    fn contrast_needle(w: u32, h: u32) -> GrayImage {
        let mut img = GrayImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                // (x*y) で変化する非一様パターンを 0..=150 に射影
                let v = (((x * y) % 150) + 10) as u8; // 10..=159
                img.put_pixel(x, y, Luma([v]));
            }
        }
        img
    }

    /// haystack 中央 (ox, oy) に needle を埋め込んだ画像。
    fn embed(haystack_w: u32, haystack_h: u32, needle: &GrayImage, ox: u32, oy: u32, bg: u8) -> GrayImage {
        let mut img = GrayImage::from_pixel(haystack_w, haystack_h, Luma([bg]));
        for y in 0..needle.height() {
            for x in 0..needle.width() {
                let p = needle.get_pixel(x, y)[0];
                img.put_pixel(ox + x, oy + y, Luma([p]));
            }
        }
        img
    }

    /// 全画素に定数を足して clamp[0,255]。
    fn shift_brightness(img: &GrayImage, delta: i32) -> GrayImage {
        let mut out = GrayImage::new(img.width(), img.height());
        for (x, y, p) in img.enumerate_pixels() {
            let v = (p[0] as i32 + delta).clamp(0, 255) as u8;
            out.put_pixel(x, y, Luma([v]));
        }
        out
    }

    fn luma_dyn(img: GrayImage) -> DynamicImage {
        DynamicImage::ImageLuma8(img)
    }

    #[test]
    fn ccoeff_exact_match_returns_near_one() {
        let needle = gradient_needle(20, 20);
        let haystack = embed(80, 80, &needle, 40, 40, 128);

        let engine = CcoeffVisionEngine::threshold_only(MatchConfidence(0.0));
        let m = engine
            .match_template(&luma_dyn(haystack), &luma_dyn(needle.clone()))
            .expect("should match");

        // 完全一致位置で CCOEFF は理論上 1.0
        assert!(
            m.confidence.0 > 0.999,
            "exact match confidence should be ~1.0, got {}",
            m.confidence.0
        );
        assert!(m.region.x >= 38 && m.region.x <= 42);
        assert!(m.region.y >= 38 && m.region.y <= 42);
    }

    #[test]
    fn ccoeff_score_map_dimensions() {
        let needle = gradient_needle(20, 20);
        let haystack = embed(100, 100, &needle, 40, 40, 128);

        let engine = CcoeffVisionEngine::threshold_only(MatchConfidence(0.0));
        let map = engine
            .score_map(&luma_dyn(haystack), &luma_dyn(needle))
            .expect("score map should exist");

        // threshold_only（ダウンスケール1）→ (100-20+1)^2
        assert_eq!(map.dimensions(), (81, 81));
    }

    #[test]
    fn ccoeff_template_too_large_returns_none() {
        let haystack = luma_dyn(GrayImage::from_pixel(10, 10, Luma([0])));
        let needle = luma_dyn(gradient_needle(20, 20));

        let engine = CcoeffVisionEngine::threshold_only(MatchConfidence(0.0));
        assert!(engine.match_template(&haystack, &needle).is_none());
        assert!(engine.score_map(&haystack, &needle).is_none());
    }

    #[test]
    fn ccoeff_uniform_template_returns_zero() {
        // 一様テンプレート: denomT=0 → score_map 全 0
        let needle = GrayImage::from_pixel(20, 20, Luma([100]));
        let haystack = embed(80, 80, &needle, 30, 30, 200);

        let engine = CcoeffVisionEngine::threshold_only(MatchConfidence(0.0));
        let map = engine
            .score_map(&luma_dyn(haystack), &luma_dyn(needle))
            .expect("score map should exist");
        for p in map.pixels() {
            assert_eq!(p[0], 0, "uniform template must yield all-zero score map");
        }
    }

    #[test]
    fn ccoeff_nan_filtered() {
        // 完全に一様な haystack（全窓が denomI=0）では score_map は全 0 になる。
        // ゼロ割りや NaN/Inf が混入せず、安全に 0 に弾かれることを確認する。
        let needle = gradient_needle(8, 8);
        let haystack = GrayImage::from_pixel(60, 60, Luma([150]));

        let engine = CcoeffVisionEngine::threshold_only(MatchConfidence(0.0));
        let map = engine
            .score_map(&luma_dyn(haystack.clone()), &luma_dyn(needle.clone()))
            .expect("score map should exist");
        for p in map.pixels() {
            assert_eq!(p[0], 0, "uniform-window positions must yield 0, not NaN/Inf garbage");
        }
        // match_all も一様窓では空（閾値0でも r=0 は 0.0 >= 0.0 で受理されるが、
        // CCOEFF は denomI=0 で 0 を返すので confidence=0。閾値0で受理される点を確認）。
        let all = engine.match_all(&luma_dyn(haystack), &luma_dyn(needle));
        for m in &all {
            assert_eq!(m.confidence.0, 0.0);
        }
    }

    #[test]
    fn ccoeff_match_template_finds_same_as_match_all() {
        let needle = gradient_needle(20, 20);
        let haystack = embed(100, 100, &needle, 40, 40, 128);

        let engine = CcoeffVisionEngine::threshold_only(MatchConfidence(0.0));
        let best = engine.match_template(&luma_dyn(haystack.clone()), &luma_dyn(needle.clone()));
        let all = engine.match_all(&luma_dyn(haystack), &luma_dyn(needle));

        assert!(best.is_some());
        assert!(!all.is_empty());
        assert!(
            (best.unwrap().confidence.0 - all[0].confidence.0).abs() < 0.001,
            "best and all[0] confidence must match"
        );
    }

    #[test]
    fn ccoeff_robust_to_brightness_shift_sse_degrades() {
        // 照明不変性テスト用の高コントラストパターン（0-150 の広いダイナミックレンジ）。
        // 広いレンジにより、一律シフトが SSE に大きな絶対差として現れる。
        let needle_base = contrast_needle(30, 30);
        // haystack 中央 (40,40) にベースを埋め込み。背景はベース平均に近くない値。
        let haystack = embed(120, 120, &needle_base, 40, 40, 10);
        // 照明シフト版テンプレート（+80）。ベース最大値 150 + 80 = 230 < 255 で飽和なし。
        let needle_shift = shift_brightness(&needle_base, 80);

        let haystack_dyn = luma_dyn(haystack);
        let needle_shift_dyn = luma_dyn(needle_shift);

        // SSE エンジンでシフト版テンプレを探す
        let sse = SseVisionEngine::new(TemplateMatcher::threshold_only(MatchConfidence(0.0)));
        let sse_res = sse
            .match_template(&haystack_dyn, &needle_shift_dyn)
            .expect("SSE position should resolve");
        let sse_conf = sse_res.confidence.0;

        // CCOEFF エンジンで同じく
        let cc = CcoeffVisionEngine::threshold_only(MatchConfidence(0.0));
        let cc_res = cc
            .match_template(&haystack_dyn, &needle_shift_dyn)
            .expect("CCOEFF should match");
        let cc_conf = cc_res.confidence.0;

        // 核心: SSE は大きく低下、CCOEFF は高いまま
        assert!(
            sse_conf < 0.6,
            "SSE should degrade under brightness shift, got {sse_conf}"
        );
        assert!(
            cc_conf > 0.95,
            "CCOEFF must stay near 1.0 under brightness shift, got {cc_conf}"
        );
        assert!(
            cc_conf > sse_conf,
            "CCOEFF must outperform SSE under illumination change (cc={cc_conf} sse={sse_conf})"
        );
    }

    #[test]
    fn ccoeff_location_matches_sse_on_exact_embed() {
        // 完全一致の場合、CCOEFF と SSE は同じ位置を指す
        let needle = gradient_needle(20, 20);
        let haystack = embed(100, 100, &needle, 40, 40, 128);
        let haystack_dyn = luma_dyn(haystack);
        let needle_dyn = luma_dyn(needle.clone());

        let sse = SseVisionEngine::new(TemplateMatcher::threshold_only(MatchConfidence(0.0)));
        let cc = CcoeffVisionEngine::threshold_only(MatchConfidence(0.0));

        let sse_m = sse.match_template(&haystack_dyn, &needle_dyn).expect("sse match");
        let cc_m = cc.match_template(&haystack_dyn, &needle_dyn).expect("ccoeff match");

        assert_eq!(sse_m.region.x, cc_m.region.x, "x position must agree");
        assert_eq!(sse_m.region.y, cc_m.region.y, "y position must agree");
    }
}
