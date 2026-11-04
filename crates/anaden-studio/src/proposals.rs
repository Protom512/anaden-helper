use anaden_core::ScreenRegion;
use anaden_vision::VisionEngine;
use image::DynamicImage;

const MIN_PEAKINESS: f32 = 0.30;
const MIN_ABS_CONF: u8 = 204;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Proposal {
    pub roi: ScreenRegion,
    pub score: f32,
}

struct Peakiness {
    value: f32,
    #[allow(dead_code)]
    peak: (u32, u32),
    max_confidence: u8,
}

fn peakiness_and_peak(map: &image::GrayImage) -> Peakiness {
    let mut values: Vec<u8> = Vec::with_capacity((map.width() as usize) * (map.height() as usize));
    let mut peak = (0u32, 0u32);
    let mut max_v: u8 = 0;
    for y in 0..map.height() {
        for x in 0..map.width() {
            let v = map.get_pixel(x, y)[0];
            values.push(v);
            if v > max_v {
                max_v = v;
                peak = (x, y);
            }
        }
    }
    if values.is_empty() {
        return Peakiness {
            value: 0.0,
            peak: (0, 0),
            max_confidence: 0,
        };
    }
    values.sort_unstable();
    let median = values[values.len() / 2];
    let max = *values.last().unwrap();
    let value = (max as f32 - median as f32) / 255.0;
    Peakiness {
        value,
        peak,
        max_confidence: max,
    }
}

pub fn propose(
    engine: &dyn VisionEngine,
    screenshot: &DynamicImage,
    tile_w: u32,
    tile_h: u32,
    step: u32,
    max_n: usize,
) -> Vec<Proposal> {
    let step = step.max(1);
    let (w, h) = (screenshot.width(), screenshot.height());
    if tile_w == 0 || tile_h == 0 || tile_w > w || tile_h > h {
        return vec![];
    }

    let mut out: Vec<Proposal> = Vec::new();

    let mut y = 0u32;
    while y + tile_h <= h {
        let mut x = 0u32;
        while x + tile_w <= w {
            let needle = screenshot.crop_imm(x, y, tile_w, tile_h);
            let Some(map) = engine.score_map(screenshot, &needle) else {
                x += step;
                continue;
            };
            let pk = peakiness_and_peak(&map);
            if pk.max_confidence < MIN_ABS_CONF {
                x += step;
                continue;
            }
            if pk.value < MIN_PEAKINESS {
                x += step;
                continue;
            }
            out.push(Proposal {
                roi: ScreenRegion::new(x, y, tile_w, tile_h),
                score: pk.value,
            });
            x += step;
        }
        y += step;
    }

    out.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    out.truncate(max_n);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use anaden_core::MatchConfidence;
    use anaden_vision::{SseVisionEngine, TemplateMatcher};
    use image::{DynamicImage, GrayImage, Luma};

    fn engine() -> SseVisionEngine {
        SseVisionEngine::new(TemplateMatcher::threshold_only(MatchConfidence::new(0.0)))
    }

    fn haystack_with_black_square() -> DynamicImage {
        let mut img = GrayImage::from_pixel(120, 120, Luma([255]));
        for y in 40..60 {
            for x in 40..60 {
                img.put_pixel(x, y, Luma([0]));
            }
        }
        DynamicImage::ImageLuma8(img)
    }

    fn uniform_gray() -> DynamicImage {
        DynamicImage::ImageLuma8(GrayImage::from_pixel(120, 120, Luma([128])))
    }

    #[test]
    fn peakiness_and_peak_ピーク画像_高尖り度() {
        let haystack = haystack_with_black_square();
        let needle = haystack.crop_imm(40, 40, 20, 20);
        let map = engine()
            .score_map(&haystack, &needle)
            .expect("score_map should exist");
        let pk = peakiness_and_peak(&map);

        assert!(
            pk.value > 0.5,
            "value should be high for sharp peak, got {}",
            pk.value
        );
        assert!(
            pk.max_confidence >= 250,
            "max_confidence should be ~255, got {}",
            pk.max_confidence
        );
        assert!(
            pk.peak.0 <= 45 && pk.peak.0 >= 35,
            "peak.x near 40, got {}",
            pk.peak.0
        );
        assert!(
            pk.peak.1 <= 45 && pk.peak.1 >= 35,
            "peak.y near 40, got {}",
            pk.peak.1
        );
    }

    #[test]
    fn peakiness_and_peak_一様画像_低尖り度() {
        let haystack = uniform_gray();
        let needle = haystack.crop_imm(0, 0, 20, 20);
        let map = engine()
            .score_map(&haystack, &needle)
            .expect("score_map should exist");
        let pk = peakiness_and_peak(&map);

        assert!(
            pk.value < 0.1,
            "value should be ~0 for uniform image, got {}",
            pk.value
        );
    }

    #[test]
    fn propose_鋭い特徴画像_候補を返す() {
        let haystack = haystack_with_black_square();
        let ps = propose(&engine(), &haystack, 20, 20, 20, 5);

        assert!(!ps.is_empty(), "should return proposals");

        let top = &ps[0];
        assert!(
            top.roi.contains(50, 50),
            "top roi should contain square center (50,50), got {:?}",
            top.roi
        );
        assert_eq!(top.roi.width, 20);
        assert_eq!(top.roi.height, 20);

        for w in ps.windows(2) {
            assert!(
                w[0].score >= w[1].score,
                "proposals must be score-descending: {} < {}",
                w[0].score,
                w[1].score
            );
        }
    }

    #[test]
    fn propose_一様画像_候補なし() {
        let haystack = uniform_gray();
        let ps = propose(&engine(), &haystack, 20, 20, 20, 5);
        assert!(
            ps.is_empty(),
            "uniform image should yield no proposals, got {}",
            ps.len()
        );
    }

    #[test]
    fn propose_max_nで打ち切り() {
        let haystack = haystack_with_black_square();
        let ps = propose(&engine(), &haystack, 20, 20, 20, 1);
        assert_eq!(ps.len(), 1, "max_n=1 should truncate to 1");

        let ps5 = propose(&engine(), &haystack, 20, 20, 20, 5);
        assert!(!ps5.is_empty());
        assert_eq!(ps[0].roi, ps5[0].roi);
    }

    #[test]
    fn propose_タイル大きすぎ_空() {
        let haystack = haystack_with_black_square();
        let ps = propose(&engine(), &haystack, 200, 200, 20, 5);
        assert!(
            ps.is_empty(),
            "tile larger than image should return empty, got {}",
            ps.len()
        );
    }

    #[test]
    fn proposalのコピーと比較可能性() {
        fn assert_copy_partial_eq<T: Copy + PartialEq>() {}
        assert_copy_partial_eq::<Proposal>();

        let a = Proposal {
            roi: ScreenRegion::new(1, 2, 3, 4),
            score: 0.5,
        };
        let b = a;
        assert_eq!(a, b);
    }
}
