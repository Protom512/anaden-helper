use image::{DynamicImage, GrayImage};
use tracing::{debug, info, warn};

use crate::matcher::TemplateMatcher;

const SAME_GROUP_THRESHOLD: f32 = 0.95;

const DIFF_GROUP_THRESHOLD: f32 = 0.80;

const GROUP_COMPARE_DOWNSCALE: u32 = 16;

const MIN_GROUP_SIZE: usize = 3;

const TILE_SIZE: u32 = 150;

const TILE_STEP: u32 = 75;

const STABLE_VARIANCE_THRESHOLD: f32 = 15.0;

const SENSITIVITY_THRESHOLD: f32 = 0.90;

const SPECIFICITY_THRESHOLD: f32 = 0.70;

const MAX_TEMPLATES_PER_GROUP: usize = 5;

#[derive(Debug)]
pub struct ScreenGroup {
    pub captures: Vec<DynamicImage>,
    pub index: usize,
}

#[derive(Debug)]
pub struct TileCandidate {
    pub image: DynamicImage,
    pub position: (u32, u32),
    pub stability: f32,
    pub edge_density: f32,
}

#[derive(Debug)]
pub struct VerifyResult {
    pub image: DynamicImage,
    pub position: (u32, u32),
    pub own_best_confidence: f32,
    pub other_worst_confidence: f32,
    pub passed: bool,
}

pub fn compute_similarity(img1: &DynamicImage, img2: &DynamicImage) -> f32 {
    let w = (img1.width().max(img2.width()) / GROUP_COMPARE_DOWNSCALE).max(1);
    let h = (img1.height().max(img2.height()) / GROUP_COMPARE_DOWNSCALE).max(1);

    let g1 = img1
        .resize_exact(w, h, image::imageops::FilterType::Triangle)
        .to_luma8();
    let g2 = img2
        .resize_exact(w, h, image::imageops::FilterType::Triangle)
        .to_luma8();

    let total_pixels = (w * h) as f32;
    if total_pixels == 0.0 {
        return 0.0;
    }

    let mut sum_abs_diff: f32 = 0.0;
    for y in 0..h {
        for x in 0..w {
            let v1 = g1.get_pixel(x, y)[0] as f32;
            let v2 = g2.get_pixel(x, y)[0] as f32;
            sum_abs_diff += (v1 - v2).abs();
        }
    }

    let mae = sum_abs_diff / total_pixels;
    1.0 - (mae / 255.0)
}

pub fn group_captures(captures: Vec<DynamicImage>) -> Vec<ScreenGroup> {
    if captures.is_empty() {
        return vec![];
    }

    let mut groups: Vec<ScreenGroup> = vec![ScreenGroup {
        captures: vec![captures[0].clone()],
        index: 0,
    }];

    for img in captures.iter().skip(1) {
        let last_group = groups.last().unwrap();
        let last_img = last_group.captures.last().unwrap();
        let sim = compute_similarity(last_img, img);

        if sim >= SAME_GROUP_THRESHOLD {
            groups.last_mut().unwrap().captures.push(img.clone());
        } else if sim < DIFF_GROUP_THRESHOLD {
            let new_index = groups.len();
            groups.push(ScreenGroup {
                captures: vec![img.clone()],
                index: new_index,
            });
        } else {
            if groups.last().unwrap().captures.len() <= 1 {
                groups.last_mut().unwrap().captures.clear();
                groups.last_mut().unwrap().captures.push(img.clone());
            } else {
                let new_index = groups.len();
                groups.push(ScreenGroup {
                    captures: vec![img.clone()],
                    index: new_index,
                });
            }
        }
    }

    for (i, g) in groups.iter_mut().enumerate() {
        g.index = i;
    }

    info!(
        "Grouped {} captures into {} groups (sizes: {})",
        captures.len(),
        groups.len(),
        groups
            .iter()
            .map(|g| g.captures.len().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    );

    groups
}

pub fn extract_stable_tiles(group: &ScreenGroup) -> Vec<TileCandidate> {
    if group.captures.len() < MIN_GROUP_SIZE {
        debug!(
            "Group {} has {} captures (need {}), skipping extraction",
            group.index,
            group.captures.len(),
            MIN_GROUP_SIZE
        );
        return vec![];
    }

    let ref_img = &group.captures[0];
    let (img_w, img_h) = (ref_img.width(), ref_img.height());

    let grays: Vec<GrayImage> = group.captures.iter().map(|img| img.to_luma8()).collect();

    let mut candidates: Vec<TileCandidate> = Vec::new();

    let mut y = 0u32;
    while y + TILE_SIZE <= img_h {
        let mut x = 0u32;
        while x + TILE_SIZE <= img_w {
            let (stability, edge_density) = compute_tile_metrics(&grays, x, y, TILE_SIZE);

            if stability > 0.0 {
                let tile_img = ref_img.crop_imm(x, y, TILE_SIZE, TILE_SIZE);
                candidates.push(TileCandidate {
                    image: tile_img,
                    position: (x, y),
                    stability,
                    edge_density,
                });
            }

            x += TILE_STEP;
        }
        y += TILE_STEP;
    }

    candidates.sort_by(|a, b| {
        b.edge_density
            .partial_cmp(&a.edge_density)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    debug!(
        "Group {}: {} tile candidates extracted",
        group.index,
        candidates.len()
    );

    candidates
}

struct SensitivityCheck {
    min_conf: f32,
    all_pass: bool,
}

fn check_sensitivity(
    candidate: &TileCandidate,
    own_group: &ScreenGroup,
    matcher: &TemplateMatcher,
) -> SensitivityCheck {
    let mut min_conf = f32::MAX;
    for capture in &own_group.captures {
        let Some(m) = matcher.find_best_match(capture, &candidate.image) else {
            return SensitivityCheck {
                min_conf,
                all_pass: false,
            };
        };
        if m.confidence.0 < SENSITIVITY_THRESHOLD {
            debug!(
                "Candidate ({}, {}) sensitivity fail: confidence {:.3} < {:.3}",
                candidate.position.0, candidate.position.1, m.confidence.0, SENSITIVITY_THRESHOLD
            );
            return SensitivityCheck {
                min_conf,
                all_pass: false,
            };
        }
        min_conf = min_conf.min(m.confidence.0);
    }
    SensitivityCheck {
        min_conf,
        all_pass: true,
    }
}

struct SpecificityCheck {
    max_conf: f32,
    all_pass: bool,
}

fn check_specificity(
    candidate: &TileCandidate,
    other_groups: &[&ScreenGroup],
    matcher: &TemplateMatcher,
) -> SpecificityCheck {
    let mut max_conf = 0.0f32;
    for other_group in other_groups {
        let representative = &other_group.captures[0];
        if let Some(m) = matcher.find_best_match(representative, &candidate.image) {
            max_conf = max_conf.max(m.confidence.0);
            if m.confidence.0 >= SPECIFICITY_THRESHOLD {
                debug!(
                    "Candidate ({}, {}) specificity fail: confidence {:.3} >= {:.3} on group {}",
                    candidate.position.0,
                    candidate.position.1,
                    m.confidence.0,
                    SPECIFICITY_THRESHOLD,
                    other_group.index
                );
                return SpecificityCheck {
                    max_conf,
                    all_pass: false,
                };
            }
        }
    }
    SpecificityCheck {
        max_conf,
        all_pass: true,
    }
}

pub fn verify_templates(
    candidates: Vec<TileCandidate>,
    own_group: &ScreenGroup,
    other_groups: &[&ScreenGroup],
    matcher: &TemplateMatcher,
) -> Vec<VerifyResult> {
    let mut verified = Vec::new();
    let mut accepted_count = 0;

    for candidate in candidates {
        if accepted_count >= MAX_TEMPLATES_PER_GROUP {
            break;
        }

        let sensitivity = check_sensitivity(&candidate, own_group, matcher);
        if !sensitivity.all_pass {
            continue;
        }

        let specificity = check_specificity(&candidate, other_groups, matcher);
        let passed = specificity.all_pass;
        if passed {
            accepted_count += 1;
            info!(
                "✅ Template verified: ({}, {}) own_min={:.3} other_max={:.3}",
                candidate.position.0,
                candidate.position.1,
                sensitivity.min_conf,
                specificity.max_conf
            );
        }

        verified.push(VerifyResult {
            image: candidate.image,
            position: candidate.position,
            own_best_confidence: sensitivity.min_conf,
            other_worst_confidence: specificity.max_conf,
            passed,
        });
    }

    let passed_count = verified.iter().filter(|v| v.passed).count();
    info!(
        "Group {}: {}/{} candidates passed verification",
        own_group.index,
        passed_count,
        verified.len()
    );

    verified
}

pub fn collect_templates(groups: &[ScreenGroup]) -> Vec<(usize, Vec<VerifyResult>)> {
    let matcher = TemplateMatcher::with_defaults();
    let mut results = Vec::new();

    for (i, group) in groups.iter().enumerate() {
        if group.captures.len() < MIN_GROUP_SIZE {
            info!(
                "Group {}: {} captures (< {}), skipped",
                group.index,
                group.captures.len(),
                MIN_GROUP_SIZE
            );
            continue;
        }

        info!(
            "Processing group {} ({} captures)...",
            group.index,
            group.captures.len()
        );

        let candidates = extract_stable_tiles(group);

        if candidates.is_empty() {
            warn!("Group {}: no stable tiles found", group.index);
            continue;
        }

        let other_groups: Vec<&ScreenGroup> = groups
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != i)
            .map(|(_, g)| g)
            .collect();

        let verified = verify_templates(candidates, group, &other_groups, &matcher);
        let passed: Vec<VerifyResult> = verified.into_iter().filter(|v| v.passed).collect();

        if !passed.is_empty() {
            results.push((group.index, passed));
        }
    }

    results
}

fn compute_tile_metrics(grays: &[GrayImage], x: u32, y: u32, size: u32) -> (f32, f32) {
    let n = grays.len() as f32;
    if n < MIN_GROUP_SIZE as f32 {
        return (0.0, 0.0);
    }

    let mut total_variance: f32 = 0.0;
    let mut pixel_count: u32 = 0;

    let mut edge_sum: f32 = 0.0;
    let ref_gray = &grays[0];

    for py in 0..size {
        for px in 0..size {
            let gx = x + px;
            let gy = y + py;

            let mut sum: f32 = 0.0;
            let mut sum_sq: f32 = 0.0;
            for gray in grays {
                let v = gray.get_pixel(gx, gy)[0] as f32;
                sum += v;
                sum_sq += v * v;
            }
            let mean = sum / n;
            let variance = (sum_sq / n) - (mean * mean);
            total_variance += variance;

            if px > 0 && py > 0 {
                let curr = ref_gray.get_pixel(gx, gy)[0] as f32;
                let left = ref_gray.get_pixel(gx - 1, gy)[0] as f32;
                let up = ref_gray.get_pixel(gx, gy - 1)[0] as f32;
                edge_sum += ((curr - left).abs() + (curr - up).abs()) / 2.0;
            }

            pixel_count += 1;
        }
    }

    let avg_variance = if pixel_count > 0 {
        total_variance / pixel_count as f32
    } else {
        f32::MAX
    };

    let stability = if avg_variance < STABLE_VARIANCE_THRESHOLD {
        1.0 / (1.0 + avg_variance)
    } else {
        0.0
    };

    let edge_density = if pixel_count > 0 {
        edge_sum / pixel_count as f32 / 255.0
    } else {
        0.0
    };

    (stability, edge_density)
}

#[cfg(test)]
mod tests {
    use super::*;
    use anaden_core::MatchConfidence;
    use image::{DynamicImage, RgbImage};

    fn solid_rgb_image(w: u32, h: u32, r: u8, g: u8, b: u8) -> DynamicImage {
        DynamicImage::ImageRgb8(RgbImage::from_pixel(w, h, image::Rgb([r, g, b])))
    }

    fn white_image(w: u32, h: u32) -> DynamicImage {
        solid_rgb_image(w, h, 255, 255, 255)
    }

    fn black_image(w: u32, h: u32) -> DynamicImage {
        solid_rgb_image(w, h, 0, 0, 0)
    }

    #[test]
    fn similarity_identical_images() {
        let img = white_image(600, 270);
        let sim = compute_similarity(&img, &img);
        assert!(
            sim > 0.99,
            "Identical images should have similarity > 0.99, got {}",
            sim
        );
    }

    #[test]
    fn similarity_different_images() {
        let white = white_image(600, 270);
        let black = black_image(600, 270);
        let sim = compute_similarity(&white, &black);
        assert!(
            sim < 0.1,
            "Black vs white should have similarity < 0.1, got {}",
            sim
        );
    }

    #[test]
    fn similarity_similar_images() {
        let img1 = solid_rgb_image(600, 270, 128, 128, 128);
        let img2 = solid_rgb_image(600, 270, 130, 130, 130);
        let sim = compute_similarity(&img1, &img2);
        assert!(
            sim > 0.95,
            "Very similar images should have similarity > 0.95, got {}",
            sim
        );
    }

    #[test]
    fn grouping_same_images_become_one_group() {
        let captures = vec![
            white_image(600, 270),
            white_image(600, 270),
            white_image(600, 270),
            white_image(600, 270),
        ];
        let groups = group_captures(captures);
        assert_eq!(groups.len(), 1, "All identical images should form 1 group");
        assert_eq!(groups[0].captures.len(), 4);
    }

    #[test]
    fn grouping_different_images_become_separate_groups() {
        let captures = vec![
            white_image(600, 270),
            white_image(600, 270),
            white_image(600, 270),
            black_image(600, 270),
            black_image(600, 270),
            black_image(600, 270),
        ];
        let groups = group_captures(captures);
        assert!(
            groups.len() >= 2,
            "White and black images should form at least 2 groups, got {}",
            groups.len()
        );
    }

    #[test]
    fn extract_stable_tiles_from_uniform_group() {
        let mut captures = Vec::new();
        for _ in 0..5 {
            captures.push(white_image(600, 270));
        }
        let group = ScreenGroup { captures, index: 0 };
        let _tiles = extract_stable_tiles(&group);
    }

    #[test]
    fn extract_stable_tiles_with_content() {
        let mut captures = Vec::new();
        for _ in 0..5 {
            let mut img = RgbImage::new(300, 200);
            for y in 10..60 {
                for x in 10..60 {
                    img.put_pixel(x, y, image::Rgb([0, 0, 0]));
                }
            }
            captures.push(DynamicImage::ImageRgb8(img));
        }
        let group = ScreenGroup { captures, index: 0 };
        let tiles = extract_stable_tiles(&group);
        assert!(
            !tiles.is_empty(),
            "Group with stable content should produce tile candidates"
        );
    }

    #[test]
    fn verify_rejects_template_matching_other_group() {
        let group_a = ScreenGroup {
            captures: vec![
                black_image(300, 200),
                black_image(300, 200),
                black_image(300, 200),
            ],
            index: 0,
        };
        let group_b = ScreenGroup {
            captures: vec![
                white_image(300, 200),
                white_image(300, 200),
                white_image(300, 200),
            ],
            index: 1,
        };

        let matcher = TemplateMatcher::threshold_only(MatchConfidence::new(0.5));

        let candidate = TileCandidate {
            image: black_image(50, 50),
            position: (0, 0),
            stability: 1.0,
            edge_density: 0.5,
        };

        let results = verify_templates(vec![candidate], &group_a, &[&group_b], &matcher);

        assert_eq!(results.len(), 1, "Should have 1 result");
        assert!(
            results[0].passed,
            "Black tile should pass on black group, not match white group"
        );
    }
}
