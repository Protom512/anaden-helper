//! Pipeline 実行ランナーの egui 描画（Issue #168 分割）。
//!
//! 旧 runner.rs から描画系を移動:
//! - `level_color` (LogLevel の色分け・Tasks ペインと共有の純関数)
//! - `PipelineRunnerApp::render_body` 系 (Run / History / Strategy / Settings
//!   の 4 ペイン描画・Issue #120 欠陥2 / Issue #125 shard 3)
//! - ログビューア本体 (`log_view_ui`: LogLevel 色分け・自動スクロール・クリア)
//! - `eframe::App` 実装
//!
//! 状態遷移・子プロセス管理・ログ drain は runner_exec.rs (egui 非依存)。
//! 本モジュールは runner_exec への単一方向依存のみを持つ。

use eframe::egui;

use crate::log_view::LogLevel;
use crate::runner_exec::{PipelineRunnerApp, RunnerPane, RunnerStatus};

/// LogLevel の表示色（スクロールログビューアの色分け・純関数）。
///
/// Issue #154 Shard 1: Tasks ペイン (app.rs) のログビューアからも再利用する
/// ため pub(crate) 化 (単一実装の共有)。
pub(crate) fn level_color(level: LogLevel) -> egui::Color32 {
    match level {
        LogLevel::Error => egui::Color32::from_rgb(240, 80, 80),
        LogLevel::Warn => egui::Color32::from_rgb(230, 180, 50),
        LogLevel::Info => egui::Color32::from_gray(200),
    }
}

impl PipelineRunnerApp {
    /// ログビューア本体（シャード3前半: LogLevel 色分け・自動スクロール・クリア）。
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
    /// 実行GUI 本体ペイン（Issue #119: 統合シェルへの埋め込み用公開 API）。
    ///
    /// `eframe::App::ui` から CentralPanel の内側を切り出したもの。
    /// 単一ウィンドウ統合 GUI (`shell::UnifiedShell`) の Run/History タブから
    /// 委譲される。
    /// モード本体を親レイアウト内に描画する埋め込み用 API（統合GUIシェル経由）。
    ///
    /// Issue #120 欠陥2修正: かつて Run/History 両タブが同一内容を描画していた
    /// （履歴タブがダミー）。`RunnerPane` で実行ビューと履歴ビューを区別する。
    pub fn render_body(&mut self, ui: &mut egui::Ui, pane: RunnerPane) {
        match pane {
            RunnerPane::Run => self.render_run_body(ui),
            RunnerPane::History => self.render_history_body(ui),
            RunnerPane::Strategy => self.render_strategy_body(ui),
            RunnerPane::Settings => self.render_settings_body(ui),
        }
    }

    /// 実行ビュー（開始/停止/再実行・戦略選択・ログ・履歴の全量）。
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

            // 実行状態サマリ（goal/iterations/stop_reason・Issue #88 タスク3）。
            ui.label(format!("run: {}", self.run_status_summary()));

            if let Some(err) = self.last_error() {
                ui.colored_label(egui::Color32::RED, format!("エラー: {err}"));
            }

            // 異常終了検知時の失敗状態サマリ（UC-3: 状態 + エラーログ末尾）。
            if let Some(failure) = self.failure_summary() {
                ui.colored_label(egui::Color32::RED, format!("直前の実行: {failure}"));
            }

            ui.separator();
            // 戦略選択サマリ一行（Issue #125 shard 3: 選択 UI は Strategy タブへ
            // 切り出したため、実行ビューにはサマリのみ残す）。
            ui.weak(format!("選択: {}", self.strategy_summary));

            ui.separator();
            // ログビューア（シャード3前半・毎フレーム drain）。
            self.drain_logs();
            self.log_view_ui(ui);

            ui.separator();
            // 履歴ビューペイン（タスク5・UC-1/UC-2）。毎フレーム実行状態と
            // 履歴ストアを同期し、ボタン操作は handle_history_actions で処理。
            self.render_history_section(ui, running);
        }
    }

    /// 履歴ビュー（履歴テーブル + 設定保存/読込のみ。実行制御は含まない）。
    ///
    /// Issue #120 欠陥2: 統合GUIの「🕘 履歴」タブ専用ビュー。実行ビューと
    /// 区別され、履歴参照・再実行・設定操作に集中したレイアウト。
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

    /// 履歴セクション（両ビュー共通）。毎フレーム実行状態と履歴ストアを同期し、
    /// ボタン操作は handle_history_actions で処理。
    fn render_history_section(&mut self, ui: &mut egui::Ui, running: bool) {
        self.history_panel.set_running(running);
        self.history_panel.refresh_from(self.history.records());
        self.history_panel.ui(ui);
        self.handle_history_actions();
    }

    /// 戦略選択ビュー（Issue #125 shard 3: 実行ビューからの切り出し）。
    ///
    /// 描画本体は strategy_ui.rs の `render_tab_body` へ委譲（runner.rs は
    /// 500 行ルール超過のためロジックを持たない・estimate 承認条件）。
    /// パネルは runner が保持する単一インスタンスを実行ビューと共有する。
    fn render_strategy_body(&mut self, ui: &mut egui::Ui) {
        let running = self.status() == RunnerStatus::Running;
        let changed = self.strategy_panel.render_tab_body(ui, running);
        self.on_strategy_changed(changed);
    }

    /// 設定ビュー（Issue #125 shard 3: 設定保存/読込の独立タブ化）。
    ///
    /// 保存/読込ロジックは settings.rs の `SettingsTab` へ集約し、ここでは
    /// ボタン描画とステータス表示のみ（runner.rs 肥大化の回避）。
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
}
