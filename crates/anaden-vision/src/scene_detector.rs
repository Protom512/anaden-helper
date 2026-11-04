use std::collections::HashMap;

use image::DynamicImage;
use tracing::{debug, info};

use anaden_core::{GameState, MatchConfidence, RecognitionResult, TemplateMatch};

use crate::matcher::{MatchResult, TemplateMatcher};
use crate::template_store::TemplateEntry;
use crate::template_store::TemplateStore;

const DEFAULT_MIN_VOTES: usize = 2;

const STRICT_SINGLE_TEMPLATE_THRESHOLD: f32 = 0.95;

pub struct SceneDetector {
    matcher: TemplateMatcher,
    store: TemplateStore,
    min_votes: usize,
}

impl SceneDetector {
    pub fn new(store: TemplateStore, matcher: TemplateMatcher) -> Self {
        Self {
            matcher,
            store,
            min_votes: DEFAULT_MIN_VOTES,
        }
    }

    pub fn with_defaults(store: TemplateStore) -> Self {
        Self {
            matcher: TemplateMatcher::with_defaults(),
            store,
            min_votes: DEFAULT_MIN_VOTES,
        }
    }

    pub fn with_min_votes(mut self, min_votes: usize) -> Self {
        self.min_votes = min_votes.max(1);
        self
    }

    pub fn detect_scene(&self, screenshot: &DynamicImage) -> RecognitionResult {
        let screen_size = (screenshot.width(), screenshot.height());

        let mut all_best: Vec<TemplateMatch> = Vec::new();

        for template in self.store.all_templates() {
            if let Some(m) = self.matcher.find_best_match(screenshot, &template.image) {
                debug!(
                    "Template '{}' -> {:?} confidence={:.3}",
                    template.name, template.state, m.confidence.0
                );
                all_best.push(TemplateMatch {
                    region: m.region,
                    confidence: m.confidence,
                    state: template.state.clone(),
                });
            }
        }

        let mut state_groups: HashMap<GameState, Vec<&TemplateMatch>> = HashMap::new();
        for m in &all_best {
            state_groups.entry(m.state.clone()).or_default().push(m);
        }

        let mut confirmed: Vec<TemplateMatch> = Vec::new();

        for (state, matches) in &state_groups {
            let templates_available = self.store.templates_for_state(state).len();
            let min_required = if templates_available < self.min_votes {
                1
            } else {
                self.min_votes
            };

            if matches.len() < min_required {
                debug!(
                    "State {:?}: {}/{} votes ({} templates registered) — below minimum {}, rejected",
                    state,
                    matches.len(),
                    templates_available,
                    templates_available,
                    min_required
                );
                continue;
            }

            if templates_available == 1 {
                let best_conf = matches
                    .iter()
                    .map(|m| m.confidence.0)
                    .fold(f32::MIN, f32::max);
                if best_conf < STRICT_SINGLE_TEMPLATE_THRESHOLD {
                    debug!(
                        "State {:?}: single template match confidence {:.3} < strict threshold {:.3}, rejected",
                        state, best_conf, STRICT_SINGLE_TEMPLATE_THRESHOLD
                    );
                    continue;
                }
            }

            let best = matches
                .iter()
                .max_by(|a, b| {
                    a.confidence
                        .partial_cmp(&b.confidence)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .unwrap();

            info!(
                "State {:?} confirmed: {}/{} votes, best confidence {:.3}",
                state,
                matches.len(),
                templates_available,
                best.confidence.0
            );
            confirmed.push((*best).clone());
        }

        confirmed.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        debug!(
            "Scene detection: {} templates matched, {} states confirmed ({}x{} screen)",
            all_best.len(),
            confirmed.len(),
            screen_size.0,
            screen_size.1
        );

        RecognitionResult {
            matches: confirmed,
            screen_size,
        }
    }

    pub fn detect_state(&self, screenshot: &DynamicImage) -> GameState {
        let result = self.detect_scene(screenshot);
        result.to_game_state(&MatchConfidence::DEFAULT_THRESHOLD)
    }

    pub fn template_count(&self) -> usize {
        self.store.len()
    }

    pub fn template_list(&self) -> impl Iterator<Item = &TemplateEntry> {
        self.store.all_templates()
    }

    pub fn match_single_template(
        &self,
        screenshot: &DynamicImage,
        template: &TemplateEntry,
    ) -> Option<MatchResult> {
        self.matcher.find_best_match(screenshot, &template.image)
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

    fn gray_image(w: u32, h: u32) -> DynamicImage {
        DynamicImage::ImageRgb8(RgbImage::from_pixel(w, h, image::Rgb([128, 128, 128])))
    }

    #[test]
    fn detect_state_with_no_templates_returns_unknown() {
        let store = TemplateStore::new();
        let detector = SceneDetector::with_defaults(store);
        let screenshot = white_image(200, 200);

        let state = detector.detect_state(&screenshot);
        assert_eq!(state, GameState::Unknown);
    }

    #[test]
    fn detect_state_with_single_template_exact_match() {
        let mut store = TemplateStore::new();
        store.register("title", black_image(50, 50), GameState::TitleScreen);

        let detector = SceneDetector::with_defaults(store);
        let screenshot = black_image(200, 200);
        let state = detector.detect_state(&screenshot);
        assert_eq!(state, GameState::TitleScreen);
    }

    #[test]
    fn detect_state_requires_multiple_votes_for_many_templates() {
        let mut store = TemplateStore::new();
        store.register("title_a", black_image(50, 50), GameState::TitleScreen);
        store.register("title_b", white_image(50, 50), GameState::TitleScreen);
        store.register("title_c", gray_image(50, 50), GameState::TitleScreen);

        let detector = SceneDetector::with_defaults(store);

        let screenshot = black_image(200, 200);
        let state = detector.detect_state(&screenshot);
        assert_eq!(
            state,
            GameState::Unknown,
            "1 template match should not be enough when 3 templates are registered"
        );
    }

    #[test]
    fn detect_state_confirms_with_enough_votes() {
        let mut store = TemplateStore::new();
        store.register("title_a", black_image(50, 50), GameState::TitleScreen);
        store.register("title_b", black_image(30, 30), GameState::TitleScreen);

        let detector = SceneDetector::with_defaults(store);

        let screenshot = black_image(200, 200);
        let state = detector.detect_state(&screenshot);
        assert_eq!(
            state,
            GameState::TitleScreen,
            "2 matching templates should confirm the state"
        );
    }

    #[test]
    fn detect_state_non_matching_screen_returns_unknown() {
        let mut store = TemplateStore::new();
        store.register("title_a", black_image(50, 50), GameState::TitleScreen);
        store.register("title_b", black_image(30, 30), GameState::TitleScreen);

        let detector = SceneDetector::with_defaults(store);

        let screenshot = white_image(200, 200);
        let state = detector.detect_state(&screenshot);
        assert_eq!(
            state,
            GameState::Unknown,
            "non-matching screen should return Unknown"
        );
    }
}
