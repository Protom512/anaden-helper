use eframe::egui;

use crate::log_view::LogLevel;
use crate::runner_exec::{PipelineRunnerApp, RunnerPane, RunnerStatus};

pub(crate) fn level_color(level: LogLevel) -> egui::Color32 {
    match level {
        LogLevel::Error => egui::Color32::from_rgb(240, 80, 80),
        LogLevel::Warn => egui::Color32::from_rgb(230, 180, 50),
        LogLevel::Info => egui::Color32::from_gray(200),
    }
}

fn routine_status_line(running: bool) -> &'static str {
    if running {
        "状態: 実行中 (pipeline 実行のため routine 起動不可)"
    } else {
        "状態: 停止"
    }
}

impl PipelineRunnerApp {
    fn log_view_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("実行ログ");
            if ui.button("クリア").clicked() {
                self.clear_logs();
            }
            ui.checkbox(&mut self.auto_scroll, "自動スクロール");
        });
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .stick_to_bottom(self.auto_scroll)
            .show(ui, |ui| {
                if self.log_snapshot.is_empty() {
                    ui.weak("（ログなし）");
                }
                for entry in &self.log_snapshot {
                    ui.monospace(
                        egui::RichText::new(&entry.line)
                            .monospace()
                            .color(level_color(entry.level)),
                    );
                }
            });
    }
}

impl eframe::App for PipelineRunnerApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show_inside(ui, |ui| {
            self.render_body(ui, RunnerPane::Run);
        });
    }
}

impl PipelineRunnerApp {
    pub fn render_body(&mut self, ui: &mut egui::Ui, pane: RunnerPane) {
        match pane {
            RunnerPane::Run => self.render_run_body(ui),
            RunnerPane::History => self.render_history_body(ui),
            RunnerPane::Strategy => self.render_strategy_body(ui),
            RunnerPane::Settings => self.render_settings_body(ui),
        }
    }

    fn render_run_body(&mut self, ui: &mut egui::Ui) {
        {
            ui.heading("anaden pipeline runner");

            let running = self.status() == RunnerStatus::Running;
            ui.add_enabled_ui(!running, |ui| {
                if ui.button("開始").clicked() {
                    self.start_pipeline_with_selection();
                }
            });
            ui.add_enabled_ui(running, |ui| {
                if ui.button("停止").clicked() {
                    self.stop_pipeline();
                }
            });
            ui.add_enabled_ui(!running, |ui| {
                if ui.button("再実行").clicked() {
                    self.rerun_pipeline();
                }
            });

            ui.label(if running {
                "状態: 実行中"
            } else {
                "状態: 停止"
            });

            ui.label(format!("run: {}", self.run_status_summary()));

            if let Some(err) = self.last_error() {
                ui.colored_label(egui::Color32::RED, format!("エラー: {err}"));
            }

            if let Some(failure) = self.failure_summary() {
                ui.colored_label(egui::Color32::RED, format!("直前の実行: {failure}"));
            }

            ui.separator();
            ui.weak(format!("選択: {}", self.strategy_summary));

            ui.separator();
            self.drain_logs();
            self.log_view_ui(ui);

            ui.separator();
            self.render_history_section(ui, running);
        }
    }

    fn render_history_body(&mut self, ui: &mut egui::Ui) {
        ui.heading("実行履歴");
        let running = self.status() == RunnerStatus::Running;
        ui.label(if running {
            "状態: 実行中"
        } else {
            "状態: 停止"
        });
        if let Some(failure) = self.failure_summary() {
            ui.colored_label(egui::Color32::RED, format!("直前の実行: {failure}"));
        }
        ui.separator();
        self.render_history_section(ui, running);
    }

    fn render_history_section(&mut self, ui: &mut egui::Ui, running: bool) {
        self.history_panel.set_running(running);
        self.history_panel.refresh_from(self.history.records());
        self.history_panel.ui(ui);
        self.handle_history_actions();
    }

    fn render_strategy_body(&mut self, ui: &mut egui::Ui) {
        let running = self.status() == RunnerStatus::Running;
        let changed = self.strategy_panel.render_tab_body(ui, running);
        self.on_strategy_changed(changed);
        self.render_routine_section(ui, running);
    }

    fn render_routine_section(&mut self, ui: &mut egui::Ui, running: bool) {
        ui.add_space(8.0);
        ui.separator();
        ui.heading("ルーチン（複数 pipeline の連続実行）");
        ui.label(routine_status_line(running));
        self.routine_panel.ui(ui);
        ui.add_enabled_ui(!running, |ui| {
            if ui.button("ルーチンを実行").clicked() {
                self.start_routine();
            }
        });
        ui.weak(format!("選択: {}", self.routine_panel.summary()));
    }

    fn render_settings_body(&mut self, ui: &mut egui::Ui) {
        ui.heading("設定");
        ui.label(crate::settings::settings_path_display(&self.settings_path));
        ui.separator();
        if ui.button("💾 設定を保存").clicked() {
            self.save_settings_to_path();
        }
        if ui.button("📂 設定を読込").clicked() {
            self.load_settings_from_path();
        }
        match self.settings_tab.status() {
            crate::settings::SettingsTabStatus::Ok(p) => {
                ui.label(format!("設定: {}", p.display()));
            }
            crate::settings::SettingsTabStatus::Err(e) => {
                ui.colored_label(egui::Color32::RED, e.clone());
            }
            crate::settings::SettingsTabStatus::None => {}
        }
        ui.separator();
        ui.weak(format!("現在の選択: {}", self.strategy_summary));
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    /// LogLevel → 表示色の対応（色分け描画の純ロジック部分）。
    #[test]
    fn test_level_color_varies_by_level() {
        assert_ne!(level_color(LogLevel::Error), level_color(LogLevel::Warn));
        assert_ne!(level_color(LogLevel::Warn), level_color(LogLevel::Info));
        assert_ne!(level_color(LogLevel::Error), level_color(LogLevel::Info));
    }

    /// ルーチンセクションの状態行 (Issue #199: 実行中は起動不可の注記)。
    #[test]
    fn test_routine_status_line_reflects_running_state() {
        assert!(routine_status_line(true).contains("実行中"));
        assert_eq!(routine_status_line(false), "状態: 停止");
    }
}
