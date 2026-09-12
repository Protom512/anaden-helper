//! StudioApp の作成 (Authoring) モード本体描画 (Issue #174: app_ui.rs 分割)。
//!
//! テンプレート作成ワークフロー (データ読込・認識エンジン切替・ライブ
//! キャプチャ・ROI候補・識別力サマリ・テンプレート/pipeline task 保存・
//! シナリオ作成配線) と中央キャンバス (ヒートマップ・最良マッチ) の描画。
//! モードディスパッチは app_ui_body の [`StudioApp::render_body`]。

use std::sync::Arc;

use eframe::egui;

use anaden_core::ScreenRegion;

use crate::app_state::{
    EngineKind, HEATMAP_DOWNSCALE, PipelineActionKind, STATE_OPTIONS, StudioApp,
};
use crate::canvas;
use crate::scoring;
use crate::source::LiveCapture;

impl StudioApp {
    /// 作成 (Authoring) モードの本体を描画する。
    ///
    /// [`Self::render_body`] の Authoring 分岐から呼ばれる (Issue #174 で
    /// app_ui.rs から抽出・コード移動のみ)。別スレッドでの propose 計算結果の
    /// 受信とライブADBキャプチャの最新フレーム取り込みを行ってから、左サイド
    /// パネル (操作 + 識別力サマリ + 保存) と中央キャンバスを描画する。
    pub(crate) fn render_authoring(&mut self, ui: &mut egui::Ui) {
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
                            ui.selectable_value(&mut new_kind, EngineKind::Sse, "SSE（輝度差）");
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

                // ライブキャプチャ (PC版 Windows — Issue #188 で Android 経路は削除)
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
                ui.horizontal(|ui| {
                    ui.label("取得元:");
                    ui.label("Windows(PC版) — Win32Capture");
                });
                ui.horizontal(|ui| {
                    ui.label("exe名:");
                    ui.add(egui::TextEdit::singleline(&mut self.win_exe).desired_width(160.0));
                });
                if self.live.is_some() {
                    if ui.button("停止（この画面で固定）").clicked() {
                        self.live = None;
                        self.status = "ライブ停止: 現在の画面で固定しました".to_string();
                    }
                } else {
                    // 開始可否: exe 名必須 (Win32 キャプチャは exe 名でプロセス解決)。
                    let can_start = !self.win_exe.trim().is_empty();
                    ui.add_enabled_ui(can_start, |ui| {
                        if ui.button("ライブ開始").clicked() {
                            self.live = Some(LiveCapture::start(800, self.win_exe.trim()));
                            self.status = format!("PC版キャプチャ中… ({})", self.win_exe.trim());
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
                        task_event = self.scenario.ui_task_link(ui, &link_ctx, &mut self.status);
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
    }
}
