use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;

use image::DynamicImage;

use anaden_core::{MatchConfidence, ScreenRegion};
use anaden_vision::{CcoeffVisionEngine, SseVisionEngine, TemplateMatcher, VisionEngine};

use crate::canvas::RoiEdit;
use crate::childproc::SpawnSpec;
use crate::library::{self, TemplateSpec};
use crate::log_view::{AutoScrollFollow, DEFAULT_MAX_LINES, LogBuffer, LogEntry};
use crate::proposals::{self, Proposal};
use crate::tasks::{self, QueueAction, QueueEntry, QueueExec, QueueState};

pub use crate::app_state::{
    AppMode, ConnectionState, ConnectionStatus, PipelineActionKind, StudioApp,
    check_windows_process, pipeline_task_spec, save_pipeline_task,
};
use crate::app_state::{DEFAULT_TEMPLATE_NAME, EngineKind, HEATMAP_DOWNSCALE, STATE_OPTIONS};

impl StudioApp {
    pub(crate) fn build_engine(kind: EngineKind) -> Box<dyn VisionEngine> {
        match kind {
            EngineKind::Sse => Box::new(SseVisionEngine::new(TemplateMatcher::new(
                MatchConfidence::new(0.0),
                2,
            ))),
            EngineKind::Ccoeff => Box::new(CcoeffVisionEngine::new(MatchConfidence::new(0.0), 2)),
        }
    }

    pub fn mode(&self) -> AppMode {
        self.mode
    }

    pub fn set_mode(&mut self, mode: AppMode) {
        self.mode = mode;
    }

    pub fn connection(&self) -> &ConnectionStatus {
        &self.connection
    }

    pub fn set_anaden_program(&mut self, program: impl Into<String>) {
        self.anaden_program = program.into();
    }

    pub fn task_queue(&self) -> Option<&QueueExec> {
        self.task_queue.as_ref()
    }

    pub fn task_log_lines(&self) -> &[LogEntry] {
        &self.task_log_snapshot
    }

    pub(crate) fn task_queue_active(&self) -> bool {
        self.task_queue
            .as_ref()
            .is_some_and(|q| !matches!(q.state(), QueueState::Completed))
    }

    pub fn ensure_task_list_loaded(&mut self) {
        if self.task_defs.is_none() {
            let dir = Self::workspace_root().join("templates/tasks");
            self.load_task_list(&dir);
        }
    }

    pub fn load_task_list(&mut self, tasks_dir: &Path) {
        match tasks::TaskListState::load(tasks_dir) {
            Ok(list) => {
                self.status = format!("タスク定義: {} 件読込", list.definitions().len());
                self.task_defs = Some(std::sync::Arc::new(list));
            }
            Err(e) => self.status = format!("タスク定義読込失敗: {e}"),
        }
    }

    pub fn toggle_task(&mut self, id: &str) {
        let Some(list) = self.task_defs.as_mut() else {
            self.status = "タスク定義が未読込です".to_string();
            return;
        };
        let list = std::sync::Arc::make_mut(list);
        match list.toggle(id) {
            Ok(()) => self.status = format!("選択: {} 件", list.selected_count()),
            Err(e) => self.status = e.to_string(),
        }
    }

    pub(crate) fn on_scenario_task_event(
        &mut self,
        event: crate::scenario_task_link::ScenarioPanelEvent,
    ) {
        match event {
            crate::scenario_task_link::ScenarioPanelEvent::TaskEnabled { message } => {
                let selected: Vec<String> = self
                    .task_defs
                    .as_ref()
                    .map(|list| list.selected_ids().to_vec())
                    .unwrap_or_default();
                self.load_task_list(&Self::workspace_root().join("templates/tasks"));
                if let Some(list) = self.task_defs.as_mut() {
                    let list = std::sync::Arc::make_mut(list);
                    for id in selected {
                        if list.find(&id).is_some_and(|def| def.is_selectable()) {
                            let _ = list.toggle(&id);
                        }
                    }
                }
                self.status = message;
            }
        }
    }

    pub(crate) fn workspace_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
    }

    pub(crate) fn cli_target(&self) -> &'static str {
        "windows"
    }

    pub fn start_task_queue(&mut self) {
        let Some(list) = &self.task_defs else {
            self.status = "タスク定義が未読込です".to_string();
            return;
        };
        let entries = match list.queue_entries(
            &self.anaden_program,
            self.cli_target(),
            None,
            &Self::workspace_root(),
        ) {
            Ok(e) => e,
            Err(e) => {
                self.status = e.to_string();
                return;
            }
        };
        self.start_task_entries(entries);
    }

    pub fn start_task_entries(&mut self, entries: Vec<QueueEntry>) {
        if self.task_queue_active() {
            self.status = "キュー実行中のため開始できません（中止してから再開）".to_string();
            return;
        }
        if entries.is_empty() {
            self.status = "チェックされたタスクがありません".to_string();
            return;
        }
        self.task_log.with_buf(LogBuffer::clear);
        self.task_scroll = AutoScrollFollow::default();
        let mut queue = QueueExec::new(entries);
        let action = queue.start();
        let count = queue.total();
        self.task_queue = Some(queue);
        self.status = format!("開始: {count} タスク");
        self.apply_task_action(action);
        self.refresh_task_log_snapshot();
    }

    pub fn resume_task_queue(&mut self) {
        let action = match &mut self.task_queue {
            Some(queue) => queue.resume(),
            None => QueueAction::Noop,
        };
        self.apply_task_action(action);
    }

    pub fn abort_task_queue(&mut self) {
        let _ = self.task_child.stop();
        if let Some(queue) = &mut self.task_queue {
            queue.abort();
        }
        self.push_task_log("[studio] === キュー中止 ===");
        self.status = "キューを中止しました".to_string();
    }

    fn apply_task_action(&mut self, action: QueueAction) {
        match action {
            QueueAction::Start(spec) => self.spawn_task_spec(&spec),
            QueueAction::WaitForExit => {
                if let Some(queue) = self.task_queue.as_ref() {
                    self.status = queue.summary();
                }
            }
            QueueAction::Noop => {}
            QueueAction::QueueCompleted => {
                self.push_task_log("[studio] === キュー完了 ===");
                self.status = "全タスクが完了しました".to_string();
            }
        }
    }

    fn spawn_task_spec(&mut self, spec: &SpawnSpec) {
        let sep = match self.task_queue.as_ref().and_then(|q| q.current_entry()) {
            Some(entry) => format!("[studio] === task: {} ===", entry.label),
            None => "[studio] === task ===".to_string(),
        };
        self.push_task_log(&sep);
        self.task_queue_report = crate::runner_exec::report_file_from_args(&spec.args);
        if let Err(e) = self.task_child.start(spec, self.task_log_tx.clone()) {
            self.push_task_log(&format!("[studio] 起動に失敗: {e}"));
            let action = match &mut self.task_queue {
                Some(queue) => queue.on_exit(None),
                None => QueueAction::Noop,
            };
            self.apply_task_action(action);
            self.status = format!("起動に失敗: {e}");
        }
    }

    pub fn drain_task_logs(&mut self) {
        if self.task_queue.is_none() {
            return;
        }
        let (new_lines, exit_code) =
            crate::log_view::drain_channel_into(&self.task_log, &self.task_log_rx);
        if new_lines > 0 {
            self.task_scroll.observe_new_lines(new_lines);
        }
        let mut report = None;
        if exit_code.is_some() {
            report = self.task_queue_report.take().and_then(|path| {
                let parsed = crate::history::load_run_report(&path);
                if parsed.is_none() {
                    self.push_task_log(&format!(
                        "WARN キュー run レポート読込失敗 (終了コードのみで分類): {}",
                        path.display()
                    ));
                }
                parsed
            });
        }
        if let Some(code) = exit_code
            && let Some(queue) = &mut self.task_queue
        {
            let outcome = crate::history::classify_outcome(code, report.as_ref());
            let action = queue.on_exit_classified(outcome, code);
            self.apply_task_action(action);
        }
        self.refresh_task_log_snapshot();
    }

    pub(crate) fn push_task_log(&mut self, line: &str) {
        let line = line.to_string();
        self.task_log.with_buf(|b| b.push_line(&line));
        self.refresh_task_log_snapshot();
    }

    pub(crate) fn refresh_task_log_snapshot(&mut self) {
        if let Some(update) = self.task_log.take_update() {
            update.apply_to(&mut self.task_log_snapshot, DEFAULT_MAX_LINES);
        }
    }

    pub fn run_connection_check(&mut self) {
        self.connection.state = ConnectionState::Checking;
        self.connection = check_windows_process(&self.win_exe);
    }

    pub(crate) fn switch_engine(&mut self, kind: EngineKind) {
        self.engine_kind = kind;
        self.engine = StudioApp::build_engine(kind);
        self.scored_roi = None;
        self.discrimination = None;
        self.status = format!(
            "エンジン切替: {}",
            match kind {
                EngineKind::Sse => "SSE（輝度差ベース）",
                EngineKind::Ccoeff => "CCOEFF（ロバスト・輝度シフト不変）",
            }
        );
    }

    pub(crate) fn open_screenshot(&mut self) {
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
                    self.screenshot_tex = None;
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

    pub(crate) fn load_positives(&mut self) {
        if let Some(dir) = rfd::FileDialog::new().pick_folder() {
            let imgs = load_folder(&dir);
            self.status = format!("正例: {} 枚読込", imgs.len());
            self.positives = imgs;
            self.scored_roi = None;
        }
    }

    pub(crate) fn load_negatives(&mut self) {
        if let Some(dir) = rfd::FileDialog::new().pick_folder() {
            let imgs = load_folder(&dir);
            self.status = format!("負例: {} 枚読込", imgs.len());
            self.negatives = imgs;
            self.scored_roi = None;
        }
    }

    fn roi_crop_and_threshold(&self) -> Option<(DynamicImage, ScreenRegion, f32)> {
        let img = self.screenshot.clone()?;
        let roi = self.roi.rect()?;
        let crop = img.crop_imm(roi.x, roi.y, roi.width, roi.height);
        let threshold = self
            .discrimination
            .as_ref()
            .map(|d| ((d.own_min + d.other_max) / 2.0).clamp(0.5, 0.99))
            .unwrap_or(0.9);
        Some((crop, roi, threshold))
    }

    fn report_save_status<E: std::fmt::Display>(
        &mut self,
        label: &str,
        result: Result<std::path::PathBuf, E>,
        crop: &DynamicImage,
    ) {
        match result {
            Ok(p) => {
                self.status = match library::template_structure_warning(crop) {
                    Some(warning) => format!("{label}: {} / {warning}", p.display()),
                    None => format!("{label}: {}", p.display()),
                };
            }
            Err(e) => self.status = format!("{label}失敗: {e}"),
        }
    }

    fn current_template_spec(&self, roi: ScreenRegion, threshold: f32) -> TemplateSpec {
        TemplateSpec {
            name: self.tpl_name.trim().to_string(),
            state: STATE_OPTIONS[self.tpl_state_idx].to_string(),
            roi,
            threshold,
            method: self.engine_kind.method_str().to_string(),
        }
    }

    pub(crate) fn save_current_template(&mut self) {
        let Some((crop, roi, threshold)) = self.roi_crop_and_threshold() else {
            return;
        };
        let spec = self.current_template_spec(roi, threshold);
        let result = library::save_template(&self.save_dir, &spec, &crop);
        self.report_save_status("保存", result, &crop);
    }

    pub(crate) fn run_proposals(&mut self) {
        if self.proposing {
            return;
        }
        let Some(img) = self.screenshot.clone() else {
            self.status = "スクリーンショットを先に読み込んでください".to_string();
            return;
        };
        self.proposing = true;
        self.status = "ROI候補を計算中…".to_string();

        let (tx, rx) = mpsc::channel::<Vec<Proposal>>();
        self.proposal_rx = Some(rx);

        let downscale = HEATMAP_DOWNSCALE;
        std::thread::spawn(move || {
            let engine =
                SseVisionEngine::new(TemplateMatcher::new(MatchConfidence::new(0.0), downscale));
            let ps = proposals::propose(&engine, &img, 96, 96, 96, 12);
            let _ = tx.send(ps);
        });
    }

    pub(crate) fn apply_proposal(&mut self, roi: ScreenRegion) {
        self.roi.anchor = Some((roi.x, roi.y));
        self.roi.current = Some((roi.right(), roi.bottom()));
        self.roi.dragging = false;
        self.scored_roi = None;
    }

    pub(crate) fn save_current_pipeline_task(&mut self) {
        let Some((crop, roi, threshold)) = self.roi_crop_and_threshold() else {
            self.status = "pipeline task 保存にはスクリーンショットとROI確定が必要です".to_string();
            return;
        };
        let name = if self.tpl_name.trim().is_empty() {
            DEFAULT_TEMPLATE_NAME.to_string()
        } else {
            self.tpl_name.trim().to_string()
        };
        let Some(spec) = pipeline_task_spec(
            &name,
            STATE_OPTIONS[self.tpl_state_idx],
            self.engine_kind.method_str(),
            roi,
            threshold,
            self.task_action,
        ) else {
            self.status = format!(
                "pipeline task 保存失敗: 未知の方式 ({})",
                self.engine_kind.method_str()
            );
            return;
        };
        let result = save_pipeline_task(&self.task_dir, &spec, &crop);
        self.report_save_status("pipeline task 保存", result, &crop);
    }

    pub(crate) fn scenario_candidate(&self) -> Option<(anaden_vision::TaskDef, DynamicImage)> {
        let roi = self.roi.rect()?;
        let img = self.screenshot.as_ref()?;
        let name = if self.tpl_name.trim().is_empty() {
            DEFAULT_TEMPLATE_NAME.to_string()
        } else {
            self.tpl_name.trim().to_string()
        };
        let threshold = self
            .discrimination
            .as_ref()
            .map(|d| ((d.own_min + d.other_max) / 2.0).clamp(0.5, 0.99))
            .unwrap_or(0.9);
        let spec = pipeline_task_spec(
            &name,
            STATE_OPTIONS[self.tpl_state_idx],
            self.engine_kind.method_str(),
            roi,
            threshold,
            self.task_action,
        )?;
        let crop = img.crop_imm(roi.x, roi.y, roi.width, roi.height);
        Some((spec, crop))
    }
}

fn load_folder(path: &Path) -> Vec<Arc<DynamicImage>> {
    let mut out = vec![];
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            let p: PathBuf = entry.path();
            if is_image(&p)
                && let Ok(img) = image::open(&p)
            {
                out.push(Arc::new(img));
            }
        }
    }
    out
}

fn is_image(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase())
            .as_deref(),
        Some("png") | Some("jpg") | Some("jpeg") | Some("bmp")
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::scoring::Discrimination;
    use image::{GrayImage, Luma};

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
    fn build_engine_produces_ccoeff_by_default() {
        let _engine = StudioApp::build_engine(EngineKind::default());
        let _sse = StudioApp::build_engine(EngineKind::Sse);
    }

    #[test]
    fn set_mode_switches_between_authoring_and_batch() {
        let mut app = StudioApp::default();
        app.set_mode(AppMode::Batch);
        assert_eq!(app.mode(), AppMode::Batch);
        app.set_mode(AppMode::Authoring);
        assert_eq!(app.mode(), AppMode::Authoring);
    }

    #[test]
    fn build_engine_both_engines_locate_embedded_needle() {
        let needle = textured_needle(20, 20);
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

    #[test]
    fn run_connection_check_updates_state() {
        let mut app = StudioApp::default();
        app.win_exe.clear();
        app.run_connection_check();
        assert_eq!(app.connection().state, ConnectionState::Disconnected);
        assert!(app.connection().reason_line().contains("理由"));
    }

    #[test]
    fn save_current_pipeline_task_without_roi_reports_status() {
        let dir = tempfile::tempdir().unwrap();
        let app = StudioApp {
            task_dir: dir.path().to_path_buf(),
            ..StudioApp::default()
        };
        let mut app = app;
        app.save_current_pipeline_task();
        assert!(app.status.contains("ROI"));
        assert!(dir.path().read_dir().unwrap().next().is_none());
    }

    #[test]
    fn save_current_pipeline_task_writes_loadable_toml() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = StudioApp {
            task_dir: dir.path().to_path_buf(),
            ..StudioApp::default()
        };
        app.tpl_name = "created_task".to_string();
        app.screenshot = Some(Arc::new(DynamicImage::ImageLuma8(
            image::GrayImage::from_pixel(200, 100, Luma([255])),
        )));
        app.roi.anchor = Some((10, 10));
        app.roi.current = Some((110, 60));
        app.discrimination = Some(Discrimination {
            own_min: 0.9,
            other_max: 0.8,
            own_scores: vec![0.9],
            other_scores: vec![0.8],
        });

        app.save_current_pipeline_task();
        assert!(app.status.contains("保存"), "status: {}", app.status);

        let tasks = anaden_vision::load_pipeline(dir.path()).unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].name, "created_task");
        assert_eq!(tasks[0].roi, Some([10, 10, 100, 50]));
        assert!((tasks[0].threshold - 0.85).abs() < 1e-4);
        assert_eq!(tasks[0].action, Some(anaden_vision::Action::ClickSelf));
    }

    #[test]
    fn save_current_pipeline_task_threshold_defaults_without_discrimination() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = StudioApp {
            task_dir: dir.path().to_path_buf(),
            ..StudioApp::default()
        };
        app.screenshot = Some(Arc::new(DynamicImage::ImageLuma8(
            image::GrayImage::from_pixel(200, 100, Luma([255])),
        )));
        app.roi.anchor = Some((0, 0));
        app.roi.current = Some((50, 50));
        app.save_current_pipeline_task();
        let tasks = anaden_vision::load_pipeline(dir.path()).unwrap();
        assert!((tasks[0].threshold - 0.9).abs() < 1e-4);
    }

    #[test]
    fn save_current_template_flat_crop_warns_but_saves() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = StudioApp {
            save_dir: dir.path().to_path_buf(),
            ..StudioApp::default()
        };
        app.tpl_name = "flat_tpl".to_string();
        app.screenshot = Some(Arc::new(DynamicImage::ImageLuma8(
            image::GrayImage::from_pixel(200, 100, Luma([255])),
        )));
        app.roi.anchor = Some((10, 10));
        app.roi.current = Some((110, 60));
        app.save_current_template();

        assert!(app.status.contains("保存"), "status: {}", app.status);
        assert!(
            app.status.contains("認識不能"),
            "無構造テンプレート警告 (認識不能の恐れ) が出ること: {}",
            app.status
        );
        assert!(dir.path().join("title").join("flat_tpl.png").exists());
        assert!(dir.path().join("title").join("flat_tpl.toml").exists());
    }

    #[test]
    fn save_current_template_textured_crop_saves_without_warning() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = StudioApp {
            save_dir: dir.path().to_path_buf(),
            ..StudioApp::default()
        };
        app.tpl_name = "ok_tpl".to_string();
        app.screenshot = Some(Arc::new(luma_dyn(textured_needle(200, 100))));
        app.roi.anchor = Some((10, 10));
        app.roi.current = Some((110, 60));
        app.save_current_template();

        assert!(app.status.contains("保存"), "status: {}", app.status);
        assert!(
            !app.status.contains("警告"),
            "構造ありテンプレートに警告を出さない (偽陽性ゼロ): {}",
            app.status
        );
        assert!(dir.path().join("title").join("ok_tpl.png").exists());
    }

    #[test]
    fn save_current_pipeline_task_flat_crop_warns_but_saves() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = StudioApp {
            task_dir: dir.path().to_path_buf(),
            ..StudioApp::default()
        };
        app.tpl_name = "flat_task".to_string();
        app.screenshot = Some(Arc::new(DynamicImage::ImageLuma8(
            image::GrayImage::from_pixel(200, 100, Luma([255])),
        )));
        app.roi.anchor = Some((0, 0));
        app.roi.current = Some((50, 50));
        app.save_current_pipeline_task();

        assert!(
            app.status.contains("認識不能"),
            "無構造テンプレート警告 (認識不能の恐れ) が出ること: {}",
            app.status
        );
        let tasks = anaden_vision::load_pipeline(dir.path()).unwrap();
        assert_eq!(tasks.len(), 1, "TOML+PNG が書かれている: {}", app.status);
    }

    #[test]
    fn save_current_pipeline_task_textured_crop_saves_without_warning() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = StudioApp {
            task_dir: dir.path().to_path_buf(),
            ..StudioApp::default()
        };
        app.tpl_name = "ok_task".to_string();
        app.screenshot = Some(Arc::new(luma_dyn(textured_needle(200, 100))));
        app.roi.anchor = Some((0, 0));
        app.roi.current = Some((100, 80));
        app.save_current_pipeline_task();

        assert!(app.status.contains("保存"), "status: {}", app.status);
        assert!(
            !app.status.contains("警告"),
            "構造ありテンプレートに警告を出さない (偽陽性ゼロ): {}",
            app.status
        );
        assert!(dir.path().join("ok_task.png").exists());
    }

    #[test]
    fn scenario_candidate_default_name_follows_naming_convention() {
        assert!(
            crate::scenario_load::task_name_issue(DEFAULT_TEMPLATE_NAME).is_none(),
            "既定候補名 {DEFAULT_TEMPLATE_NAME} は命名規約に適合すること"
        );

        let mut app = StudioApp::default();
        assert_eq!(app.tpl_name, DEFAULT_TEMPLATE_NAME, "フォーム初期値");
        app.tpl_name = String::new();
        app.screenshot = Some(Arc::new(DynamicImage::ImageLuma8(
            image::GrayImage::from_pixel(200, 100, Luma([255])),
        )));
        app.roi.anchor = Some((10, 10));
        app.roi.current = Some((110, 60));

        let (spec, _crop) = app.scenario_candidate().expect("candidate");

        assert_eq!(spec.name, DEFAULT_TEMPLATE_NAME);
    }

    fn tasks_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("templates")
            .join("tasks")
    }

    fn exit_spec(code: i32) -> SpawnSpec {
        if cfg!(windows) {
            SpawnSpec::new(
                "cmd",
                ["/C".to_string(), "exit".to_string(), code.to_string()],
            )
        } else {
            SpawnSpec::new("sh", ["-c".to_string(), format!("exit {code}")])
        }
    }

    fn echo_spec() -> SpawnSpec {
        if cfg!(windows) {
            SpawnSpec::new("cmd", ["/C".to_string(), "echo task-log-line".to_string()])
        } else {
            SpawnSpec::new("sh", ["-c".to_string(), "echo task-log-line".to_string()])
        }
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

    fn pump_until(
        app: &mut StudioApp,
        timeout_ms: u64,
        done: impl Fn(&QueueState) -> bool,
        what: &str,
    ) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
        loop {
            app.drain_task_logs();
            if let Some(q) = app.task_queue()
                && done(q.state())
            {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for {what} (status: {})",
                app.status
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    fn task_separator_count(app: &StudioApp) -> usize {
        app.task_log_snapshot
            .iter()
            .filter(|e| e.line.starts_with("[studio] === task:"))
            .count()
    }

    #[test]
    fn task_queue_runs_entries_sequentially_in_check_order() {
        let mut app = StudioApp::default();
        app.start_task_entries(vec![
            queue_entry("タスクA", exit_spec(0)),
            queue_entry("タスクB", exit_spec(0)),
        ]);
        pump_until(
            &mut app,
            30_000,
            |s| matches!(s, QueueState::Completed),
            "queue completion",
        );
        let queue = app.task_queue().unwrap();
        assert_eq!(queue.summary(), "完了 2/2");
        assert_eq!(task_separator_count(&app), 2);
        let lines: Vec<&str> = app
            .task_log_snapshot
            .iter()
            .map(|e| e.line.as_str())
            .collect();
        let a = lines
            .iter()
            .position(|l| *l == "[studio] === task: タスクA ===");
        let b = lines
            .iter()
            .position(|l| *l == "[studio] === task: タスクB ===");
        assert!(a.is_some() && b.is_some() && a < b, "lines: {lines:?}");
        assert!(lines.iter().any(|l| l.contains("キュー完了")));
    }

    #[test]
    fn task_list_selection_starts_queue_in_check_order() {
        let mut app = StudioApp::default();
        app.load_task_list(&tasks_dir());
        app.toggle_task("field_loop_pc");
        app.toggle_task("launch");
        app.set_anaden_program("anaden-nonexistent-bin-xyz");
        app.start_task_queue();
        let queue = app.task_queue().unwrap();
        assert!(matches!(
            queue.state(),
            QueueState::PausedAfterFailure { current: 0, .. }
        ));
        assert_eq!(queue.total(), 2);
        assert_eq!(queue.entries()[0].label, "フィールド周回");
        assert_eq!(queue.entries()[0].spec.args[0], "run");
        assert_eq!(queue.entries()[1].label, "ゲーム起動");
        assert_eq!(queue.entries()[1].spec.args[0], "launch");
        assert!(app.status.contains("起動に失敗"), "status: {}", app.status);
    }

    #[test]
    fn task_queue_progress_summary_during_run() {
        let mut app = StudioApp::default();
        app.start_task_entries(vec![queue_entry("周回", long_spec())]);
        let queue = app.task_queue().unwrap();
        assert!(matches!(queue.state(), QueueState::Running { current: 0 }));
        assert!(
            queue.summary().contains("1/1"),
            "summary: {}",
            queue.summary()
        );
        assert!(
            queue.summary().contains("周回"),
            "summary: {}",
            queue.summary()
        );
        app.drain_task_logs();
        assert!(
            app.task_log_snapshot
                .iter()
                .any(|e| e.line == "[studio] === task: 周回 ===")
        );
        app.abort_task_queue();
    }

    #[test]
    fn task_queue_log_view_renders_child_output() {
        let mut app = StudioApp::default();
        app.start_task_entries(vec![queue_entry("出力", echo_spec())]);
        pump_until(
            &mut app,
            30_000,
            |s| matches!(s, QueueState::Completed),
            "echo completion",
        );
        let lines: Vec<&str> = app
            .task_log_snapshot
            .iter()
            .map(|e| e.line.as_str())
            .collect();
        assert!(
            lines.iter().any(|l| l.contains("task-log-line")),
            "lines: {lines:?}"
        );
        assert!(
            lines.iter().any(|l| l.contains("exit=0")),
            "lines: {lines:?}"
        );
    }

    #[test]
    fn task_log_snapshot_idempotent_across_idle_drains_and_keeps_lines() {
        let mut app = StudioApp::default();
        let bogus = SpawnSpec::new("anaden-nonexistent-bin-xyz", Vec::new());
        app.start_task_entries(vec![queue_entry("失敗", bogus)]);
        pump_until(
            &mut app,
            30_000,
            |s| matches!(s, QueueState::PausedAfterFailure { .. }),
            "pause after spawn failure",
        );
        app.drain_task_logs();
        let before: Vec<String> = app
            .task_log_lines()
            .iter()
            .map(|e| e.line.clone())
            .collect();
        assert!(
            before.iter().any(|l| l.contains("起動に失敗")),
            "lines: {before:?}"
        );
        for _ in 0..60 {
            app.drain_task_logs();
        }
        let after: Vec<String> = app
            .task_log_lines()
            .iter()
            .map(|e| e.line.clone())
            .collect();
        assert_eq!(before, after);
    }

    #[test]
    fn scenario_task_enabled_event_reloads_home_list_preserving_selection() {
        let mut app = StudioApp::default();
        app.load_task_list(&tasks_dir());
        app.toggle_task("login");
        let selected_before = app.task_defs.as_ref().unwrap().selected_ids().to_vec();
        assert_eq!(selected_before, vec!["login".to_string()]);

        app.on_scenario_task_event(crate::scenario_task_link::ScenarioPanelEvent::TaskEnabled {
            message: "タスク登録・有効化: テスト".to_string(),
        });

        let list = app.task_defs.as_ref().unwrap();
        assert!(
            list.find("login").is_some_and(|def| def.is_selectable()),
            "再読込後も有効化タスクが選択可能"
        );
        assert_eq!(
            list.selected_ids(),
            selected_before.as_slice(),
            "選択は復元"
        );
        assert_eq!(app.status, "タスク登録・有効化: テスト");
    }

    #[test]
    fn task_queue_failure_pauses_and_resume_continues() {
        let mut app = StudioApp::default();
        app.start_task_entries(vec![
            queue_entry("失敗", exit_spec(1)),
            queue_entry("次", exit_spec(0)),
        ]);
        pump_until(
            &mut app,
            30_000,
            |s| matches!(s, QueueState::PausedAfterFailure { .. }),
            "failure pause",
        );
        assert_eq!(task_separator_count(&app), 1);
        assert!(app.status.contains("失敗"), "status: {}", app.status);
        app.resume_task_queue();
        pump_until(
            &mut app,
            30_000,
            |s| matches!(s, QueueState::Completed),
            "resume completion",
        );
        assert_eq!(task_separator_count(&app), 2);
        assert_eq!(app.task_queue().unwrap().summary(), "完了 2/2");
    }

    #[test]
    fn task_queue_abort_discards_remaining() {
        let mut app = StudioApp::default();
        app.start_task_entries(vec![
            queue_entry("長時間", long_spec()),
            queue_entry("次", exit_spec(0)),
        ]);
        app.abort_task_queue();
        let queue = app.task_queue().unwrap();
        assert!(matches!(queue.state(), QueueState::Completed));
        assert!(queue.is_aborted());
        assert_eq!(queue.summary(), "中止");
        pump_until(
            &mut app,
            30_000,
            |s| matches!(s, QueueState::Completed),
            "post-abort drain",
        );
        assert_eq!(task_separator_count(&app), 1, "残りタスクは起動しない");
        assert!(app.status.contains("中止"), "status: {}", app.status);
    }

    #[test]
    fn task_queue_rejects_restart_while_active() {
        let mut app = StudioApp::default();
        app.load_task_list(&tasks_dir());
        app.toggle_task("launch");
        app.start_task_entries(vec![queue_entry("長時間", long_spec())]);
        app.start_task_queue();
        assert!(
            app.status.contains("開始できません"),
            "status: {}",
            app.status
        );
        assert_eq!(app.task_queue().unwrap().total(), 1);
        app.abort_task_queue();
    }

    #[test]
    fn start_without_selection_reports_status() {
        let mut app = StudioApp::default();
        app.start_task_queue();
        assert!(app.status.contains("未読込"), "status: {}", app.status);
        assert!(app.task_queue().is_none());
        app.load_task_list(&tasks_dir());
        app.start_task_queue();
        assert!(app.status.contains("チェック"), "status: {}", app.status);
        assert!(app.task_queue().is_none());
        app.toggle_task("launch");
        app.set_anaden_program("anaden-nonexistent-bin-xyz");
        app.start_task_queue();
        assert!(app.task_queue().is_some());
        app.abort_task_queue();
        app.start_task_entries(Vec::new());
        assert!(
            app.status.contains("チェックされたタスクがありません"),
            "status: {}",
            app.status
        );
    }
}
