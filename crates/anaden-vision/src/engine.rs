//! 認識エンジンの抽象化（Strategy パターン）。
//!
//! `VisionEngine` trait でテンプレートマッチ方式を差し替え可能にする。
//! 現状は `SseVisionEngine`（imageproc の正規化SSE）のみ。
//! 将来の CCOEFF 実装や opencv バックエンドもこの trait 配下に追加できる
//! （Wiki [[Vision-Engine-Design]] / [[OpenCV-Integration]] 参照）。

use image::{DynamicImage, GrayImage};

use crate::matcher::{MatchResult, TemplateMatcher};

/// テンプレートマッチ認識エンジンの抽象インターフェース。
///
/// GUI（anaden-studio）と実行エンジン（anaden-engine）の両方がこの trait を通じて
/// 認識を行うことで、マッチ方式の差し替えが上位層に影響しない。
pub trait VisionEngine: Send + Sync {
    /// 最も信頼度の高いマッチを1件返す。閾値未満・テンプレートが大きすぎる場合は `None`。
    fn match_template(&self, haystack: &DynamicImage, needle: &DynamicImage)
    -> Option<MatchResult>;

    /// マッチスコアの全体マップ（ヒートマップ用）。テンプレートが大きすぎる場合は `None`。
    fn score_map(&self, haystack: &DynamicImage, needle: &DynamicImage) -> Option<GrayImage>;

    /// 閾値を超える全マッチを信頼度降順で返す。
    fn match_all(&self, haystack: &DynamicImage, needle: &DynamicImage) -> Vec<MatchResult>;
}

/// imageproc 正規化SSE による実装（現行デフォルト）。
pub struct SseVisionEngine {
    matcher: TemplateMatcher,
}

impl SseVisionEngine {
    /// 指定マッチャーでラップする。
    pub fn new(matcher: TemplateMatcher) -> Self {
        Self { matcher }
    }

    /// デフォルト設定（閾値85%、1/2ダウンスケール）で作成する。
    pub fn with_defaults() -> Self {
        Self::new(TemplateMatcher::with_defaults())
    }
}

impl VisionEngine for SseVisionEngine {
    fn match_template(
        &self,
        haystack: &DynamicImage,
        needle: &DynamicImage,
    ) -> Option<MatchResult> {
        self.matcher.find_best_match(haystack, needle)
    }

    fn score_map(&self, haystack: &DynamicImage, needle: &DynamicImage) -> Option<GrayImage> {
        self.matcher.score_map(haystack, needle)
    }

    fn match_all(&self, haystack: &DynamicImage, needle: &DynamicImage) -> Vec<MatchResult> {
        self.matcher.find_matches(haystack, needle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anaden_core::MatchConfidence;
    use image::{DynamicImage, GrayImage, ImageBuffer, Luma};

    fn solid_image(w: u32, h: u32, color: Luma<u8>) -> DynamicImage {
        DynamicImage::ImageLuma8(ImageBuffer::from_pixel(w, h, color))
    }

    #[test]
    fn sse_engine_finds_best_match() {
        let mut bg = GrayImage::from_pixel(100, 100, Luma([255]));
        for y in 40..60 {
            for x in 30..50 {
                bg.put_pixel(x, y, Luma([0]));
            }
        }
        let haystack = DynamicImage::ImageLuma8(bg);
        let needle = solid_image(20, 20, Luma([0]));

        let engine = SseVisionEngine::new(TemplateMatcher::threshold_only(MatchConfidence(0.5)));
        let m = engine
            .match_template(&haystack, &needle)
            .expect("should match");

        assert!(m.region.x <= 35 && m.region.x >= 25);
        assert!(m.region.y <= 45 && m.region.y >= 35);
    }

    #[test]
    fn score_map_has_expected_dimensions() {
        let mut bg = GrayImage::from_pixel(100, 100, Luma([255]));
        for y in 40..60 {
            for x in 30..50 {
                bg.put_pixel(x, y, Luma([0]));
            }
        }
        let haystack = DynamicImage::ImageLuma8(bg);
        let needle = solid_image(20, 20, Luma([0]));

        let engine = SseVisionEngine::new(TemplateMatcher::threshold_only(MatchConfidence(0.0)));
        let map = engine
            .score_map(&haystack, &needle)
            .expect("score map should exist");

        // ダウンスケール1（threshold_only）なので (100-20+1)^2
        assert_eq!(map.dimensions(), (81, 81));
    }

    #[test]
    fn score_map_none_when_needle_too_large() {
        let haystack = solid_image(10, 10, Luma([0]));
        let needle = solid_image(20, 20, Luma([0]));

        let engine = SseVisionEngine::new(TemplateMatcher::threshold_only(MatchConfidence(0.0)));
        assert!(engine.score_map(&haystack, &needle).is_none());
    }
}
