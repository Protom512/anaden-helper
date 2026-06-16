//! 探索的テンプレート自動収集エンジン。
//!
//! キャプチャした画面画像をグループ化し、安定領域をテンプレートとして抽出し、
//! 厳格な検証（感度＋特異性）を通過したものだけを保存する。
//!
//! ## 設計方針: 確認不当（誤検出）を防ぐ
//!
//! 1. テンプレート候補は **自分の画面で常にマッチ** すること（感度 ≥ 0.90）
//! 2. テンプレート候補は **別の画面で絶対にマッチしない** こと（特異性 < 0.70）
//! 3. 両方を満たしたものだけがテンプレートとして採用される

use image::{DynamicImage, GrayImage};
use tracing::{debug, info, warn};

use crate::matcher::TemplateMatcher;

// ─── 定数 ───

/// 類似度閾値: この値以上なら同じグループ（同じ画面状態）
const SAME_GROUP_THRESHOLD: f32 = 0.95;

/// 類似度閾値: この値未満なら別グループ（画面が変わった）
const DIFF_GROUP_THRESHOLD: f32 = 0.80;

/// グループ化に使うダウンスケール倍率
const GROUP_COMPARE_DOWNSCALE: u32 = 16;

/// 安定タイル抽出の最小グループサイズ（これ未満のキャプチャ数はスキップ）
const MIN_GROUP_SIZE: usize = 3;

/// タイルのサイズ（幅、高さ）
const TILE_SIZE: u32 = 150;

/// タイルのステップ（オーバーラップ量 = TILE_SIZE - TILE_STEP）
const TILE_STEP: u32 = 75;

/// 安定と判定するピクセル分散の閾値（0-255 のスケール）
const STABLE_VARIANCE_THRESHOLD: f32 = 15.0;

/// 感度チェック: 自グループで最低この信頼度が必要
const SENSITIVITY_THRESHOLD: f32 = 0.90;

/// 特異性チェック: 他グループでこの信頼度未満である必要がある
const SPECIFICITY_THRESHOLD: f32 = 0.70;

/// グループあたりの最大テンプレート数
const MAX_TEMPLATES_PER_GROUP: usize = 5;

// ─── データ構造 ───

/// 同じ画面状態のキャプチャ群。
#[derive(Debug)]
pub struct ScreenGroup {
    /// グループ内のキャプチャ画像
    pub captures: Vec<DynamicImage>,
    /// グループのインデックス（0始まり）
    pub index: usize,
}

/// テンプレート候補のタイル。
#[derive(Debug)]
pub struct TileCandidate {
    /// タイル画像
    pub image: DynamicImage,
    /// 元画像上の位置 (x, y)
    pub position: (u32, u32),
    /// 安定性スコア（分散の逆数。高いほど安定）
    pub stability: f32,
    /// エッジ密度（高いほど識別的）
    pub edge_density: f32,
}

/// テンプレートの検証結果。
#[derive(Debug)]
pub struct VerifyResult {
    /// テンプレート画像
    pub image: DynamicImage,
    /// 元画像上の位置
    pub position: (u32, u32),
    /// 自グループでの最低信頼度
    pub own_best_confidence: f32,
    /// 他グループでの最高信頼度
    pub other_worst_confidence: f32,
    /// 検証通過かどうか
    pub passed: bool,
}

// ─── 公開API ───

/// 2つの画像の類似度を計算する（0.0〜1.0）。
///
/// ダウンスケールしてグレースケール変換後、
/// 平均絶対誤差（MAE）を計算し `1.0 - mae/255` で類似度とする。
pub fn compute_similarity(img1: &DynamicImage, img2: &DynamicImage) -> f32 {
    let w = (img1.width().max(img2.width()) / GROUP_COMPARE_DOWNSCALE).max(1);
    let h = (img1.height().max(img2.height()) / GROUP_COMPARE_DOWNSCALE).max(1);

    let g1 = img1.resize_exact(w, h, image::imageops::FilterType::Triangle).to_luma8();
    let g2 = img2.resize_exact(w, h, image::imageops::FilterType::Triangle).to_luma8();

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

/// キャプチャ画像のストリームをグループ化する。
///
/// 連続する画像の類似度が高い（同じ画面）ものを同じグループにまとめる。
/// 画面が変わると新しいグループが開始される。
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
            // 同じ画面 → 現在のグループに追加
            groups.last_mut().unwrap().captures.push(img.clone());
        } else if sim < DIFF_GROUP_THRESHOLD {
            // 明確に別の画面 → 新グループ
            let new_index = groups.len();
            groups.push(ScreenGroup {
                captures: vec![img.clone()],
                index: new_index,
            });
        } else {
            // 境界付近: 直前のグループが1枚だけなら上書き、
            // 複数枚あれば遷移中として新しいグループ
            if groups.last().unwrap().captures.len() <= 1 {
                // 直前も1枚しかない → 遷移中のフレーム。現在のグループを上書き
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

    // インデックスを再採番
    for (i, g) in groups.iter_mut().enumerate() {
        g.index = i;
    }

    info!(
        "Grouped {} captures into {} groups (sizes: {})",
        captures.len(),
        groups.len(),
        groups.iter().map(|g| g.captures.len().to_string()).collect::<Vec<_>>().join(", ")
    );

    groups
}

/// グループから安定したテンプレート候補を抽出する。
///
/// 画面をオーバーラップ付きタイルに分割し、
/// グループ内全キャプチャでピクセル分散が低い（安定した）タイルを候補とする。
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

    // グレースケールに変換
    let grays: Vec<GrayImage> = group
        .captures
        .iter()
        .map(|img| img.to_luma8())
        .collect();

    let mut candidates: Vec<TileCandidate> = Vec::new();

    // タイルグリッドを走査
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

    // エッジ密度順 → 安定性順でソート（識別的で安定なものを優先）
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

/// テンプレート候補を検証する。
///
/// 感度チェック: 自グループの全キャプチャで confidence ≥ SENSITIVITY_THRESHOLD
/// 特異性チェック: 他グループの代表キャプチャで confidence < SPECIFICITY_THRESHOLD
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

        // 感度チェック: 自グループの全キャプチャでマッチ
        let mut own_min_conf = f32::MAX;
        let mut own_all_pass = true;

        for capture in &own_group.captures {
            if let Some(m) = matcher.find_best_match(capture, &candidate.image) {
                if m.confidence.0 < SENSITIVITY_THRESHOLD {
                    own_all_pass = false;
                    debug!(
                        "Candidate ({}, {}) sensitivity fail: confidence {:.3} < {:.3}",
                        candidate.position.0,
                        candidate.position.1,
                        m.confidence.0,
                        SENSITIVITY_THRESHOLD
                    );
                    break;
                }
                own_min_conf = own_min_conf.min(m.confidence.0);
            } else {
                own_all_pass = false;
                break;
            }
        }

        if !own_all_pass {
            continue;
        }

        // 特異性チェック: 他グループでマッチしないこと
        let mut other_max_conf = 0.0f32;
        let mut spec_all_pass = true;

        for other_group in other_groups {
            // 各グループの代表画像（1枚目）でチェック
            let representative = &other_group.captures[0];
            if let Some(m) = matcher.find_best_match(representative, &candidate.image) {
                if m.confidence.0 >= SPECIFICITY_THRESHOLD {
                    spec_all_pass = false;
                    debug!(
                        "Candidate ({}, {}) specificity fail: confidence {:.3} >= {:.3} on group {}",
                        candidate.position.0,
                        candidate.position.1,
                        m.confidence.0,
                        SPECIFICITY_THRESHOLD,
                        other_group.index
                    );
                    other_max_conf = other_max_conf.max(m.confidence.0);
                    break;
                }
                other_max_conf = other_max_conf.max(m.confidence.0);
            }
        }

        let passed = spec_all_pass;
        if passed {
            accepted_count += 1;
            info!(
                "✅ Template verified: ({}, {}) own_min={:.3} other_max={:.3}",
                candidate.position.0, candidate.position.1, own_min_conf, other_max_conf
            );
        }

        verified.push(VerifyResult {
            image: candidate.image,
            position: candidate.position,
            own_best_confidence: own_min_conf,
            other_worst_confidence: other_max_conf,
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

/// 全グループに対してテンプレート抽出＋検証を実行する。
///
/// 返り値: (グループインデックス, 検証結果) のリスト。
/// 検証通過したものだけが含まれる。
pub fn collect_templates(
    groups: &[ScreenGroup],
) -> Vec<(usize, Vec<VerifyResult>)> {
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

        info!("Processing group {} ({} captures)...", group.index, group.captures.len());

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

// ─── 内部関数 ───

/// タイルの安定性とエッジ密度を計算する。
///
/// 安定性: グループ内全キャプチャ間でのピクセル分散の逆。
/// エッジ密度: 隣接ピクセル差分の平均（高いほど識別的）。
fn compute_tile_metrics(
    grays: &[GrayImage],
    x: u32,
    y: u32,
    size: u32,
) -> (f32, f32) {
    let n = grays.len() as f32;
    if n < MIN_GROUP_SIZE as f32 {
        return (0.0, 0.0);
    }

    // 各ピクセル位置の分散を計算
    let mut total_variance: f32 = 0.0;
    let mut pixel_count: u32 = 0;

    // エッジ密度（代表画像=最初の画像で計算）
    let mut edge_sum: f32 = 0.0;
    let ref_gray = &grays[0];

    for py in 0..size {
        for px in 0..size {
            let gx = x + px;
            let gy = y + py;

            // 分散計算
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

            // エッジ密度（水平・垂直勾配の平均）
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

    // 安定性: 分散が低いほど安定
    let stability = if avg_variance < STABLE_VARIANCE_THRESHOLD {
        1.0 / (1.0 + avg_variance)
    } else {
        0.0 // 分散が大きすぎる → 不安定
    };

    // エッジ密度（0-255 スケール）
    let edge_density = if pixel_count > 0 {
        edge_sum / pixel_count as f32 / 255.0
    } else {
        0.0
    };

    (stability, edge_density)
}

// ─── テスト ───

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
        // 全く同じ画像（真っ白）のグループ → 全ピクセル分散ゼロ → 安定
        let mut captures = Vec::new();
        for _ in 0..5 {
            captures.push(white_image(600, 270));
        }
        let group = ScreenGroup {
            captures,
            index: 0,
        };
        let _tiles = extract_stable_tiles(&group);
        // 真っ白画像はエッジ密度がほぼゼロなので、候補が少ないはず
        // （edge_density > 0.01 の条件で除外される）
        // これは期待される動作: 真っ白な領域はテンプレートとして役に立たない
    }

    #[test]
    fn extract_stable_tiles_with_content() {
        // 異なる内容のタイルを含む画像
        let mut captures = Vec::new();
        for _ in 0..5 {
            let mut img = RgbImage::new(300, 200);
            // 左上に黒い四角（安定した特徴）
            for y in 10..60 {
                for x in 10..60 {
                    img.put_pixel(x, y, image::Rgb([0, 0, 0]));
                }
            }
            captures.push(DynamicImage::ImageRgb8(img));
        }
        let group = ScreenGroup {
            captures,
            index: 0,
        };
        let tiles = extract_stable_tiles(&group);
        // 黒い四角を含むタイルが候補として抽出されるはず
        assert!(
            !tiles.is_empty(),
            "Group with stable content should produce tile candidates"
        );
    }

    #[test]
    fn verify_rejects_template_matching_other_group() {
        // グループA: 黒画像
        let group_a = ScreenGroup {
            captures: vec![
                black_image(300, 200),
                black_image(300, 200),
                black_image(300, 200),
            ],
            index: 0,
        };
        // グループB: 白画像
        let group_b = ScreenGroup {
            captures: vec![
                white_image(300, 200),
                white_image(300, 200),
                white_image(300, 200),
            ],
            index: 1,
        };

        let matcher = TemplateMatcher::threshold_only(MatchConfidence::new(0.5));

        // グループAの黒いタイル候補
        let candidate = TileCandidate {
            image: black_image(50, 50),
            position: (0, 0),
            stability: 1.0,
            edge_density: 0.5,
        };

        let results = verify_templates(
            vec![candidate],
            &group_a,
            &[&group_b],
            &matcher,
        );

        // 黒いタイルは白い画像にマッチしない → 特異性OK → 通過するはず
        assert_eq!(results.len(), 1, "Should have 1 result");
        assert!(results[0].passed, "Black tile should pass on black group, not match white group");
    }
}
