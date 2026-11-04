use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

use eframe::egui;
use image::DynamicImage;

use anaden_vision::VisionEngine;

use crate::library::TemplateSpec;

pub struct LoadedTemplate {
    pub spec: TemplateSpec,
    pub image: DynamicImage,
}

pub struct TestImage {
    pub true_label: String,
    pub image: Arc<DynamicImage>,
}

#[derive(Debug, Clone)]
pub struct TemplateReport {
    pub name: String,
    pub state: String,
    pub sensitivity: f32,
    pub specificity: f32,
}

#[derive(Debug, Clone)]
pub struct ConfusionMatrix {
    pub labels: Vec<String>,
    pub matrix: Vec<Vec<usize>>,
    pub per_template: Vec<TemplateReport>,
    pub total: usize,
}

impl ConfusionMatrix {
    #[allow(dead_code)]
    pub fn index_of(&self, label: &str) -> Option<usize> {
        self.labels.iter().position(|l| l == label)
    }

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

fn collect_labels(templates: &[LoadedTemplate], tests: &[TestImage]) -> Vec<String> {
    let mut label_set: BTreeSet<String> = BTreeSet::new();
    for t in templates {
        label_set.insert(t.spec.state.clone());
    }
    for t in tests {
        label_set.insert(t.true_label.clone());
    }
    label_set.insert("unknown".to_string());
    label_set.into_iter().collect()
}

fn score_matrix(
    engine: &dyn VisionEngine,
    templates: &[LoadedTemplate],
    tests: &[TestImage],
) -> Vec<Vec<f32>> {
    tests
        .iter()
        .map(|test| {
            templates
                .iter()
                .map(|t| {
                    engine
                        .match_template(&test.image, &t.image)
                        .map(|m| m.confidence.0)
                        .unwrap_or(0.0)
                })
                .collect()
        })
        .collect()
}

fn predict_label(scores: &[f32], templates: &[LoadedTemplate], threshold: f32) -> String {
    scores
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
        .and_then(|(idx, &best)| (best >= threshold).then(|| templates[idx].spec.state.clone()))
        .unwrap_or_else(|| "unknown".to_string())
}

fn class_counts(
    tests: &[TestImage],
    per_image_scores: &[Vec<f32>],
    tidx: usize,
    threshold: f32,
    state: &str,
    positive: bool,
) -> (usize, usize) {
    let mut total = 0usize;
    let mut matched = 0usize;
    for (test_idx, test) in tests.iter().enumerate() {
        let is_positive = test.true_label == state;
        if is_positive != positive {
            continue;
        }
        total += 1;
        if per_image_scores[test_idx][tidx] >= threshold {
            matched += 1;
        }
    }
    (total, matched)
}

fn per_template_reports(
    templates: &[LoadedTemplate],
    tests: &[TestImage],
    per_image_scores: &[Vec<f32>],
    threshold: f32,
) -> Vec<TemplateReport> {
    let mut per_template = Vec::with_capacity(templates.len());
    for (tidx, t) in templates.iter().enumerate() {
        let state = t.spec.state.as_str();
        let (pos, tp) = class_counts(tests, per_image_scores, tidx, threshold, state, true);
        let (neg, fp) = class_counts(tests, per_image_scores, tidx, threshold, state, false);
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
    per_template
}

pub fn evaluate(
    engine: &dyn VisionEngine,
    templates: &[LoadedTemplate],
    tests: &[TestImage],
    threshold: f32,
) -> ConfusionMatrix {
    let labels = collect_labels(templates, tests);
    let n = labels.len();
    let mut matrix = vec![vec![0usize; n]; n];

    let per_image_scores = score_matrix(engine, templates, tests);
    for (test_idx, test) in tests.iter().enumerate() {
        let predicted = predict_label(&per_image_scores[test_idx], templates, threshold);
        let ti = labels
            .iter()
            .position(|l| *l == test.true_label)
            .unwrap_or(n - 1);
        let pi = labels.iter().position(|l| *l == predicted).unwrap_or(n - 1);
        matrix[ti][pi] += 1;
    }

    ConfusionMatrix {
        labels,
        matrix,
        per_template: per_template_reports(templates, tests, &per_image_scores, threshold),
        total: tests.len(),
    }
}

pub fn render_confusion_matrix(ui: &mut egui::Ui, cm: &ConfusionMatrix) {
    ui.heading(format!("混同行列（正答率 {:.1}%）", cm.accuracy() * 100.0));
    ui.label(format!(
        "テスト画像 {} 枚 / 状態 {} 種",
        cm.total,
        cm.labels.len()
    ));
    ui.separator();

    egui::Grid::new("confusion")
        .num_columns(cm.labels.len() + 1)
        .striped(true)
        .show(ui, |ui| {
            ui.strong("真\\予測");
            for lbl in &cm.labels {
                ui.strong(lbl);
            }
            ui.end_row();
            for (i, true_lbl) in cm.labels.iter().enumerate() {
                ui.strong(true_lbl);
                for (j, _pred) in cm.labels.iter().enumerate() {
                    let count = cm.matrix[i][j];
                    let color = if i == j && count > 0 {
                        egui::Color32::from_rgb(60, 180, 75)
                    } else if count > 0 {
                        egui::Color32::from_rgb(220, 60, 60)
                    } else {
                        egui::Color32::from_gray(160)
                    };
                    let txt = if count > 0 {
                        format!("{count}")
                    } else {
                        "·".to_string()
                    };
                    ui.colored_label(color, txt);
                }
                ui.end_row();
            }
        });

    ui.separator();
    ui.heading("テンプレート別");
    egui::Grid::new("per_template")
        .striped(true)
        .show(ui, |ui| {
            ui.strong("名前");
            ui.strong("状態");
            ui.strong("感度");
            ui.strong("特異性");
            ui.end_row();
            for r in &cm.per_template {
                ui.label(&r.name);
                ui.label(&r.state);
                ui.monospace(format!("{:.2}", r.sensitivity));
                ui.monospace(format!("{:.2}", r.specificity));
                ui.end_row();
            }
        });
}

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
