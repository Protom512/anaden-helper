//! テンプレートマッチングエンジン。
//!
//! `imageproc` の正規化誤差二乗和（SumOfSquaredErrorsNormalized）を使用して、
//! 画面キャプチャ内からテンプレート画像の位置と信頼度を検出する。
//! SSE は 0.0（完全一致）〜 大きい値（不一致）のため、信頼度は `1.0 - sse` に変換する。
//!
//! **パフォーマンス最適化**:
//! - `find_best_match`: 最小SSEを1パスで発見（Vec確保なし）。高速。
//! - `downscale_factor` で両画像を縮小してからマッチング。
//! - デフォルトダウンスケール 1/2（1/4だと詳細がつぶれて誤検出が増える）。

use image::{DynamicImage, GrayImage, Luma};
use imageproc::template_matching::match_template;
use tracing::debug;

use anaden_core::{MatchConfidence, ScreenRegion};

/// テンプレートマッチングの1件の検出結果。
#[derive(Debug, Clone)]
pub struct MatchResult {
    /// マッチした領域（元の解像度の座標）
    pub region: ScreenRegion,
    /// 信頼度（0.0〜1.0）
    pub confidence: MatchConfidence,
}

/// テンプレートマッチングを実行するエンジン。
pub struct TemplateMatcher {
    /// マッチ判定の最低閾値
    threshold: MatchConfidence,
    /// ダウンスケール倍率（例: 2 で 1/2 に縮小してマッチング）
    downscale_factor: u32,
}

impl TemplateMatcher {
    /// 指定閾値とダウンスケール倍率でマッチャーを作成する。
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

    /// デフォルト設定（閾値85%、1/2ダウンスケール）でマッチャーを作成する。
    pub fn with_defaults() -> Self {
        Self::new(MatchConfidence::DEFAULT_THRESHOLD, 2)
    }

    /// 指定閾値のみ指定（ダウンスケールなし）。
    pub fn threshold_only(threshold: MatchConfidence) -> Self {
        Self::new(threshold, 1)
    }

    /// 最も信頼度の高いマッチを1つだけ返す。見つからなければ `None`。
    ///
    /// **高速実装**: `match_template` の結果から最小SSEピクセルを
    /// 1パスで発見する。Vec確保もソートも不要。
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

        // 1パスで最小SSE（= 最良マッチ）を見つける。Vec確保なし。
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

    /// 画面画像内からテンプレート画像を検索し、閾値を超える全マッチを返す。
    ///
    /// **注意**: これは全画素走査するため `find_best_match` より遅い。
    /// NMS（非最大抑制）が必要な場合のみ使用する。
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

    /// マッチスコアの全体マップを返す（ヒートマップ用）。
    ///
    /// 戻り値は各テンプレート開始位置の信頼度を 0-255 にスケールしたグレースケア画像。
    /// サイズは `(画面幅 - テンプレート幅 + 1) × (画面高 - テンプレート高 + 1)`（ダウンスケール空間）。
    /// テンプレートが画面より大きい場合は `None`。
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

    /// 両画像をダウンスケールする。テンプレートが画面より大きい場合は None。
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
        // find_best_match の結果が find_matches の最高信頼度と一致する
        assert!((best.unwrap().confidence.0 - all[0].confidence.0).abs() < 0.001);
    }

    #[test]
    fn no_match_returns_none() {
        let haystack = DynamicImage::ImageLuma8(solid_image(100, 100, Luma([255])));
        let needle = DynamicImage::ImageLuma8(solid_image(20, 20, Luma([0])));

        // 白い画面に黒いテンプレート → 高閾値ならマッチしない
        let matcher = TemplateMatcher::threshold_only(MatchConfidence::new(0.999));
        let result = matcher.find_best_match(&haystack, &needle);

        assert!(result.is_none());
    }
}
