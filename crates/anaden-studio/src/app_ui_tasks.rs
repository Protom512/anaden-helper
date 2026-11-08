use std::path::Path;

use eframe::egui;

use crate::app_state::StudioApp;
use crate::log_view::LogBuffer;
use crate::tasks::{self, QueueState};

impl StudioApp {
    pub fn render_task_list(&mut self, ui: &mut egui::Ui) {
        ui.heading("タスク一覧");
        self.drain_task_logs();
        if self.task_defs.is_none() {
            if ui.button("タスク定義を読み込む").clicked() {
                self.load_task_list(&Self::workspace_root().join("templates/tasks"));
            }
        } else if let Some(list) = self.task_defs.clone() {
            self.task_checkboxes(ui, &list);
            self.task_queue_start_controls(ui, &list);
            Self::task_queue_order_preview(ui, &list);
        }
        self.task_queue_progress(ui);
        ui.separator();
        self.task_log_ui(ui);
        ui.separator();
        ui.label(&self.status);
    }

    fn task_checkboxes(&mut self, ui: &mut egui::Ui, list: &tasks::TaskListState) {
        let target = self.cli_target();
        let serial: Option<&str> = None;
        let root = Self::workspace_root();
        let selected = list.selected_ids().to_vec();
        let mut clicked: Option<String> = None;
        for def in list.definitions() {
            let mut checked = list.is_selected(&def.id);
            let label = tasks::checkbox_label(def);
            ui.horizontal(|ui| {
                ui.add_enabled(
                    def.is_selectable(),
                    egui::Checkbox::new(&mut checked, label),
                );
                if let Some(pos) = tasks::queue_position_label(&selected, &def.id) {
                    ui.weak(pos);
                }
            });
            if checked != list.is_selected(&def.id) {
                clicked = Some(def.id.clone());
            }
            egui::CollapsingHeader::new(egui::RichText::new("詳細").weak())
                .id_salt(&def.id)
                .show(ui, |ui| {
                    Self::task_detail_ui(ui, def, target, serial, &root);
                });
        }
        if let Some(id) = clicked {
            self.toggle_task(&id);
        }
    }

    fn task_queue_start_controls(&mut self, ui: &mut egui::Ui, list: &tasks::TaskListState) {
        let can_start = list.selected_count() > 0 && !self.task_queue_active();
        ui.add_enabled_ui(can_start, |ui| {
            if ui.button("開始").clicked() {
                self.start_task_queue();
            }
        });
    }

    fn task_queue_order_preview(ui: &mut egui::Ui, list: &tasks::TaskListState) {
        let selected = list.selected_ids().to_vec();
        let rows = tasks::queue_order_rows(&selected, list.definitions());
        if rows.is_empty() {
            return;
        }
        ui.separator();
        ui.label("実行順 (チェック順)");
        for row in &rows {
            if row.runnable {
                ui.label(format!("{}. {}", row.position, row.title));
            } else {
                ui.weak(format!("{}. {} (未実装)", row.position, row.title));
            }
        }
    }

    fn task_queue_progress(&mut self, ui: &mut egui::Ui) {
        let Some(queue) = self.task_queue.clone() else {
            return;
        };
        ui.separator();
        ui.label(queue.summary());
        self.task_queue_controls(ui, queue.state());
        for (i, entry) in queue.entries().iter().enumerate() {
            ui.label(format!(
                "{}. [{}] {}",
                i + 1,
                queue.entry_marker(i),
                entry.label
            ));
        }
    }

    fn task_queue_controls(&mut self, ui: &mut egui::Ui, state: &QueueState) {
        match state {
            QueueState::Running { .. } => {
                if ui.button("中止").clicked() {
                    self.abort_task_queue();
                }
            }
            QueueState::PausedAfterFailure { .. } => {
                ui.colored_label(egui::Color32::RED, "タスクが失敗しました。継続しますか?");
                self.task_queue_resume_or_abort(ui);
            }
            QueueState::PausedAfterBudgetExhausted { .. } => {
                ui.colored_label(
                    egui::Color32::YELLOW,
                    "タスクが予算切れ (成果ゼロ) で停止しました。継続しますか?",
                );
                self.task_queue_resume_or_abort(ui);
            }
            QueueState::Pending | QueueState::Completed => {}
        }
    }

    fn task_queue_resume_or_abort(&mut self, ui: &mut egui::Ui) {
        if ui.button("継続").clicked() {
            self.resume_task_queue();
        }
        if ui.button("停止").clicked() {
            self.abort_task_queue();
        }
    }
    fn task_detail_ui(
        ui: &mut egui::Ui,
        def: &tasks::TaskDefinition,
        target: &str,
        serial: Option<&str>,
        root: &Path,
    ) {
        let view = tasks::task_detail_view(def, target, serial, root);
        ui.label(format!("ID: {}", view.id));
        ui.label(format!("種別: {}", view.kind));
        match &view.pipeline_dir {
            Some(dir) => {
                ui.label(format!("pipeline_dir: {dir}"));
            }
            None => {
                ui.weak("pipeline_dir: なし (サブコマンド実行)");
            }
        }
        match &view.start_task {
            Some(task) => {
                ui.label(format!("start_task: {task}"));
            }
            None if def.kind == tasks::TaskKind::PipelineRun => {
                ui.colored_label(egui::Color32::RED, "start_task: 未解決");
            }
            None => {}
        }
        ui.label(format!("引数プレビュー: {}", view.args_preview()));
        if let Some(reason) = &view.unimplemented_reason {
            ui.colored_label(egui::Color32::RED, format!("未実装: {reason}"));
        }
    }

    fn task_log_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("実行ログ");
            let mut follow = self.task_scroll.is_enabled();
            ui.checkbox(&mut follow, "自動スクロール");
            if follow != self.task_scroll.is_enabled() {
                self.task_scroll.set_enabled(follow);
            }
            if ui.button("クリア").clicked() {
                self.task_log.with_buf(LogBuffer::clear);
                self.refresh_task_log_snapshot();
            }
            if self.task_scroll.pending_lines() > 0 {
                ui.weak(format!("新着 {} 行", self.task_scroll.pending_lines()));
            }
        });
        let stick = self.task_scroll.should_stick_to_bottom();
        if self.task_log_snapshot.is_empty() {
            ui.weak("（ログなし）");
        } else {
            egui::ScrollArea::both()
                .auto_shrink([false, false])
                .stick_to_bottom(stick)
                .show_rows(
                    ui,
                    ui.text_style_height(&egui::TextStyle::Monospace),
                    self.task_log_snapshot.len(),
                    |ui, rows| {
                        for i in rows {
                            let entry = &self.task_log_snapshot[i];
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(&entry.line)
                                        .monospace()
                                        .color(crate::runner::level_color(entry.level)),
                                )
                                .wrap_mode(egui::TextWrapMode::Extend),
                            );
                        }
                    },
                );
        }
        if stick {
            self.task_scroll.on_scrolled_to_bottom();
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::childproc::SpawnSpec;
    use crate::tasks::QueueEntry;

    fn child_ui(ctx: &egui::Context) -> egui::Ui {
        egui::Ui::new(
            ctx.clone(),
            egui::Id::new("test-area"),
            egui::UiBuilder::new().max_rect(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800.0, 600.0),
            )),
        )
    }

    fn tasks_dir() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("templates")
            .join("tasks")
    }

    fn long_spec() -> SpawnSpec {
        if cfg!(windows) {
            SpawnSpec::new(
                "ping",
                ["-n".to_string(), "30".to_string(), "127.0.0.1".to_string()],
            )
        } else {
            SpawnSpec::new(
                "ping",
                ["-c".to_string(), "30".to_string(), "127.0.0.1".to_string()],
            )
        }
    }

    fn queue_entry(label: &str, spec: SpawnSpec) -> QueueEntry {
        QueueEntry {
            label: label.to_string(),
            spec,
        }
    }

    #[test]
    fn embed_render_task_list_completes_without_panic() {
        let ctx = egui::Context::default();
        let mut app = StudioApp::default();
        app.load_task_list(&tasks_dir());
        ctx.begin_pass(egui::RawInput::default());
        app.render_task_list(&mut child_ui(&ctx));
        let _ = ctx.end_pass();
    }

    #[test]
    fn embed_task_log_with_overlong_line_renders_without_panic() {
        let ctx = egui::Context::default();
        let mut app = StudioApp::default();
        let wide = format!("ERROR wide-line {}", "x".repeat(500));
        app.push_task_log(&wide);
        app.push_task_log("短い行");
        ctx.begin_pass(egui::RawInput::default());
        app.render_task_list(&mut child_ui(&ctx));
        let _ = ctx.end_pass();
        assert_eq!(app.task_log_lines().len(), 2);
    }

    #[test]
    fn embed_render_task_list_with_active_queue_completes_without_panic() {
        let ctx = egui::Context::default();
        let mut app = StudioApp::default();
        app.start_task_entries(vec![queue_entry("長時間", long_spec())]);
        ctx.begin_pass(egui::RawInput::default());
        app.render_task_list(&mut child_ui(&ctx));
        let _ = ctx.end_pass();
        app.abort_task_queue();
        ctx.begin_pass(egui::RawInput::default());
        app.render_task_list(&mut child_ui(&ctx));
        let _ = ctx.end_pass();
    }

    #[test]
    fn embed_render_task_list_with_detail_view_completes_without_panic() {
        let ctx = egui::Context::default();
        let mut app = StudioApp::default();
        app.load_task_list(&tasks_dir());
        app.toggle_task("launch");
        app.toggle_task("field_loop_pc");
        let root = StudioApp::workspace_root();
        ctx.begin_pass(egui::RawInput::default());
        let mut detail_ui = child_ui(&ctx);
        let list = app.task_defs.clone().unwrap();
        for def in list.definitions() {
            StudioApp::task_detail_ui(&mut detail_ui, def, "windows", None, &root);
        }
        app.render_task_list(&mut child_ui(&ctx));
        let _ = ctx.end_pass();
    }
}
