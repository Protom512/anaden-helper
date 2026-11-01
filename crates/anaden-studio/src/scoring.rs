use std::sync::Arc;

use anaden_vision::VisionEngine;
use image::DynamicImage;

#[derive(Debug, Clone)]
pub struct Discrimination {
    pub own_min: f32,
    pub other_max: f32,
    pub own_scores: Vec<f32>,
    pub other_scores: Vec<f32>,
}

impl Discrimination {
    pub fn margin(&self) -> f32 {
        self.own_min - self.other_max
    }
}

pub fn discrimination(
    engine: &dyn VisionEngine,
    roi_image: &DynamicImage,
    positives: &[Arc<DynamicImage>],
    negatives: &[Arc<DynamicImage>],
) -> Discrimination {
    let own_scores: Vec<f32> = positives
        .iter()
        .map(|r| best_confidence(engine, r, roi_image))
        .collect();
    let other_scores: Vec<f32> = negatives
        .iter()
        .map(|r| best_confidence(engine, r, roi_image))
        .collect();

    let own_min = own_scores.iter().copied().fold(f32::MAX, f32::min);
    let other_max = other_scores.iter().copied().fold(0.0f32, f32::max);

    Discrimination {
        own_min,
        other_max,
        own_scores,
        other_scores,
    }
}

fn best_confidence(
    engine: &dyn VisionEngine,
    haystack: &DynamicImage,
    needle: &DynamicImage,
) -> f32 {
    engine
        .match_template(haystack, needle)
        .map(|m| m.confidence.0)
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use anaden_core::MatchConfidence;
    use anaden_vision::{SseVisionEngine, TemplateMatcher};
    use image::{DynamicImage, GrayImage, Luma};

    fn screen_with_square() -> DynamicImage {
        let mut img = GrayImage::from_pixel(100, 100, Luma([255]));
        for y in 40..60 {
            for x in 30..50 {
                img.put_pixel(x, y, Luma([0]));
            }
        }
        DynamicImage::ImageLuma8(img)
    }

    fn white_screen() -> DynamicImage {
        DynamicImage::ImageLuma8(GrayImage::from_pixel(100, 100, Luma([255])))
    }

    fn black_roi() -> DynamicImage {
        DynamicImage::ImageLuma8(GrayImage::from_pixel(20, 20, Luma([0])))
    }

    fn test_engine() -> SseVisionEngine {
        SseVisionEngine::new(TemplateMatcher::threshold_only(MatchConfidence::new(0.0)))
    }

    #[test]
    fn black_roi_distinguishes_from_white() {
        let engine = test_engine();
        let roi = black_roi();
        let positives = vec![Arc::new(screen_with_square())];
        let negatives = vec![Arc::new(white_screen())];

        let d = discrimination(&engine, &roi, &positives, &negatives);

        assert!(d.own_min > 0.9, "own_min should be high, got {}", d.own_min);
        assert!(
            d.other_max < 0.5,
            "other_max should be low, got {}",
            d.other_max
        );
        assert!(d.margin() > 0.4);
    }

    #[test]
    fn margin_is_negative_when_pattern_present_in_both() {
        let engine = test_engine();
        let roi = black_roi();
        let positives = vec![Arc::new(screen_with_square())];
        let negatives = vec![Arc::new(screen_with_square())];

        let d = discrimination(&engine, &roi, &positives, &negatives);
        assert!(
            d.margin().abs() < 0.2,
            "margin should be ~0 when pattern in both, got {}",
            d.margin()
        );
    }

    #[test]
    fn empty_reference_sets_yield_extreme_bounds() {
        let engine = test_engine();
        let roi = black_roi();
        let d = discrimination(&engine, &roi, &[], &[]);
        assert_eq!(d.own_min, f32::MAX);
        assert_eq!(d.other_max, 0.0);
    }
}
