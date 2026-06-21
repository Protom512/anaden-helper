//! バッチ評価と混同行列（純関数・テスト対象）。
//!
//! テンプレートライブラリ全体をラベル付きテスト画像群に対して一括評価し、
//! 「真の状態 × 予測状態」の混同行列とテンプレート別の感度/特異性を算出する。
//! 決定規則は「最もスコアの高いテンプレートの状態（閾値以上）。なければ unknown」。

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

use image::DynamicImage;

use anaden_vision::VisionEngine;

use crate::library::TemplateSpec;

/// 評価済みテンプレート（仕様 + 画像）。
pub struct LoadedTemplate {
    pub spec: TemplateSpec,
    pub image: DynamicImage,
}

/// テスト画像とその真のラベル。
pub struct TestImage {
    pub true_label: String,
    pub image: Arc<DynamicImage>,
}

/// テンプレート別の評価レポート。
#[derive(Debug, Clone)]
pub struct TemplateReport {
    pub name: String,
    pub state: String,
    pub sensitivity: f32, // 正例(同状態)でマッチした割合
    pub specificity: f32, // 負例(別状態)でマッチしなかった割合
}

/// 混同行列の評価結果。
#[derive(Debug, Clone)]
pub struct ConfusionMatrix {
    /// ラベル一覧（整列済み、unknown 含む）。行列の軸順序に対応。
    pub labels: Vec<String>,
    /// `matrix[true_index][predicted_index]` の件数。
    pub matrix: Vec<Vec<usize>>,
    /// テンプレート別レポート。
    pub per_template: Vec<TemplateReport>,
    /// 評価したテスト画像総数。
    pub total: usize,
}

impl ConfusionMatrix {
    /// ラベルのインデックスを返す（無ければ None）。
    #[allow(dead_code)]
    pub fn index_of(&self, label: &str) -> Option<usize> {
        self.labels.iter().position(|l| l == label)
    }

    /// 正答率（対角成分 / 総数）。
    pub fn accuracy(&self) -> f32 {
        if self.total == 0 {
            return 0.0;
        }
        let diag: usize = (0..self.labels.len())
            .map(|i| {
                self.matrix
                    .get(i)
                    .and_then(|r| r.get(i))
                    .copied()
                    .unwrap_or(0)
            })
            .sum();
        diag as f32 / self.total as f32
    }
}

/// テンプレート群をテスト画像群に対して一括評価し混同行列を構築する。
///
/// `engine` は閾値0（常に最良スコアを返す）を想定。`threshold` は
/// 「マッチとみなす」決定閾値。
pub fn evaluate(
    engine: &dyn VisionEngine,
    templates: &[LoadedTemplate],
    tests: &[TestImage],
    threshold: f32,
) -> ConfusionMatrix {
    // ラベル集合: テンプレート状態 ∪ テスト真ラベル ∪ {unknown}
    let mut label_set: BTreeSet<String> = BTreeSet::new();
    for t in templates {
        label_set.insert(t.spec.state.clone());
    }
    for t in tests {
        label_set.insert(t.true_label.clone());
    }
    label_set.insert("unknown".to_string());
    let labels: Vec<String> = label_set.into_iter().collect();
    let n = labels.len();
    let mut matrix = vec![vec![0usize; n]; n];

    // 各テスト画像の予測状態を決定
    let mut per_image_scores: Vec<Vec<f32>> = Vec::with_capacity(tests.len()); // [test][template]
    for test in tests {
        let scores: Vec<f32> = templates
            .iter()
            .map(|t| {
                engine
                    .match_template(&test.image, &t.image)
                    .map(|m| m.confidence.0)
                    .unwrap_or(0.0)
            })
            .collect();

        // 最高スコアのテンプレートを予測とする
        let predicted = scores
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
            .and_then(|(idx, &best_score)| {
                if best_score >= threshold {
                    Some(templates[idx].spec.state.clone())
                } else {
                    None
                }
            })
            .unwrap_or_else(|| "unknown".to_string());
        per_image_scores.push(scores);

        let ti = labels
            .iter()
            .position(|l| *l == test.true_label)
            .unwrap_or(n - 1);
        let pi = labels.iter().position(|l| *l == predicted).unwrap_or(n - 1);
        matrix[ti][pi] += 1;
    }

    // テンプレート別の感度/特異性
    let mut per_template = Vec::with_capacity(templates.len());
    for (tidx, t) in templates.iter().enumerate() {
        let mut tp = 0usize; // 正例(同状態)でマッチ
        let mut pos = 0usize; // 正例総数
        let mut fp = 0usize; // 負例(別状態)でマッチ
        let mut neg = 0usize; // 負例総数
        for (test_idx, test) in tests.iter().enumerate() {
            let score = per_image_scores[test_idx][tidx];
            let is_match = score >= threshold;
            if test.true_label == t.spec.state {
                pos += 1;
                if is_match {
                    tp += 1;
                }
            } else {
                neg += 1;
                if is_match {
                    fp += 1;
                }
            }
        }
        let sensitivity = if pos > 0 { tp as f32 / pos as f32 } else { 0.0 };
        let specificity = if neg > 0 {
            1.0 - (fp as f32 / neg as f32)
        } else {
            1.0
        };
        per_template.push(TemplateReport {
            name: t.spec.name.clone(),
            state: t.spec.state.clone(),
            sensitivity,
            specificity,
        });
    }

    ConfusionMatrix {
        labels,
        matrix,
        per_template,
        total: tests.len(),
    }
}

/// ライブラリディレクトリから評価用テンプレート（TOML + PNG）を読み込む。
pub fn load_templates_for_eval(base_dir: &Path) -> Vec<LoadedTemplate> {
    let mut out = Vec::new();
    let Ok(state_dirs) = std::fs::read_dir(base_dir) else {
        return out;
    };
    for state_dir in state_dirs.flatten() {
        if !state_dir.path().is_dir() {
            continue;
        }
        let Ok(files) = std::fs::read_dir(state_dir.path()) else {
            continue;
        };
        for f in files.flatten() {
            let p = f.path();
            if p.extension().and_then(|e| e.to_str()) != Some("toml") {
                continue;
            }
            let Ok(content) = std::fs::read_to_string(&p) else {
                continue;
            };
            let Ok(spec) = toml::from_str::<TemplateSpec>(&content) else {
                continue;
            };
            let png_path = p.with_extension("png");
            if let Ok(image) = image::open(&png_path) {
                out.push(LoadedTemplate { spec, image });
            }
        }
    }
    out
}

/// テストディレクトリ（<dir>/<label>/*.png）からテストセットを読み込む。
pub fn load_test_set(test_dir: &Path) -> Vec<TestImage> {
    let mut out = Vec::new();
    let Ok(label_dirs) = std::fs::read_dir(test_dir) else {
        return out;
    };
    for label_dir in label_dirs.flatten() {
        let lpath = label_dir.path();
        if !lpath.is_dir() {
            continue;
        }
        let label = lpath
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let Ok(files) = std::fs::read_dir(&lpath) else {
            continue;
        };
        for f in files.flatten() {
            let p = f.path();
            if matches!(
                p.extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.to_lowercase())
                    .as_deref(),
                Some("png") | Some("jpg") | Some("jpeg") | Some("bmp")
            ) && let Ok(image) = image::open(&p)
            {
                out.push(TestImage {
                    true_label: label.clone(),
                    image: Arc::new(image),
                });
            }
        }
    }
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

    fn screen_with_square(x0: u32, y0: u32) -> DynamicImage {
        let mut img = GrayImage::from_pixel(100, 100, Luma([255]));
        for y in y0..y0 + 20 {
            for x in x0..x0 + 20 {
                img.put_pixel(x, y, Luma([0]));
            }
        }
        DynamicImage::ImageLuma8(img)
    }

    fn black_template() -> DynamicImage {
        DynamicImage::ImageLuma8(GrayImage::from_pixel(20, 20, Luma([0])))
    }

    fn white_image() -> DynamicImage {
        DynamicImage::ImageLuma8(GrayImage::from_pixel(100, 100, Luma([255])))
    }

    fn spec(name: &str, state: &str) -> TemplateSpec {
        TemplateSpec {
            name: name.to_string(),
            state: state.to_string(),
            roi: ScreenRegion::new(0, 0, 20, 20),
            threshold: 0.5,
            method: "sse".to_string(),
        }
    }

    use anaden_core::ScreenRegion;

    #[test]
    fn single_template_predicts_own_state_and_unknown_for_others() {
        // テンプレ: 黒四角 → title。白画面にはマッチしない（scoring テストで確認済み <0.5）
        let templates = vec![LoadedTemplate {
            spec: spec("black", "title"),
            image: black_template(),
        }];
        let tests = vec![
            TestImage {
                true_label: "title".into(),
                image: Arc::new(screen_with_square(30, 40)),
            },
            TestImage {
                true_label: "field".into(),
                image: Arc::new(white_image()),
            },
        ];
        let cm = evaluate(&engine(), &templates, &tests, 0.5);
        assert_eq!(cm.total, 2);
        let ti = cm.index_of("title").unwrap();
        let fi = cm.index_of("field").unwrap();
        let unk = cm.index_of("unknown").unwrap();
        assert_eq!(cm.matrix[ti][ti], 1, "title test -> predicted title");
        assert_eq!(cm.matrix[fi][unk], 1, "field test -> predicted unknown");
    }

    #[test]
    fn sub_threshold_predicted_unknown() {
        let templates = vec![LoadedTemplate {
            spec: spec("black", "title"),
            image: black_template(),
        }];
        // 白画面に黒テンプレ → 低スコア → 閾値超えず unknown
        let tests = vec![TestImage {
            true_label: "field".into(),
            image: Arc::new(DynamicImage::ImageLuma8(GrayImage::from_pixel(
                100,
                100,
                Luma([255]),
            ))),
        }];
        let cm = evaluate(&engine(), &templates, &tests, 0.99);
        let unk = cm.index_of("unknown").unwrap();
        let field = cm.index_of("field").unwrap();
        // 真field → 予測unknown
        assert_eq!(cm.matrix[field][unk], 1);
    }

    #[test]
    fn per_template_sensitivity_specificity() {
        let templates = vec![LoadedTemplate {
            spec: spec("black", "title"),
            image: black_template(),
        }];
        // 正例2枚(黒四角含む) + 負例1枚(白)
        let tests = vec![
            TestImage {
                true_label: "title".into(),
                image: Arc::new(screen_with_square(30, 40)),
            },
            TestImage {
                true_label: "title".into(),
                image: Arc::new(screen_with_square(10, 10)),
            },
            TestImage {
                true_label: "field".into(),
                image: Arc::new(DynamicImage::ImageLuma8(GrayImage::from_pixel(
                    100,
                    100,
                    Luma([255]),
                ))),
            },
        ];
        let cm = evaluate(&engine(), &templates, &tests, 0.5);
        assert_eq!(cm.per_template.len(), 1);
        assert!((cm.per_template[0].sensitivity - 1.0).abs() < 1e-6);
        assert!((cm.per_template[0].specificity - 1.0).abs() < 1e-6);
    }
}
