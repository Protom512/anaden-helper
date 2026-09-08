//! StudioApp のバッチ評価モード描画 (Issue #174: app_ui.rs 分割)。
//!
//! テンプレート × テスト画像で混同行列を作成する Batch モードの UI と
//! 評価実行。[`crate::app_ui_body`] の [`StudioApp::render_body`] から
//! Batch 分岐で呼ばれる。

use eframe::egui;

use crate::app_state::StudioApp;
use crate::batch;

impl StudioApp {
    /// バッチ評価モードのUI。
    pub(crate) fn batch_ui(&mut self, ui: &mut egui::Ui) {
        egui::Panel::left("batch_controls")
            .resizable(true)
            .default_size(340.0)
            .show_inside(ui, |ui| {
                ui.heading("バッチ評価");
                ui.label("テンプレート × テスト画像で混同行列を作成");
                ui.separator();
                ui.label(format!("テンプレート元: {}", self.save_dir.display()));
                ui.label(format!("テスト元: {}", self.test_dir.display()));
                if ui.button("テスト元変更").clicked()
                    && let Some(dir) = rfd::FileDialog::new().pick_folder()
                {
                    self.test_dir = dir;
                }
                ui.horizontal(|ui| {
                    ui.label("閾値:");
                    ui.add(egui::Slider::new(&mut self.batch_threshold, 0.0..=1.0));
                });
                let mut run_clicked = false;
                if ui.button("実行").clicked() {
                    run_clicked = true;
                }
                if run_clicked {
                    self.run_batch();
                }
                ui.separator();
                ui.label(&self.status);
            });

        egui::CentralPanel::default().show_inside(ui, |ui| {
            if let Some(cm) = &self.batch_result {
                batch::render_confusion_matrix(ui, cm);
            } else {
                ui.heading("「実行」でバッチ評価を行います");
                ui.label("テンプレート元フォルダ（PNG+TOML）と、");
                ui.label("テストフォルダ（<ラベル名>/画像）を選んでください");
            }
        });
    }

    /// バッチ評価を実行する。
    fn run_batch(&mut self) {
        let templates = batch::load_templates_for_eval(&self.save_dir);
        if templates.is_empty() {
            self.status = format!("テンプレート未検出: {}", self.save_dir.display());
            return;
        }
        let tests = batch::load_test_set(&self.test_dir);
        if tests.is_empty() {
            self.status = format!("テスト画像未検出: {}", self.test_dir.display());
            return;
        }
        self.status = format!(
            "評価中... {} テンプレ × {} テスト",
            templates.len(),
            tests.len()
        );
        let cm = batch::evaluate(
            self.engine.as_ref(),
            &templates,
            &tests,
            self.batch_threshold,
        );
        self.status = format!(
            "完了: 正答率 {:.1}% ({} テンプレ × {} テスト)",
            cm.accuracy() * 100.0,
            templates.len(),
            tests.len()
        );
        self.batch_result = Some(cm);
    }
}
