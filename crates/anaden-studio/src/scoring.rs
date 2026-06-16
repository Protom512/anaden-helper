//! ライブ識別力スコアリング（純関数・テスト対象）。
//!
//! ROI候補画像を正例（同じ画面状態）/負例（別状態）の参照画像群に対して
//! 評価し、「識別マージン」を算出する。GUI のライブスコアパネルの中核。
//!
//! 設計意図: スコアリングは純関数化し UI から分離する。これにより
//! 単体テストが容易で、VisionEngine の差し替え（SSE→CCOEFF）が透過的。

use std::sync::Arc;

use anaden_vision::VisionEngine;
use image::DynamicImage;

/// ROI候補の正例/負例に対する識別力の集計結果。
#[derive(Debug, Clone)]
pub struct Discrimination {
    /// 正例（同じ画面）の最低スコア。高いほど良い。
    pub own_min: f32,
    /// 負例（別画面）の最高スコア。低いほど良い。
    pub other_max: f32,
    /// 正例ごとのスコア一覧。
    pub own_scores: Vec<f32>,
    /// 負例ごとのスコア一覧。
    pub other_scores: Vec<f32>,
}

impl Discrimination {
    /// 識別マージン = own_min - other_max。正で識別可能、大きいほど安全。
    pub fn margin(&self) -> f32 {
        self.own_min - self.other_max
    }
}

/// ROI候補画像に対する正例/負例の識別力を評価する。
///
/// `engine` は閾値0（常に最良スコアを返す）設定を想定。こうすることで
/// 閾値未満の低スコアも見え、負例でのマッチ具合が分かる。
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

/// 1組の（画面, テンプレート）に対する最良信頼度。マッチ不能時は 0.0。
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

    /// 白背景に中央付近(30,40)-(50,60)に黒四角のある画面。
    fn screen_with_square() -> DynamicImage {
        let mut img = GrayImage::from_pixel(100, 100, Luma([255]));
        for y in 40..60 {
            for x in 30..50 {
                img.put_pixel(x, y, Luma([0]));
            }
        }
        DynamicImage::ImageLuma8(img)
    }

    /// 完全に白い画面（別状態）。
    fn white_screen() -> DynamicImage {
        DynamicImage::ImageLuma8(GrayImage::from_pixel(100, 100, Luma([255])))
    }

    /// 20x20 の黒塗りテンプレート候補。
    fn black_roi() -> DynamicImage {
        DynamicImage::ImageLuma8(GrayImage::from_pixel(20, 20, Luma([0])))
    }

    fn test_engine() -> SseVisionEngine {
        // 閾値0: 常に最良スコアを返す
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
        // 正例も負例も黒四角を含む → 識別不可（マージン小）
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
        // 正例なし → own_min は f32::MAX、負例なし → other_max は 0.0
        assert_eq!(d.own_min, f32::MAX);
        assert_eq!(d.other_max, 0.0);
    }
}
