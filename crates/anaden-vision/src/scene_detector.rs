//! 画面キャプチャから GameState への変換を担当する。
//!
//! 複数のテンプレートを画面に対してマッチングし、
//! 最も確からしい GameState を決定する。

use image::DynamicImage;
use tracing::debug;

use anaden_core::{GameState, MatchConfidence, RecognitionResult, TemplateMatch};

use crate::matcher::TemplateMatcher;
use crate::template_store::TemplateStore;

/// 画面のゲーム状態を検出する。
pub struct SceneDetector {
    matcher: TemplateMatcher,
    store: TemplateStore,
}

impl SceneDetector {
    /// テンプレートストアとマッチャーから検出器を作成する。
    pub fn new(store: TemplateStore, matcher: TemplateMatcher) -> Self {
        Self { matcher, store }
    }

    /// デフォルト設定で検出器を作成する。
    pub fn with_defaults(store: TemplateStore) -> Self {
        Self {
            matcher: TemplateMatcher::with_defaults(),
            store,
        }
    }

    /// 画面画像を解析して `RecognitionResult` を返す。
    pub fn detect_scene(&self, screenshot: &DynamicImage) -> RecognitionResult {
        let screen_size = (screenshot.width(), screenshot.height());
        let mut all_matches: Vec<TemplateMatch> = Vec::new();

        for template in self.store.all_templates() {
            let matches = self.matcher.find_matches(screenshot, &template.image);

            for m in matches {
                all_matches.push(TemplateMatch {
                    region: m.region,
                    confidence: m.confidence,
                    state: template.state.clone(),
                });
            }
        }

        // 信頼度降順でソート
        all_matches.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        debug!(
            "Scene detection: {} matches found for {}x{} screen",
            all_matches.len(),
            screen_size.0,
            screen_size.1
        );

        RecognitionResult {
            matches: all_matches,
            screen_size,
        }
    }

    /// 画面画像を解析して、閾値を超える最も確からしい GameState を返す。
    pub fn detect_state(&self, screenshot: &DynamicImage) -> GameState {
        let result = self.detect_scene(screenshot);
        result.to_game_state(&MatchConfidence::DEFAULT_THRESHOLD)
    }

    /// 登録されているテンプレート数を返す。
    pub fn template_count(&self) -> usize {
        self.store.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::template_store::TemplateStore;
    use image::{DynamicImage, RgbImage};

    fn white_image(w: u32, h: u32) -> DynamicImage {
        DynamicImage::ImageRgb8(RgbImage::from_pixel(w, h, image::Rgb([255, 255, 255])))
    }

    fn black_image(w: u32, h: u32) -> DynamicImage {
        DynamicImage::ImageRgb8(RgbImage::from_pixel(w, h, image::Rgb([0, 0, 0])))
    }

    #[test]
    fn detect_state_with_no_templates_returns_unknown() {
        let store = TemplateStore::new();
        let detector = SceneDetector::with_defaults(store);
        // テスト用に小さい画像を使用（テンプレートなしテストは解像度无关）
        let screenshot = white_image(200, 200);

        let state = detector.detect_state(&screenshot);
        assert_eq!(state, GameState::Unknown);
    }

    #[test]
    fn detect_state_with_template_match() {
        let mut store = TemplateStore::new();
        // テスト用に小さいテンプレートを登録
        store.register("title", black_image(50, 50), GameState::TitleScreen);

        let detector = SceneDetector::with_defaults(store);

        // 小さい画面で高速にテスト
        let screenshot = black_image(200, 200);
        let state = detector.detect_state(&screenshot);
        assert_eq!(state, GameState::TitleScreen);
    }
}
