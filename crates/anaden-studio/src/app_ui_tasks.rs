//! StudioApp の Tasks ペイン描画 (Issue #174: app_ui.rs 分割)。
//!
//! タスク一覧 (MAA 型チェックボックス)・タスク詳細 (UC-3)・実行ログ
//! ビューアの描画 impl。状態定義は app_state.rs、状態操作 (キュー実行・
//! ファイル入出力) は app.rs。

use std::path::Path;

use eframe::egui;

use crate::app_state::StudioApp;
use crate::log_view::LogBuffer;
use crate::tasks::{self, QueueState};

impl StudioApp {
    /// タスク一覧 UI (MAA 型チェックボックス) を描画する。
    /// implemented=false はグレー表示・チェック不可 (嘘の動作可能表示禁止)。
    /// UC-4: 毎フレーム drain によるリアルタイム進行表示 (i/N + チェック順
    /// キュー)・ログ表示・失敗時の明示的な「継続」「停止」ボタンを含む。
    pub fn render_task_list(&mut self, ui: &mut egui::Ui) {
        ui.heading("タスク一覧");
        // 毎フレーム drain (UC-4: Exit 観測がキュー進行の唯一の契機)。
        self.drain_task_logs();
        if self.task_defs.is_none() {
            if ui.button("タスク定義を読み込む").clicked() {
                self.load_task_list(&Self::workspace_root().join("templates/tasks"));
            }
        } else if let Some(list) = self.task_defs.clone() {
            // UC-3: 詳細プレビューは実行と同じ引数解決条件 (target/root)。
            // serial は Issue #188 の Android 削除で廃止 — 常に None。
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
                    // UC-3: 選択済みなら実行順位置を横に表示 (未選択は非表示)。
                    if let Some(pos) = tasks::queue_position_label(&selected, &def.id) {
                        ui.weak(pos);
                    }
                });
                if checked != list.is_selected(&def.id) {
                    clicked = Some(def.id.clone());
                }
                // UC-3: 展開可能な詳細表示 (kind・pipeline_dir・start_task・
                // 引数プレビュー — 読み取り専用・schema 変更なし)。
                egui::CollapsingHeader::new(egui::RichText::new("詳細").weak())
                    .id_salt(&def.id)
                    .show(ui, |ui| {
                        Self::task_detail_ui(ui, def, target, serial, &root);
                    });
            }
            if let Some(id) = clicked {
                self.toggle_task(&id);
            }
            // 開始ボタンはキュー非アクティブ時のみ有効 (実行中の再開始拒否)。
            let can_start = list.selected_count() > 0 && !self.task_queue_active();
            ui.add_enabled_ui(can_start, |ui| {
                if ui.button("開始").clicked() {
                    self.start_task_queue();
                }
            });
            // UC-3: 選択済みキューの実行順リスト (チェック順 1. 2. 3. ...・
            // 未実装 (不整合検出時) はグレー表示)。
            let rows = tasks::queue_order_rows(&selected, list.definitions());
            if !rows.is_empty() {
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
        }
        // UC-4: 進行サマリ + 実行制御 + チェック順キュー一覧。
        if let Some(queue) = self.task_queue.clone() {
            ui.separator();
            ui.label(queue.summary());
            match queue.state() {
                QueueState::Running { .. } => {
                    if ui.button("中止").clicked() {
                        self.abort_task_queue();
                    }
                }
                QueueState::PausedAfterFailure { .. } => {
                    ui.colored_label(egui::Color32::RED, "タスクが失敗しました。継続しますか?");
                    if ui.button("継続").clicked() {
                        self.resume_task_queue();
                    }
                    if ui.button("停止").clicked() {
                        self.abort_task_queue();
                    }
                }
                QueueState::Pending | QueueState::Completed => {}
            }
            for (i, entry) in queue.entries().iter().enumerate() {
                ui.label(format!(
                    "{}. [{}] {}",
                    i + 1,
                    queue.entry_marker(i),
                    entry.label
                ));
            }
        }
        ui.separator();
        self.task_log_ui(ui);
        ui.separator();
        ui.label(&self.status);
    }

    /// UC-3: タスク 1 件の詳細表示ボディ (collapsing header 配下・読み取り専用)。
    ///
    /// kind・pipeline_dir・start_task (未宣言時は解決結果)・実引数プレビューを
    /// 表示する。引数解決は [`tasks::task_detail_view`] (実行の [`tasks::spawn_args`]
    /// と単一情報源)。未実装タスクは赤字で理由を表示 (fail-closed)。
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
                // pipeline_run なのに解決不能 = 実行不可 (fail-closed 表示)。
                ui.colored_label(egui::Color32::RED, "start_task: 未解決");
            }
            None => {} // launch_subcommand は start_task を使用しない
        }
        ui.label(format!("引数プレビュー: {}", view.args_preview()));
        if let Some(reason) = &view.unimplemented_reason {
            ui.colored_label(egui::Color32::RED, format!("未実装: {reason}"));
        }
    }

    /// Tasks ペインの実行ログビューア (log_view.rs の LogBuffer/AutoScroll 再利用)。
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
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .stick_to_bottom(stick)
            .show(ui, |ui| {
                if self.task_log_snapshot.is_empty() {
                    ui.weak("（ログなし）");
                }
                for entry in &self.task_log_snapshot {
                    ui.monospace(
                        egui::RichText::new(&entry.line)
                            .monospace()
                            .color(crate::runner::level_color(entry.level)),
                    );
                }
            });
        if stick {
            // stick_to_bottom が有効な間は egui が末尾へ張り付くため追従清算する。
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

    /// ヘッドレス egui コンテキストを用意し、その中に子 Ui を作る。
    /// GUI バックエンド不要でパネル描画を単体テストできる。
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

    /// 長時間 (約30秒) 生きる子の SpawnSpec (ping は両 OS に存在)。
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

    /// タスク一覧 UI (チェックボックス・開始ボタン含む) がパニックせず描画できる。
    #[test]
    fn embed_render_task_list_completes_without_panic() {
        let ctx = egui::Context::default();
        let mut app = StudioApp::default();
        app.load_task_list(&tasks_dir());
        ctx.begin_pass(egui::RawInput::default());
        app.render_task_list(&mut child_ui(&ctx));
        let _ = ctx.end_pass();
    }

    /// UC-4: キュー実行中の描画 (進行表示・失敗ボタン・ログ) もパニックしない。
    #[test]
    fn embed_render_task_list_with_active_queue_completes_without_panic() {
        let ctx = egui::Context::default();
        let mut app = StudioApp::default();
        app.start_task_entries(vec![queue_entry("長時間", long_spec())]);
        ctx.begin_pass(egui::RawInput::default());
        app.render_task_list(&mut child_ui(&ctx));
        let _ = ctx.end_pass();
        app.abort_task_queue();
        // 中止後の描画も安定していること。
        ctx.begin_pass(egui::RawInput::default());
        app.render_task_list(&mut child_ui(&ctx));
        let _ = ctx.end_pass();
    }

    /// UC-3: 詳細展開ビュー (kind/pipeline_dir/start_task/引数プレビュー) を
    /// 含む描画がパニックなく完了する。collapsing header 展開時に描画される
    /// ボディを全タスク分直接描画 + 選択済み一覧 (実行順リスト含む) 全体描画。
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
