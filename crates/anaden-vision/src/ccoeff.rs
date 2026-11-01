use image::{DynamicImage, GrayImage, Luma};
use imageproc::integral_image::{integral_image, integral_squared_image, sum_image_pixels};
use tracing::debug;

use anaden_core::{MatchConfidence, ScreenRegion};

use crate::engine::VisionEngine;
use crate::matcher::MatchResult;

type IntegralImage = image::ImageBuffer<Luma<u64>, Vec<u64>>;

pub struct CcoeffVisionEngine {
    threshold: MatchConfidence,
    downscale_factor: u32,
}

impl CcoeffVisionEngine {
    pub fn new(threshold: MatchConfidence, downscale_factor: u32) -> Self {
        let factor = if downscale_factor == 0 {
            1
        } else {
            downscale_factor
        };
        Self {
            threshold,
            downscale_factor: factor,
        }
    }

    pub fn with_defaults() -> Self {
        Self::new(MatchConfidence::DEFAULT_THRESHOLD, 2)
    }

    pub fn threshold_only(threshold: MatchConfidence) -> Self {
        Self::new(threshold, 1)
    }

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

    pub(crate) fn match_template_prepared(
        &self,
        haystack: &DynamicImage,
        prepared: &PreparedCcoeff,
    ) -> Option<MatchResult> {
        if self.downscale_factor > 1 {
            let needle = DynamicImage::ImageLuma8(prepared.luma.clone());
            return self.match_template(haystack, &needle);
        }
        let hg = haystack.to_luma8();
        let dims = prepared.luma.dimensions();
        self.scan_best_thresholded(
            &hg,
            &prepared.t_prime,
            prepared.sum_t_prime,
            prepared.denom_t,
            dims,
            dims,
        )
    }

    fn scan_best_thresholded(
        &self,
        hg: &GrayImage,
        t_prime: &[f32],
        sum_t_prime: f64,
        denom_t: f64,
        scan_dims: (u32, u32),
        report_dims: (u32, u32),
    ) -> Option<MatchResult> {
        let (scan_w, scan_h) = scan_dims;
        let (report_w, report_h) = report_dims;
        if scan_w > hg.width() || scan_h > hg.height() {
            debug!(
                "Template ({}x{}) larger than screen ({}x{}), skipping",
                scan_w,
                scan_h,
                hg.width(),
                hg.height()
            );
            return None;
        }
        let rw = hg.width().saturating_sub(scan_w) + 1;
        let rh = hg.height().saturating_sub(scan_h) + 1;

        let ii = integral_image::<_, u64>(hg);
        let ii2 = integral_squared_image::<_, u64>(hg);

        let mut best_x = 0u32;
        let mut best_y = 0u32;
        let mut best_r = f32::MIN;

        for y in 0..rh {
            for x in 0..rw {
                let r = ccoeff_at(
                    hg,
                    t_prime,
                    sum_t_prime,
                    denom_t,
                    &ii,
                    &ii2,
                    x,
                    y,
                    scan_w,
                    scan_h,
                );
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
            region: ScreenRegion::new(orig_x, orig_y, report_w, report_h),
            confidence,
        })
    }
}

struct CcoeffScan {
    hg: GrayImage,
    t_prime: Vec<f32>,
    sum_t_prime: f64,
    denom_t: f64,
    ii: image::ImageBuffer<Luma<u64>, Vec<u64>>,
    ii2: image::ImageBuffer<Luma<u64>, Vec<u64>>,
    nw: u32,
    nh: u32,
    rw: u32,
    rh: u32,
}

impl CcoeffVisionEngine {
    fn prepare_scan(&self, haystack: &DynamicImage, needle: &DynamicImage) -> Option<CcoeffScan> {
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
        Some(CcoeffScan {
            hg,
            t_prime,
            sum_t_prime,
            denom_t,
            ii,
            ii2,
            nw,
            nh,
            rw,
            rh,
        })
    }
}

impl VisionEngine for CcoeffVisionEngine {
    fn match_template(
        &self,
        haystack: &DynamicImage,
        needle: &DynamicImage,
    ) -> Option<MatchResult> {
        let (h, n) = self.downscale_images(haystack, needle)?;
        let hg = h.to_luma8();
        let ng = n.to_luma8();

        let (t_prime, sum_t_prime, denom_t) = preprocess_template(&ng);
        self.scan_best_thresholded(
            &hg,
            &t_prime,
            sum_t_prime,
            denom_t,
            (ng.width(), ng.height()),
            (needle.width(), needle.height()),
        )
    }

    fn score_map(&self, haystack: &DynamicImage, needle: &DynamicImage) -> Option<GrayImage> {
        let s = self.prepare_scan(haystack, needle)?;
        let mut out = GrayImage::new(s.rw, s.rh);
        for y in 0..s.rh {
            for x in 0..s.rw {
                let r = ccoeff_at(
                    &s.hg,
                    &s.t_prime,
                    s.sum_t_prime,
                    s.denom_t,
                    &s.ii,
                    &s.ii2,
                    x,
                    y,
                    s.nw,
                    s.nh,
                );
                let v = (r.clamp(0.0, 1.0) * 255.0) as u8;
                out.put_pixel(x, y, Luma([v]));
            }
        }
        Some(out)
    }

    fn match_all(&self, haystack: &DynamicImage, needle: &DynamicImage) -> Vec<MatchResult> {
        let Some(s) = self.prepare_scan(haystack, needle) else {
            return vec![];
        };
        let mut matches: Vec<MatchResult> = Vec::new();
        for y in 0..s.rh {
            for x in 0..s.rw {
                let r = ccoeff_at(
                    &s.hg,
                    &s.t_prime,
                    s.sum_t_prime,
                    s.denom_t,
                    &s.ii,
                    &s.ii2,
                    x,
                    y,
                    s.nw,
                    s.nh,
                );
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

#[derive(Debug, Clone)]
pub(crate) struct PreparedCcoeff {
    pub(crate) luma: GrayImage,
    pub(crate) t_prime: Vec<f32>,
    pub(crate) sum_t_prime: f64,
    pub(crate) denom_t: f64,
}

impl PreparedCcoeff {
    pub(crate) fn prepare(needle: &DynamicImage) -> Self {
        let luma = needle.to_luma8();
        let (t_prime, sum_t_prime, denom_t) = preprocess_template(&luma);
        Self {
            luma,
            t_prime,
            sum_t_prime,
            denom_t,
        }
    }
}

fn preprocess_template(ng: &GrayImage) -> (Vec<f32>, f64, f64) {
    let w = ng.width() as usize;
    let h = ng.height() as usize;
    let n = (w * h) as f64;

    let mut sum_t: f64 = 0.0;
    for p in ng.pixels() {
        sum_t += p[0] as f64;
    }
    let mu_t = sum_t / n;

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
    if denom_t == 0.0 {
        return 0.0;
    }

    let n = (w as u64) * (h as u64);
    let nf = n as f64;

    let sum_i = sum_image_pixels(ii, x, y, x + w - 1, y + h - 1)[0] as f64;
    let sum_i2 = sum_image_pixels(ii2, x, y, x + w - 1, y + h - 1)[0] as f64;
    let mu_i = sum_i / nf;

    let var_i = sum_i2 - nf * mu_i * mu_i;
    if var_i <= 0.0 {
        return 0.0;
    }
    let denom_i = var_i.sqrt();

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

    fn contrast_needle(w: u32, h: u32) -> GrayImage {
        let mut img = GrayImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let v = (((x * y) % 150) + 10) as u8;
                img.put_pixel(x, y, Luma([v]));
            }
        }
        img
    }

    fn embed(
        haystack_w: u32,
        haystack_h: u32,
        needle: &GrayImage,
        ox: u32,
        oy: u32,
        bg: u8,
    ) -> GrayImage {
        let mut img = GrayImage::from_pixel(haystack_w, haystack_h, Luma([bg]));
        for y in 0..needle.height() {
            for x in 0..needle.width() {
                let p = needle.get_pixel(x, y)[0];
                img.put_pixel(ox + x, oy + y, Luma([p]));
            }
        }
        img
    }

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
        let needle = gradient_needle(8, 8);
        let haystack = GrayImage::from_pixel(60, 60, Luma([150]));

        let engine = CcoeffVisionEngine::threshold_only(MatchConfidence(0.0));
        let map = engine
            .score_map(&luma_dyn(haystack.clone()), &luma_dyn(needle.clone()))
            .expect("score map should exist");
        for p in map.pixels() {
            assert_eq!(
                p[0], 0,
                "uniform-window positions must yield 0, not NaN/Inf garbage"
            );
        }
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
    fn ccoeff_match_template_prepared_matches_unprepared_result() {
        let needle = gradient_needle(20, 20);
        let haystack = embed(100, 100, &needle, 40, 40, 128);
        let needle_dyn = luma_dyn(needle);
        let haystack_dyn = luma_dyn(haystack);

        let engine = CcoeffVisionEngine::threshold_only(MatchConfidence(0.0));
        let plain = engine
            .match_template(&haystack_dyn, &needle_dyn)
            .expect("plain path should match");

        let prepared = PreparedCcoeff::prepare(&needle_dyn);
        let cached = engine
            .match_template_prepared(&haystack_dyn, &prepared)
            .expect("prepared path should match");

        assert_eq!(plain.region, cached.region, "region must be identical");
        assert!(
            (plain.confidence.0 - cached.confidence.0).abs() < 1e-6,
            "confidence must be identical: plain={} cached={}",
            plain.confidence.0,
            cached.confidence.0
        );
    }

    #[test]
    fn ccoeff_match_template_prepared_needle_too_large_returns_none() {
        let prepared = PreparedCcoeff::prepare(&luma_dyn(gradient_needle(20, 20)));
        let haystack = luma_dyn(GrayImage::from_pixel(10, 10, Luma([0])));
        let engine = CcoeffVisionEngine::threshold_only(MatchConfidence(0.0));
        assert!(
            engine
                .match_template_prepared(&haystack, &prepared)
                .is_none(),
            "oversized prepared needle must yield None (no panic, no underflow)"
        );
    }

    #[test]
    fn ccoeff_robust_to_brightness_shift_sse_degrades() {
        let needle_base = contrast_needle(30, 30);
        let haystack = embed(120, 120, &needle_base, 40, 40, 10);
        let needle_shift = shift_brightness(&needle_base, 80);

        let haystack_dyn = luma_dyn(haystack);
        let needle_shift_dyn = luma_dyn(needle_shift);

        let sse = SseVisionEngine::new(TemplateMatcher::threshold_only(MatchConfidence(0.0)));
        let sse_res = sse
            .match_template(&haystack_dyn, &needle_shift_dyn)
            .expect("SSE position should resolve");
        let sse_conf = sse_res.confidence.0;

        let cc = CcoeffVisionEngine::threshold_only(MatchConfidence(0.0));
        let cc_res = cc
            .match_template(&haystack_dyn, &needle_shift_dyn)
            .expect("CCOEFF should match");
        let cc_conf = cc_res.confidence.0;

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
        let needle = gradient_needle(20, 20);
        let haystack = embed(100, 100, &needle, 40, 40, 128);
        let haystack_dyn = luma_dyn(haystack);
        let needle_dyn = luma_dyn(needle.clone());

        let sse = SseVisionEngine::new(TemplateMatcher::threshold_only(MatchConfidence(0.0)));
        let cc = CcoeffVisionEngine::threshold_only(MatchConfidence(0.0));

        let sse_m = sse
            .match_template(&haystack_dyn, &needle_dyn)
            .expect("sse match");
        let cc_m = cc
            .match_template(&haystack_dyn, &needle_dyn)
            .expect("ccoeff match");

        assert_eq!(sse_m.region.x, cc_m.region.x, "x position must agree");
        assert_eq!(sse_m.region.y, cc_m.region.y, "y position must agree");
    }
}
