//! StudioApp: GUI 全体状態と eframe::App 実装。
//!
//! 左パネル（操作・識別力サマリ）と中央キャンバス（画像＋ROI選択）で構成。
//! ROIが確定（ドラッグ解放）するたび、候補テンプレートを正例/負例で評価する。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui;
use image::DynamicImage;

use anaden_core::{MatchConfidence, ScreenRegion};
use anaden_vision::{
    CcoeffVisionEngine, ScreenScaler, SseVisionEngine, TemplateMatcher, VisionEngine,
};

use crate::batch::{self, ConfusionMatrix};
use crate::canvas::{self, RoiEdit};
use crate::library::{self, TemplateSpec};
use crate::proposals::{self, Proposal};
use crate::scoring::{self, Discrimination};
use crate::source::LiveCapture;

/// ヒートマップ計算用のダウンスケール倍率。
/// imageproc の match_template は O(W·H·w·h) の総当たりのため、フル解像度では重い。
/// 4倍縮小で速度と位置精度を両立する（位置精度 ±4px）。
const HEATMAP_DOWNSCALE: u32 = 4;

/// テンプレート保存時の状態選択肢。TemplateStore の parse_state_from_dir_name と整合。
const STATE_OPTIONS: &[&str] = &[
    "title",
    "field",
    "loading",
    "battle",
    "fishing",
    "menu",
    "dialog",
    "unknown",
];

/// GUI のモード。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppMode {
    /// テンプレート作成（ROI選択＋識別力評価）。
    Authoring,
    /// バッチ評価（混同行列）。
    Batch,
}

/// 識別力評価に使うマッチエンジン。コンボでライブ切替する。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EngineKind {
    /// imageproc 正規化SSE（絶対輝度差）。現行ベースライン。
    Sse,
    /// TM_CCOEFF_NORMED（輝度シフトにロバスト）。
    Ccoeff,
}

impl Default for EngineKind {
    fn default() -> Self {
        // よりロバストな方をデフォルト。
        EngineKind::Ccoeff
    }
}

/// GUI 全体の状態。
pub struct StudioApp {
    /// 編集中のスクリーンショット。
    screenshot: Option<Arc<DynamicImage>>,
    /// スクリーンショットの表示用テクスチャ。
    screenshot_tex: Option<egui::TextureHandle>,
    /// ドラッグROI編集状態。
    roi: RoiEdit,
    /// 最後にスコア計算したROI（変化検出用）。
    scored_roi: Option<ScreenRegion>,
    /// 正例画像（同じ画面状態）。フォルダ単位で読込。
    positives: Vec<Arc<DynamicImage>>,
    /// 負例画像（別画面状態）。
    negatives: Vec<Arc<DynamicImage>>,
    /// 直近の識別力評価結果。
    discrimination: Option<Discrimination>,
    /// 現在選択中のエンジン種別（コンボで切替）。engine 再構築の基。
    engine_kind: EngineKind,
    /// 認識エンジン（閾値0・1/2ダウンスケールで生スコアを高速に返す）。
    engine: Box<dyn VisionEngine>,
    /// ヒートマップ計算用エンジン（閾値0・1/4ダウンスケールでスコアマップ全体を算出）。
    heatmap_engine: Box<dyn VisionEngine>,
    /// ヒートマップテクスチャ（ROI解放時に更新）。
    heatmap_tex: Option<egui::TextureHandle>,
    /// ヒートマップが対応する探索領域（元画像座標）。
    heatmap_search: ScreenRegion,
    /// テンプレートの最良マッチ位置（元画像座標・ROI解放時に更新）。
    best_match: Option<ScreenRegion>,
    /// 保存時のテンプレート名入力。
    tpl_name: String,
    /// 保存時の状態選択（STATE_OPTIONS のインデックス）。
    tpl_state_idx: usize,
    /// テンプレート保存先ディレクトリ。
    save_dir: PathBuf,
    /// 現在のモード。
    mode: AppMode,
    /// バッチ評価のテストフォルダ（<dir>/<label>/*.png）。
    test_dir: PathBuf,
    /// バッチ評価の決定閾値。
    batch_threshold: f32,
    /// バッチ評価結果。
    batch_result: Option<ConfusionMatrix>,
    /// ADB デバイスシリアル（ライブキャプチャ用）。
    adb_serial: String,
    /// ライブキャプチャ（稼働中のみ）。
    live: Option<LiveCapture>,
    /// 720p 基準への解像度正規化スケーラ（TASK-009）。
    scaler: ScreenScaler,
    /// ROI自動提案の候補リスト（💡ボタン押下で生成）。
    proposals: Vec<Proposal>,
    /// ステータスメッセージ。
    status: String,
}

impl Default for StudioApp {
    fn default() -> Self {
        // engine は engine_kind（デフォルト CCOEFF）から構築。閾値0・ダウンスケール2。
        let default_kind = EngineKind::default();
        Self {
            screenshot: None,
            screenshot_tex: None,
            roi: RoiEdit::default(),
            scored_roi: None,
            positives: vec![],
            negatives: vec![],
            discrimination: None,
            engine_kind: default_kind,
            engine: StudioApp::build_engine(default_kind),
            heatmap_engine: Box::new(SseVisionEngine::new(TemplateMatcher::new(
                MatchConfidence::new(0.0),
                HEATMAP_DOWNSCALE,
            ))),
            heatmap_tex: None,
            heatmap_search: ScreenRegion::new(0, 0, 0, 0),
            best_match: None,
            tpl_name: String::from("template_01"),
            tpl_state_idx: 0,
            save_dir: PathBuf::from("./templates/scenes"),
            mode: AppMode::Authoring,
            test_dir: PathBuf::from("./templates/tests"),
            batch_threshold: 0.5,
            batch_result: None,
            adb_serial: String::new(),
            live: None,
            scaler: ScreenScaler::new(),
            proposals: vec![],
            status: String::from("スクリーンショットと正例/負例フォルダを読み込んでください"),
        }
    }
}

impl StudioApp {
    /// engine_kind から生スコア評価用エンジンを構築する（downscale=2, 閾値0）。
    /// 両エンジンで条件を統一し公平比較を保証する純関数。
    fn build_engine(kind: EngineKind) -> Box<dyn VisionEngine> {
        match kind {
            EngineKind::Sse => Box::new(SseVisionEngine::new(TemplateMatcher::new(
                MatchConfidence::new(0.0),
                2,
            ))),
            EngineKind::Ccoeff => Box::new(CcoeffVisionEngine::new(
                MatchConfidence::new(0.0),
                2,
            )),
        }
    }

    /// エンジン種別を切替え、self.engine を再構築し、再評価を強制する。
    /// downscale=2・閾値0 で現行 scoring engine と同じ条件（公平比較）。
    /// scored_roi / discrimination を None に戻すことで、次フレームの
    /// CentralPanel 再評価ブロックが新エンジンで discrimination を再計算する。
    fn switch_engine(&mut self, kind: EngineKind) {
        self.engine_kind = kind;
        self.engine = StudioApp::build_engine(kind);
        self.scored_roi = None; // 次フレームで再評価を強制
        self.discrimination = None; // 古いスコアを即クリア（チラつき防止）
        self.status = format!(
            "エンジン切替: {}",
            match kind {
                EngineKind::Sse => "SSE（輝度差ベース）",
                EngineKind::Ccoeff => "CCOEFF（ロバスト・輝度シフト不変）",
            }
        );
    }

    /// ファイルダイアログでスクリーンショットを開く。
    fn open_screenshot(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("画像", &["png", "jpg", "jpeg", "bmp"])
            .pick_file()
        {
            match image::open(&path) {
                Ok(img) => {
                    self.status = format!(
                        "スクリーンショット: {}x{} → 720p基準で正規化",
                        img.width(),
                        img.height()
                    );
                    let normalized = self.scaler.normalize(&img);
                    self.screenshot = Some(Arc::new(normalized));
                    self.screenshot_tex = None; // 再生成
                    self.roi = RoiEdit::default();
                    self.scored_roi = None;
                    self.discrimination = None;
                    self.heatmap_tex = None;
                    self.best_match = None;
                    self.proposals = vec![];
                }
                Err(e) => self.status = format!("読込失敗: {e}"),
            }
        }
    }

    /// 正例フォルダを読み込む。
    fn load_positives(&mut self) {
        if let Some(dir) = rfd::FileDialog::new().pick_folder() {
            let imgs = load_folder(&dir);
            self.status = format!("正例: {} 枚読込", imgs.len());
            self.positives = imgs;
            self.scored_roi = None; // 再評価を強制
        }
    }

    /// 負例フォルダを読み込む。
    fn load_negatives(&mut self) {
        if let Some(dir) = rfd::FileDialog::new().pick_folder() {
            let imgs = load_folder(&dir);
            self.status = format!("負例: {} 枚読込", imgs.len());
            self.negatives = imgs;
            self.scored_roi = None;
        }
    }

    /// 現在のROI切り出しをテンプレートとして保存する。
    /// 閾値は識別力があれば正例/負例スコアの中間、なければ 0.9。
    fn save_current_template(&mut self) {
        let (Some(img), Some(roi)) = (self.screenshot.clone(), self.roi.rect()) else {
            return;
        };
        let crop = img.crop_imm(roi.x, roi.y, roi.width, roi.height);
        let threshold = self
            .discrimination
            .as_ref()
            .map(|d| ((d.own_min + d.other_max) / 2.0).clamp(0.5, 0.99))
            .unwrap_or(0.9);
        let spec = TemplateSpec {
            name: self.tpl_name.clone(),
            state: STATE_OPTIONS[self.tpl_state_idx].to_string(),
            roi,
            threshold,
            // TODO(第2段): engine_kind に連動させる（Sse => "sse", Ccoeff => "ccoeff"）。
            // TemplateStore 側の method 文字列仕様に依存するため、第一段では固定。
            method: "sse".to_string(),
        };
        match library::save_template(&self.save_dir, &spec, &crop) {
            Ok(p) => self.status = format!("保存: {}", p.display()),
            Err(e) => self.status = format!("保存失敗: {e}"),
        }
    }

    /// 現在のスクリーンショットからROI候補を提案する。
    ///
    /// heatmap_engine を転用（閾値0・1/4ダウンスケール・score_map 使用可）。
    /// propose は同期的に走る（ヒートマップ計算と同様。初版はこれでよい）。
    fn run_proposals(&mut self) {
        let Some(img) = self.screenshot.clone() else {
            self.status = "スクリーンショットを先に読み込んでください".to_string();
            return;
        };
        self.status = "ROI候補を計算中…".to_string();
        let ps = proposals::propose(
            self.heatmap_engine.as_ref(),
            &img,
            96, // tile_w
            96, // tile_h
            96, // step（ノーオーバーラップ）
            12, // max_n
        );
        self.status = format!("ROI候補: {} 件（スコア順）", ps.len());
        self.proposals = ps;
    }

    /// 候補ROIをドラッグROI編集状態に読み込む。
    ///
    /// RoiEdit::rect() は width = x1 - x0 で矩形を復元するため、
    /// 候補 roi (x,y,w,h) を正確に再現するには current を (x+w, y+h) = (right(), bottom())
    /// に設定する（right-1 だと width が1つ減る）。dragging=false で確定状態にする。
    /// scored_roi を None に戻し、既存の再評価トリガで識別力スコアを自動再計算させる。
    fn apply_proposal(&mut self, roi: ScreenRegion) {
        self.roi.anchor = Some((roi.x, roi.y));
        self.roi.current = Some((roi.right(), roi.bottom()));
        self.roi.dragging = false;
        // 既存の再評価トリガを発火させるため、scored_roi を古い値に戻す。
        self.scored_roi = None;
    }
}

impl eframe::App for StudioApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // スクリーンショットのテクスチャ生成（未生成時）
        if self.screenshot_tex.is_none() {
            if let Some(img) = &self.screenshot {
                let rgba = img.to_rgba8();
                let size = [rgba.width() as usize, rgba.height() as usize];
                let color_image =
                    egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
                self.screenshot_tex = Some(ui.ctx().load_texture(
                    "studio-screenshot",
                    color_image,
                    egui::TextureOptions::default(),
                ));
            }
        }

        // モード切替バー
        egui::Panel::top("modebar")
            .exact_size(30.0)
            .show_inside(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.mode, AppMode::Authoring, "✏️ 作成");
                    ui.selectable_value(&mut self.mode, AppMode::Batch, "📊 バッチ評価");
                });
            });

        if matches!(self.mode, AppMode::Authoring) {
            // ライブADBキャプチャの最新フレームを取り込む（表示更新のみ。ROIは保持）
            if let Some(live) = &self.live {
                if let Some(frame) = live.latest() {
                    let normalized = self.scaler.normalize(&frame);
                    self.screenshot = Some(Arc::new(normalized));
                    self.screenshot_tex = None;
                }
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
                if ui.button("📸 スクリーンショットを開く").clicked() {
                    self.open_screenshot();
                }
                ui.horizontal(|ui| {
                    if ui.button("✅ 正例フォルダ").clicked() {
                        self.load_positives();
                    }
                    ui.label(format!("{}枚", self.positives.len()));
                });
                ui.horizontal(|ui| {
                    if ui.button("❌ 負例フォルダ").clicked() {
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

                // ライブADBキャプチャ
                ui.heading("ライブADB");
                ui.horizontal(|ui| {
                    ui.label("serial:");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.adb_serial).desired_width(140.0),
                    );
                });
                if self.live.is_some() {
                    if ui.button("⏹ 停止（この画面で固定）").clicked() {
                        self.live = None;
                        self.status = "ライブ停止: 現在の画面で固定しました".to_string();
                    }
                } else {
                    let serial = self.adb_serial.trim().to_string();
                    ui.add_enabled_ui(!serial.is_empty(), |ui| {
                        if ui.button("▶ ライブ開始").clicked() {
                            self.live = Some(LiveCapture::start(serial, 800));
                            self.status = "ライブキャプチャ中…".to_string();
                        }
                    });
                }
                ui.separator();

                // ROI自動提案
                ui.heading("ROI候補");
                let can_propose = self.screenshot.is_some();
                ui.add_enabled_ui(can_propose, |ui| {
                    if ui.button("💡 ROI候補を提案").clicked() {
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
                    ui.label(format!(
                        "ROI: ({},{}) {}x{}",
                        r.x, r.y, r.width, r.height
                    ));
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
                if ui.button("📁 保存先変更").clicked() {
                    if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                        self.save_dir = dir;
                    }
                }
                let can_save = self.roi.rect().is_some() && self.screenshot.is_some();
                let mut save_clicked = false;
                ui.add_enabled_ui(can_save, |ui| {
                    if ui.button("💾 テンプレート保存").clicked() {
                        save_clicked = true;
                    }
                });
                if save_clicked {
                    self.save_current_template();
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
                canvas::show(ui, tex, w, h, &mut self.roi, heatmap_view.as_ref(), best_match);

                // ROIが安定して変化したら識別力とヒートマップを再評価
                if let Some(roi_rect) = self.roi.rect() {
                    if !self.roi.dragging && Some(roi_rect) != self.scored_roi {
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
                if ui.button("📁 テスト元変更").clicked() {
                    if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                        self.test_dir = dir;
                    }
                }
                ui.horizontal(|ui| {
                    ui.label("閾値:");
                    ui.add(egui::Slider::new(&mut self.batch_threshold, 0.0..=1.0));
                });
                let mut run_clicked = false;
                if ui.button("▶ 実行").clicked() {
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
                ui.heading(format!("混同行列（正答率 {:.1}%）", cm.accuracy() * 100.0));
                ui.label(format!("テスト画像 {} 枚 / 状態 {} 種", cm.total, cm.labels.len()));
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
                egui::Grid::new("per_template").striped(true).show(ui, |ui| {
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
            } else {
                ui.heading("「▶ 実行」でバッチ評価を行います");
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
        let cm = batch::evaluate(self.engine.as_ref(), &templates, &tests, self.batch_threshold);
        self.status = format!(
            "完了: 正答率 {:.1}% ({} テンプレ × {} テスト)",
            cm.accuracy() * 100.0,
            templates.len(),
            tests.len()
        );
        self.batch_result = Some(cm);
    }
}

/// フォルダ内の画像をすべて読み込む。
fn load_folder(path: &Path) -> Vec<Arc<DynamicImage>> {
    let mut out = vec![];
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            let p: PathBuf = entry.path();
            if is_image(&p) {
                if let Ok(img) = image::open(&p) {
                    out.push(Arc::new(img));
                }
            }
        }
    }
    out
}

fn is_image(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()).map(|e| e.to_lowercase()).as_deref(),
        Some("png") | Some("jpg") | Some("jpeg") | Some("bmp")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, GrayImage, Luma};

    /// 非一様・非周期な needle。CCOEFF は一様パッチ（denomT=0）で全位置 0 を返すため、
    /// build_engine の構築健全性検証には内部分散を持つ一意パターンが必要。
    /// 値 = ((x*x + 3*y) % 200) + 20 で 20..=219 の非周期パターンを作る。
    fn textured_needle(w: u32, h: u32) -> GrayImage {
        let mut img = GrayImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let v = (((x * x + 3 * y) % 200) + 20) as u8;
                img.put_pixel(x, y, Luma([v]));
            }
        }
        img
    }

    /// 単色背景 (ox, oy) に needle を埋め込んだ画像。
    fn embed_on_bg(
        haystack_w: u32,
        haystack_h: u32,
        needle: &GrayImage,
        ox: u32,
        oy: u32,
        bg: u8,
    ) -> GrayImage {
        let mut img = GrayImage::from_pixel(haystack_w, haystack_h, Luma([bg]));
        for y in 0..needle.height() {
            for x in 0..needle.width() {
                let p = needle.get_pixel(x, y)[0];
                img.put_pixel(ox + x, oy + y, Luma([p]));
            }
        }
        img
    }

    fn luma_dyn(img: GrayImage) -> DynamicImage {
        DynamicImage::ImageLuma8(img)
    }

    #[test]
    fn engine_kind_default_is_ccoeff() {
        assert_eq!(EngineKind::default(), EngineKind::Ccoeff);
    }

    #[test]
    fn build_engine_produces_ccoeff_by_default() {
        // デフォルトエンジンは CCOEFF。構築できること（panic しない）が最小保証。
        let _engine = StudioApp::build_engine(EngineKind::default());
        let _sse = StudioApp::build_engine(EngineKind::Sse);
    }

    /// build_engine が downscale=2・閾値0 で健全に構築されていることを、
    /// 両エンジンで同一画像のマッチを返すことでエンドツーエンド検証する。
    /// 黒四角 on 白背景は一意パターンで、downscale=2 でも位置が ±2px で確定する。
    #[test]
    fn build_engine_both_engines_locate_embedded_needle() {
        let needle = textured_needle(20, 20);
        // 中間グレー背景に埋め込み（needle は非周期・非一意）。
        let haystack = embed_on_bg(100, 100, &needle, 40, 40, 128);
        let haystack_dyn = luma_dyn(haystack);
        let needle_dyn = luma_dyn(needle.clone());

        let sse = StudioApp::build_engine(EngineKind::Sse);
        let cc = StudioApp::build_engine(EngineKind::Ccoeff);

        let sse_m = sse
            .match_template(&haystack_dyn, &needle_dyn)
            .expect("SSE engine should find embedded needle");
        let cc_m = cc
            .match_template(&haystack_dyn, &needle_dyn)
            .expect("CCOEFF engine should find embedded needle");

        // 非周期 needle on 単色背景は一意。downscale=2 → 位置は (40..=42) に一致。
        for (got, axis) in [(sse_m.region.x, "sse.x"), (cc_m.region.x, "cc.x")] {
            assert!(
                (40..=42).contains(&got),
                "{axis} should be ~40 (downscale=2), got {got}"
            );
        }
        for (got, axis) in [(sse_m.region.y, "sse.y"), (cc_m.region.y, "cc.y")] {
            assert!(
                (40..=42).contains(&got),
                "{axis} should be ~40 (downscale=2), got {got}"
            );
        }
    }
}
