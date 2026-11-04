use image::{DynamicImage, GrayImage, Luma};
use imageproc::template_matching::match_template;
use tracing::debug;

use anaden_core::{MatchConfidence, ScreenRegion};

#[derive(Debug, Clone)]
pub struct MatchResult {
    pub region: ScreenRegion,
    pub confidence: MatchConfidence,
}

pub struct TemplateMatcher {
    threshold: MatchConfidence,
    downscale_factor: u32,
}

impl TemplateMatcher {
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

    pub fn find_best_match(
        &self,
        haystack: &DynamicImage,
        needle: &DynamicImage,
    ) -> Option<MatchResult> {
        let (haystack_work, needle_work) = self.downscale_images(haystack, needle)?;

        let haystack_gray = haystack_work.to_luma8();
        let needle_gray = needle_work.to_luma8();

        let result = match_template(
            &haystack_gray,
            &needle_gray,
            imageproc::template_matching::MatchTemplateMethod::SumOfSquaredErrorsNormalized,
        );

        let mut best_x = 0u32;
        let mut best_y = 0u32;
        let mut best_sse = f32::MAX;

        for y in 0..result.height() {
            for x in 0..result.width() {
                let sse = result.get_pixel(x, y)[0];
                if sse < best_sse {
                    best_sse = sse;
                    best_x = x;
                    best_y = y;
                }
            }
        }

        let confidence = MatchConfidence::new(1.0 - best_sse);

        if !confidence.exceeds_threshold(&self.threshold) {
            debug!(
                "Best match below threshold: confidence {:.4} < {:.4}",
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

    pub fn find_matches(&self, haystack: &DynamicImage, needle: &DynamicImage) -> Vec<MatchResult> {
        let (haystack_work, needle_work) = match self.downscale_images(haystack, needle) {
            Some(imgs) => imgs,
            None => return vec![],
        };

        let haystack_gray = haystack_work.to_luma8();
        let needle_gray = needle_work.to_luma8();

        let result = match_template(
            &haystack_gray,
            &needle_gray,
            imageproc::template_matching::MatchTemplateMethod::SumOfSquaredErrorsNormalized,
        );

        let mut matches: Vec<MatchResult> = Vec::new();

        for y in 0..result.height() {
            for x in 0..result.width() {
                let sse_score = result.get_pixel(x, y)[0];
                let confidence = MatchConfidence::new(1.0 - sse_score);

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

    pub fn score_map(&self, haystack: &DynamicImage, needle: &DynamicImage) -> Option<GrayImage> {
        let (haystack_work, needle_work) = self.downscale_images(haystack, needle)?;

        let haystack_gray = haystack_work.to_luma8();
        let needle_gray = needle_work.to_luma8();

        let result = match_template(
            &haystack_gray,
            &needle_gray,
            imageproc::template_matching::MatchTemplateMethod::SumOfSquaredErrorsNormalized,
        );

        let (rw, rh) = (result.width(), result.height());
        let mut out = GrayImage::new(rw, rh);
        for y in 0..rh {
            for x in 0..rw {
                let sse = result.get_pixel(x, y)[0];
                let conf = (1.0 - sse).clamp(0.0, 1.0);
                out.put_pixel(x, y, Luma([(conf * 255.0) as u8]));
            }
        }
        Some(out)
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{GrayImage, ImageBuffer, Luma};

    fn solid_image(w: u32, h: u32, color: Luma<u8>) -> GrayImage {
        ImageBuffer::from_pixel(w, h, color)
    }

    #[test]
    fn exact_match_detection() {
        let mut bg = solid_image(100, 100, Luma([255]));
        for y in 40..60 {
            for x in 30..50 {
                bg.put_pixel(x, y, Luma([0]));
            }
        }
        let haystack = DynamicImage::ImageLuma8(bg);
        let needle_img = solid_image(20, 20, Luma([0]));
        let needle = DynamicImage::ImageLuma8(needle_img);

        let matcher = TemplateMatcher::threshold_only(MatchConfidence::new(0.5));
        let result = matcher.find_best_match(&haystack, &needle);

        assert!(result.is_some());
        let m = result.unwrap();
        assert!(m.region.x <= 35 && m.region.x >= 25);
        assert!(m.region.y <= 45 && m.region.y >= 35);
    }

    #[test]
    fn template_larger_than_haystack_returns_empty() {
        let haystack = DynamicImage::ImageLuma8(solid_image(10, 10, Luma([0])));
        let needle = DynamicImage::ImageLuma8(solid_image(20, 20, Luma([0])));

        let matcher = TemplateMatcher::threshold_only(MatchConfidence::new(0.5));
        let results = matcher.find_matches(&haystack, &needle);

        assert!(results.is_empty());
    }

    #[test]
    fn find_best_match_finds_same_as_find_matches() {
        let mut bg = solid_image(100, 100, Luma([255]));
        for y in 40..60 {
            for x in 30..50 {
                bg.put_pixel(x, y, Luma([0]));
            }
        }
        let haystack = DynamicImage::ImageLuma8(bg);
        let needle_img = solid_image(20, 20, Luma([0]));
        let needle = DynamicImage::ImageLuma8(needle_img);

        let matcher = TemplateMatcher::threshold_only(MatchConfidence::new(0.5));
        let best = matcher.find_best_match(&haystack, &needle);
        let all = matcher.find_matches(&haystack, &needle);

        assert!(best.is_some());
        assert!(!all.is_empty());
        assert!((best.unwrap().confidence.0 - all[0].confidence.0).abs() < 0.001);
    }

    #[test]
    fn no_match_returns_none() {
        let haystack = DynamicImage::ImageLuma8(solid_image(100, 100, Luma([255])));
        let needle = DynamicImage::ImageLuma8(solid_image(20, 20, Luma([0])));

        let matcher = TemplateMatcher::threshold_only(MatchConfidence::new(0.999));
        let result = matcher.find_best_match(&haystack, &needle);

        assert!(result.is_none());
    }
}
