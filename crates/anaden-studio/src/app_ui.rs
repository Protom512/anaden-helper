//! StudioApp の UI 描画 impl 群 (Issue #162 Shard 1: app.rs 分割)。
//!
//! タスク一覧・modebar・Authoring/Batch 本体・ログビューアなど egui への
//! 描画処理のみを置く。状態定義は app_state.rs、状態操作 (キュー実行・
//! ファイル入出力) は app.rs。

use std::path::Path;
use std::sync::Arc;

use eframe::egui;

use anaden_core::ScreenRegion;

use crate::app_state::{
    AppMode, EngineKind, HEATMAP_DOWNSCALE, PipelineActionKind, STATE_OPTIONS, StudioApp,
};
use crate::batch;
use crate::canvas;
use crate::log_view::LogBuffer;
use crate::scoring;
use crate::source::LiveCapture;
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
            // UC-3: 詳細プレビューは実行と同じ引数解決条件 (target/serial/root)。
            let target = self.cli_target();
            let serial = Some(self.adb_serial.as_str());
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

impl eframe::App for StudioApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.render_modebar(ui);
        self.render_body(ui);
    }
}

impl StudioApp {
    /// ウィンドウ上限のモード切替バー（modebar）を描画する。
    ///
    /// 親レイアウト内への埋め込み（単一ウィンドウ統合 GUI, Issue #119）を想定した
    /// 公開パネル描画 API。単体テストからは [`Self::mode`] / [`Self::set_mode`]
    /// 経由で振る舞いを検証する。
    pub fn render_modebar(&mut self, ui: &mut egui::Ui) {
        // モード切替バー
        egui::Panel::top("modebar")
            .exact_size(30.0)
            .show_inside(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.mode, AppMode::Authoring, "作成");
                    ui.selectable_value(&mut self.mode, AppMode::Batch, "バッチ評価");
                });
            });
    }

    /// モード本体（Authoring / Batch）を親レイアウト内に描画する埋め込み用 API。
    ///
    /// modebar は含まない。呼び出し前に [`Self::render_modebar`] を実行するか、
    /// 親シェル側でタブ切替してもよい（mode は [`Self::set_mode`] で制御）。
    ///
    /// スクリーンショットのテクスチャ生成はここ（描画パスの入口）で行う。
    /// かつて [`Self::render_modebar`] 内にあったが、統合GUI シェル
    /// （Issue #119 `UnifiedShell`）は [`Self::render_body`] のみを呼ぶため、
    /// テクスチャが生成されずキャンバスが永久に空になる欠陥があった
    /// （作成タブで画像を開いても何も表示されない）。
    pub fn render_body(&mut self, ui: &mut egui::Ui) {
        // スクリーンショットのテクスチャ生成（未生成時）— 描画パスの入口で必ず走る。
        // 統合GUIシェル (UnifiedShell) 経由でも render_body は呼ばれるため、
        // どの起動経路でもキャンバスに画像が表示される (Issue #120 欠陥1修正)。
        if self.screenshot_tex.is_none()
            && let Some(img) = &self.screenshot
        {
            let rgba = img.to_rgba8();
            let size = [rgba.width() as usize, rgba.height() as usize];
            let color_image = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
            self.screenshot_tex = Some(ui.ctx().load_texture(
                "studio-screenshot",
                color_image,
                egui::TextureOptions::default(),
            ));
        }

        if matches!(self.mode, AppMode::Authoring) {
            // 別スレッドでの propose 計算結果を非ブロッキング受信。
            // 完了時: proposing を下ろし、結果を self.proposals へ反映・status 更新。
            if self.proposing
                && let Some(rx) = &self.proposal_rx
                && let Ok(ps) = rx.try_recv()
            {
                self.proposals = ps;
                self.proposing = false;
                self.proposal_rx = None;
                self.status = format!("ROI候補: {} 件（スコア順）", self.proposals.len());
            }

            // ライブADBキャプチャの最新フレームを取り込む（表示更新のみ。ROIは保持）
            if let Some(live) = &self.live
                && let Some(frame) = live.latest()
            {
                let normalized = self.scaler.normalize(&frame);
                self.screenshot = Some(Arc::new(normalized));
                self.screenshot_tex = None;
            }

            // 左サイドパネル: 操作 + 識別力サマリ
            egui::Panel::left("controls")
                .resizable(true)
                .default_size(320.0)
                .show_inside(ui, |ui| {
                    ui.heading("anaden-studio");
                    ui.label("テンプレート作成");
                    ui.separator();

                    ui.label("データ");
                    if ui.button("スクリーンショットを開く").clicked() {
                        self.open_screenshot();
                    }
                    ui.horizontal(|ui| {
                        if ui.button("正例フォルダ").clicked() {
                            self.load_positives();
                        }
                        ui.label(format!("{}枚", self.positives.len()));
                    });
                    ui.horizontal(|ui| {
                        if ui.button("負例フォルダ").clicked() {
                            self.load_negatives();
                        }
                        ui.label(format!("{}枚", self.negatives.len()));
                    });
                    ui.separator();

                    // 認識エンジン切替（ライブ比較）
                    ui.heading("認識エンジン");
                    ui.horizontal(|ui| {
                        ui.label("方式:");
                        // 借用回避: new_kind は self から Copy した値。
                        // 変更があればループ外（closure 脱出後）で switch する。
                        let mut new_kind = self.engine_kind;
                        egui::ComboBox::from_id_salt("engine_kind_combo")
                            .selected_text(match self.engine_kind {
                                EngineKind::Sse => "SSE（輝度差）",
                                EngineKind::Ccoeff => "CCOEFF（ロバスト）",
                            })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut new_kind,
                                    EngineKind::Sse,
                                    "SSE（輝度差）",
                                );
                                ui.selectable_value(
                                    &mut new_kind,
                                    EngineKind::Ccoeff,
                                    "CCOEFF（ロバスト）",
                                );
                            });
                        if new_kind != self.engine_kind {
                            self.switch_engine(new_kind);
                        }
                    });
                    ui.separator();

                    // ライブキャプチャ(android 実機 / PC版 Windows)
                    ui.heading("ライブキャプチャ");
                    // 接続状態サマリバッジ + チェックボタン + エラー理由パネル (Issue #139 T3)。
                    ui.colored_label(
                        self.connection.state.badge_color(),
                        self.connection.state.badge(),
                    );
                    if ui.button("接続チェック").clicked() {
                        self.run_connection_check();
                    }
                    ui.label(self.connection.reason_line());
                    // バックエンド選択。Windows バックエンドは Windows ビルドでのみ選択可能。
                    ui.horizontal(|ui| {
                        ui.label("取得元:");
                        ui.selectable_value(
                            &mut self.target,
                            crate::source::Target::Android,
                            "Android(adb)",
                        );
                        #[cfg(windows)]
                        ui.selectable_value(
                            &mut self.target,
                            crate::source::Target::Windows,
                            "Windows(PC版)",
                        );
                    });
                    // android は serial、windows は exe 名を入力。
                    match self.target {
                        crate::source::Target::Android => {
                            ui.horizontal(|ui| {
                                ui.label("serial:");
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.adb_serial)
                                        .desired_width(140.0),
                                );
                            });
                        }
                        #[cfg(windows)]
                        crate::source::Target::Windows => {
                            ui.horizontal(|ui| {
                                ui.label("exe名:");
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.win_exe)
                                        .desired_width(160.0),
                                );
                            });
                        }
                    }
                    if self.live.is_some() {
                        if ui.button("停止（この画面で固定）").clicked() {
                            self.live = None;
                            self.status = "ライブ停止: 現在の画面で固定しました".to_string();
                        }
                    } else {
                        // 開始可否: android は serial 必須、windows は exe 名必須。
                        let can_start = match self.target {
                            crate::source::Target::Android => !self.adb_serial.trim().is_empty(),
                            #[cfg(windows)]
                            crate::source::Target::Windows => !self.win_exe.trim().is_empty(),
                        };
                        ui.add_enabled_ui(can_start, |ui| {
                            if ui.button("ライブ開始").clicked() {
                                // android は serial、windows は exe 名を渡してバックエンドを分岐。
                                let serial = self.adb_serial.trim().to_string();
                                self.live = Some(LiveCapture::start(
                                    serial,
                                    800,
                                    self.target,
                                    self.win_exe.trim(),
                                ));
                                self.status = match self.target {
                                    crate::source::Target::Android => {
                                        "ライブキャプチャ中…".to_string()
                                    }
                                    #[cfg(windows)]
                                    crate::source::Target::Windows => {
                                        format!("PC版キャプチャ中… ({})", self.win_exe.trim())
                                    }
                                };
                            }
                        });
                    }
                    ui.separator();

                    // ROI自動提案
                    ui.heading("ROI候補");
                    // 計算中(self.proposing)はボタンを無効化（二重起動・多重ブロック防止）。
                    let can_propose = self.screenshot.is_some() && !self.proposing;
                    ui.add_enabled_ui(can_propose, |ui| {
                        let label = if self.proposing {
                            "ROI候補を計算中…"
                        } else {
                            "ROI候補を提案"
                        };
                        if ui.button(label).clicked() {
                            self.run_proposals();
                        }
                    });
                    if !self.proposals.is_empty() {
                        ui.label("クリックでROIに読込（その後スコアで検証）:");
                        // 借用チェック: ループ内で self.proposals を借用しつつ
                        // self.apply_proposal は呼べないため、クリック対象を退避し
                        // ループ外で適用する（canvas のドラッグROI更新と同パターン）。
                        let mut clicked: Option<ScreenRegion> = None;
                        for (i, p) in self.proposals.iter().enumerate() {
                            if ui
                                .small_button(format!(
                                    "[{i}] score {:.2}  ({},{}) {}x{}",
                                    p.score, p.roi.x, p.roi.y, p.roi.width, p.roi.height
                                ))
                                .clicked()
                            {
                                clicked = Some(p.roi);
                            }
                        }
                        if let Some(roi) = clicked {
                            self.apply_proposal(roi);
                        }
                    }
                    ui.separator();

                    // 識別力サマリ
                    ui.heading("識別力");
                    if let Some(d) = &self.discrimination {
                        let (verdict, color) = if d.margin() > 0.1 {
                            ("識別可能", egui::Color32::from_rgb(60, 180, 75))
                        } else if d.margin() > 0.0 {
                            ("微妙（要調整）", egui::Color32::from_rgb(230, 160, 30))
                        } else {
                            ("識別不可", egui::Color32::from_rgb(220, 60, 60))
                        };
                        ui.colored_label(color, format!("判定: {verdict}"));
                        ui.colored_label(
                            egui::Color32::from_rgb(60, 180, 75),
                            format!("正例最低: {:.3}", d.own_min),
                        );
                        ui.colored_label(
                            egui::Color32::from_rgb(220, 60, 60),
                            format!("負例最高: {:.3}", d.other_max),
                        );
                        ui.label(format!("マージン: {:+.3}", d.margin()));
                        ui.separator();
                        ui.label("正例スコア:");
                        for (i, s) in d.own_scores.iter().enumerate() {
                            ui.monospace(format!("  [{i}] {s:.3}"));
                        }
                        ui.label("負例スコア:");
                        for (i, s) in d.other_scores.iter().enumerate() {
                            ui.monospace(format!("  [{i}] {s:.3}"));
                        }
                    } else if let Some(r) = self.roi.rect() {
                        ui.label(format!("ROI: ({},{}) {}x{}", r.x, r.y, r.width, r.height));
                        ui.label("（評価中、または正例/負例未設定）");
                    } else {
                        ui.label("画面上でドラッグしてROIを選択");
                    }
                    ui.separator();

                    // テンプレート保存
                    ui.heading("保存");
                    ui.horizontal(|ui| {
                        ui.label("名前:");
                        ui.text_edit_singleline(&mut self.tpl_name);
                    });
                    ui.horizontal(|ui| {
                        ui.label("状態:");
                        egui::ComboBox::from_id_salt("state_combo")
                            .selected_text(STATE_OPTIONS[self.tpl_state_idx])
                            .show_ui(ui, |ui| {
                                for (i, s) in STATE_OPTIONS.iter().enumerate() {
                                    ui.selectable_value(&mut self.tpl_state_idx, i, *s);
                                }
                            });
                    });
                    ui.label(format!("保存先: {}", self.save_dir.display()));
                    if ui.button("保存先変更").clicked()
                        && let Some(dir) = rfd::FileDialog::new().pick_folder()
                    {
                        self.save_dir = dir;
                    }
                    let can_save = self.roi.rect().is_some() && self.screenshot.is_some();
                    let mut save_clicked = false;
                    ui.add_enabled_ui(can_save, |ui| {
                        if ui.button("テンプレート保存").clicked() {
                            save_clicked = true;
                        }
                    });
                    if save_clicked {
                        self.save_current_template();
                    }
                    ui.separator();

                    // pipeline task 保存 (UC-3: スクショ取り込み → ROI選択 →
                    // スコア計算 (scoring.rs) → pipeline task TOML 保存)。
                    // 同一の ROI/スコア/名前/状態入力を再利用し、保存形式のみ
                    // pipeline TOML (anaden-vision load_pipeline 互換) に切替。
                    ui.heading("pipeline task 保存");
                    ui.label(format!("保存先: {}", self.task_dir.display()));
                    if ui.button("task保存先変更").clicked()
                        && let Some(dir) = rfd::FileDialog::new().pick_folder()
                    {
                        self.task_dir = dir;
                    }
                    ui.horizontal(|ui| {
                        ui.label("action:");
                        egui::ComboBox::from_id_salt("task_action_combo")
                            .selected_text(self.task_action.label())
                            .show_ui(ui, |ui| {
                                for kind in [
                                    PipelineActionKind::ClickSelf,
                                    PipelineActionKind::DoNothing,
                                    PipelineActionKind::Stop,
                                ] {
                                    ui.selectable_value(&mut self.task_action, kind, kind.label());
                                }
                            });
                    });
                    let can_save_task = self.roi.rect().is_some() && self.screenshot.is_some();
                    let mut task_save_clicked = false;
                    ui.add_enabled_ui(can_save_task, |ui| {
                        if ui.button("pipeline task として保存").clicked() {
                            task_save_clicked = true;
                        }
                    });
                    if task_save_clicked {
                        self.save_current_pipeline_task();
                    }
                    ui.separator();

                    // シナリオ作成 (Issue #160 T3 / UC-1+UC-2): 単一 TaskDef 保存 (上)
                    // を多 TaskDef + manifest 保存へ拡張する collapsing セクション。
                    // パネル本体は scenario_ui (ドメイン)、ここは配線のみ。
                    // UC-3 (Shard 4): 保存済み pipeline をタスクへ登録・有効化する
                    // サブフロー (ui_task_link) も配線する。stub 一覧は読込済み
                    // タスク定義から導出 (未読込なら既定パスから遅延読込)。
                    self.ensure_task_list_loaded();
                    let link_root = Self::workspace_root();
                    let link_tasks_dir = link_root.join("templates/tasks");
                    let stubs = self
                        .task_defs
                        .as_ref()
                        .map(|list| crate::scenario_task_link::stub_options(list.definitions()))
                        .unwrap_or_default();
                    let scenario_candidate = self.scenario_candidate();
                    let mut task_event = None;
                    egui::CollapsingHeader::new("シナリオ作成")
                        .default_open(true)
                        .show(ui, |ui| {
                            self.scenario.ui(ui, scenario_candidate, &mut self.status);
                            let link_ctx = crate::scenario_task_link::TaskLinkContext {
                                root: &link_root,
                                tasks_dir: &link_tasks_dir,
                                stubs: &stubs,
                            };
                            task_event =
                                self.scenario.ui_task_link(ui, &link_ctx, &mut self.status);
                        });
                    if let Some(event) = task_event {
                        self.on_scenario_task_event(event);
                    }
                    ui.separator();
                    ui.label(&self.status);
                });

            // 中央: キャンバス
            egui::CentralPanel::default().show_inside(ui, |ui| {
                if let (Some(tex), Some(img)) = (&self.screenshot_tex, &self.screenshot) {
                    let (w, h) = (img.width(), img.height());

                    // 既存のヒートマップを描画に渡す（ROI解放時に更新される）
                    let heatmap_view = self.heatmap_tex.as_ref().map(|t| canvas::HeatmapView {
                        tex: t.id(),
                        search: self.heatmap_search,
                    });
                    let best_match = self.best_match;
                    canvas::show(
                        ui,
                        tex,
                        w,
                        h,
                        &mut self.roi,
                        heatmap_view.as_ref(),
                        best_match,
                    );

                    // ROIが安定して変化したら識別力とヒートマップを再評価
                    if let Some(roi_rect) = self.roi.rect()
                        && !self.roi.dragging
                        && Some(roi_rect) != self.scored_roi
                    {
                        let crop =
                            img.crop_imm(roi_rect.x, roi_rect.y, roi_rect.width, roi_rect.height);
                        self.discrimination = Some(scoring::discrimination(
                            self.engine.as_ref(),
                            &crop,
                            &self.positives,
                            &self.negatives,
                        ));

                        // ヒートマップ（スコアマップ全体）と最良マッチ位置
                        if let Some(sm) = self.heatmap_engine.score_map(img, &crop) {
                            let mut bx = 0u32;
                            let mut by = 0u32;
                            let mut bv = 0u8;
                            for y in 0..sm.height() {
                                for x in 0..sm.width() {
                                    let v = sm.get_pixel(x, y)[0];
                                    if v > bv {
                                        bv = v;
                                        bx = x;
                                        by = y;
                                    }
                                }
                            }
                            let d = HEATMAP_DOWNSCALE;
                            self.best_match = Some(ScreenRegion::new(
                                bx * d,
                                by * d,
                                roi_rect.width,
                                roi_rect.height,
                            ));
                            self.heatmap_search = ScreenRegion::new(
                                0,
                                0,
                                img.width().saturating_sub(roi_rect.width),
                                img.height().saturating_sub(roi_rect.height),
                            );
                            let color_img = canvas::score_map_to_heatmap(&sm);
                            self.heatmap_tex = Some(ui.ctx().load_texture(
                                "heatmap",
                                color_img,
                                egui::TextureOptions::LINEAR,
                            ));
                        }

                        self.scored_roi = Some(roi_rect);
                    }
                } else {
                    ui.heading("「スクリーンショットを開く」で画像を読み込んでください");
                }
            });
        } else {
            self.batch_ui(ui);
        }
    }
}

impl StudioApp {
    /// バッチ評価モードのUI。
    fn batch_ui(&mut self, ui: &mut egui::Ui) {
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

    /// 埋め込み描画 API（render_modebar + render_body）が Authoring モードで
    /// パニックせず完了することを検証する（Issue #119 shard 1 task 2）。
    #[test]
    fn embed_render_authoring_mode_completes_without_panic() {
        let ctx = egui::Context::default();
        let mut app = StudioApp::default();
        assert_eq!(app.mode(), AppMode::Authoring);
        ctx.begin_pass(egui::RawInput::default());
        app.render_modebar(&mut child_ui(&ctx));
        app.render_body(&mut child_ui(&ctx));
        let _ = ctx.end_pass();
    }

    /// 埋め込み描画 API が Batch モード（混同行列 UI 含む）でも壊れないことを検証する。
    #[test]
    fn embed_render_batch_mode_completes_without_panic() {
        let ctx = egui::Context::default();
        let mut app = StudioApp::default();
        app.set_mode(AppMode::Batch);
        assert_eq!(app.mode(), AppMode::Batch);
        ctx.begin_pass(egui::RawInput::default());
        app.render_modebar(&mut child_ui(&ctx));
        app.render_body(&mut child_ui(&ctx));
        let _ = ctx.end_pass();
    }

    /// 接続バッジ・チェックボタン・エラー理由パネルを含む Authoring 描画が
    /// パニックせず完了すること (Issue #139 T3)。
    #[test]
    fn embed_render_connection_panel_completes_without_panic() {
        let ctx = egui::Context::default();
        let mut app = StudioApp::default();
        app.run_connection_check();
        ctx.begin_pass(egui::RawInput::default());
        app.render_modebar(&mut child_ui(&ctx));
        app.render_body(&mut child_ui(&ctx));
        let _ = ctx.end_pass();
    }

    /// UI のボタンラベルに Unicode 絵文字が残っていないこと (豆腐排除・機械検証)。
    #[test]
    fn app_button_labels_contain_no_emoji() {
        let labels = [
            "作成",
            "バッチ評価",
            "スクリーンショットを開く",
            "正例フォルダ",
            "負例フォルダ",
            "Android(adb)",
            "停止（この画面で固定）",
            "ライブ開始",
            "ROI候補を提案",
            "テンプレート保存",
            "保存先変更",
            "実行",
        ];
        for l in labels {
            assert!(
                l.chars().all(|c| c < '\u{1F300}'),
                "label must not contain emoji: {l}"
            );
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
