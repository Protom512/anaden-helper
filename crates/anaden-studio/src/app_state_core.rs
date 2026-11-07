use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, SyncSender};

use eframe::egui;
use image::DynamicImage;

use anaden_core::{MatchConfidence, ScreenRegion};
use anaden_vision::{ScreenScaler, SseVisionEngine, TemplateMatcher, VisionEngine};

use crate::app_state_connection::ConnectionStatus;
use crate::app_state_pipeline_task::PipelineActionKind;
use crate::batch::ConfusionMatrix;
use crate::canvas::RoiEdit;
use crate::childproc::ChildProcess;
use crate::log_view::{AutoScrollFollow, DEFAULT_MAX_LINES, LogEntry, LogEvent, SharedLogBuffer};
use crate::proposals::Proposal;
use crate::scoring::Discrimination;
use crate::source::LiveCapture;
use crate::tasks::QueueExec;

pub(crate) const HEATMAP_DOWNSCALE: u32 = 4;

const TASK_LOG_CHANNEL_CAPACITY: usize = 1024;

pub(crate) const STATE_OPTIONS: &[&str] = &[
    "title", "field", "loading", "battle", "fishing", "menu", "dialog", "unknown",
];

pub(crate) const DEFAULT_TEMPLATE_NAME: &str = "MyFirstTapPc";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppMode {
    Authoring,
    LiveAuthoring,
    Batch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum EngineKind {
    Sse,
    #[default]
    Ccoeff,
}

impl EngineKind {
    pub(crate) fn method_str(self) -> &'static str {
        match self {
            EngineKind::Sse => "sse",
            EngineKind::Ccoeff => "ccoeff",
        }
    }
}

pub struct StudioApp {
    pub(crate) screenshot: Option<Arc<DynamicImage>>,
    pub(crate) screenshot_tex: Option<egui::TextureHandle>,
    pub(crate) roi: RoiEdit,
    pub(crate) scored_roi: Option<ScreenRegion>,
    pub(crate) positives: Vec<Arc<DynamicImage>>,
    pub(crate) negatives: Vec<Arc<DynamicImage>>,
    pub(crate) discrimination: Option<Discrimination>,
    pub(crate) engine_kind: EngineKind,
    pub(crate) engine: Box<dyn VisionEngine>,
    pub(crate) heatmap_engine: Box<dyn VisionEngine>,
    pub(crate) heatmap_tex: Option<egui::TextureHandle>,
    pub(crate) heatmap_search: ScreenRegion,
    pub(crate) best_match: Option<ScreenRegion>,
    pub(crate) tpl_name: String,
    pub(crate) tpl_state_idx: usize,
    pub(crate) save_dir: PathBuf,
    pub(crate) mode: AppMode,
    pub(crate) test_dir: PathBuf,
    pub(crate) batch_threshold: f32,
    pub(crate) batch_result: Option<ConfusionMatrix>,
    pub(crate) win_exe: String,
    pub(crate) live: Option<LiveCapture>,
    pub(crate) scaler: ScreenScaler,
    pub(crate) proposals: Vec<Proposal>,
    pub(crate) proposing: bool,
    pub(crate) proposal_rx: Option<Receiver<Vec<Proposal>>>,
    pub(crate) status: String,
    pub(crate) connection: ConnectionStatus,
    pub(crate) task_dir: PathBuf,
    pub(crate) task_action: PipelineActionKind,
    pub(crate) scenario: crate::scenario_ui::ScenarioPanel,
    pub(crate) live_authoring: crate::authoring_ui::AuthoringPanel,
    pub(crate) task_defs: Option<crate::tasks::TaskListState>,
    pub(crate) task_queue: Option<QueueExec>,
    pub(crate) task_queue_report: Option<PathBuf>,
    pub(crate) task_child: ChildProcess,
    pub(crate) task_log: SharedLogBuffer,
    pub(crate) task_log_tx: SyncSender<LogEvent>,
    pub(crate) task_log_rx: Receiver<LogEvent>,
    pub(crate) task_log_snapshot: Vec<LogEntry>,
    pub(crate) task_log_revision: u64,
    pub(crate) task_scroll: AutoScrollFollow,
    pub(crate) anaden_program: String,
}

impl Default for StudioApp {
    fn default() -> Self {
        Self::with_initial_target(None)
    }
}

fn default_win_exe() -> String {
    #[cfg(windows)]
    {
        crate::source::DEFAULT_PROCESS_NAME.to_string()
    }
    #[cfg(not(windows))]
    {
        "AnotherEden.exe".to_string()
    }
}

impl StudioApp {
    pub fn with_initial_target(exe: Option<String>) -> Self {
        let default_kind = EngineKind::default();
        let (task_log_tx, task_log_rx) = mpsc::sync_channel::<LogEvent>(TASK_LOG_CHANNEL_CAPACITY);
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
            tpl_name: String::from(DEFAULT_TEMPLATE_NAME),
            tpl_state_idx: 0,
            save_dir: PathBuf::from("./templates/scenes"),
            mode: AppMode::Authoring,
            test_dir: PathBuf::from("./templates/tests"),
            batch_threshold: 0.5,
            batch_result: None,
            win_exe: exe.unwrap_or_else(default_win_exe),
            live: None,
            scaler: ScreenScaler::new(),
            proposals: vec![],
            proposing: false,
            proposal_rx: None,
            status: String::from("スクリーンショットと正例/負例フォルダを読み込んでください"),
            connection: ConnectionStatus::default(),
            task_dir: PathBuf::from("./templates/pipelines/created"),
            task_action: PipelineActionKind::ClickSelf,
            scenario: crate::scenario_ui::ScenarioPanel::new(
                Self::workspace_root().join("templates/pipelines"),
            ),
            live_authoring: crate::authoring_ui::AuthoringPanel::new(),
            task_defs: None,
            task_queue: None,
            task_queue_report: None,
            task_child: ChildProcess::new(),
            task_log: SharedLogBuffer::new(DEFAULT_MAX_LINES),
            task_log_tx,
            task_log_rx,
            task_log_snapshot: Vec::new(),
            task_log_revision: 0,
            task_scroll: AutoScrollFollow::default(),
            anaden_program: "anaden".to_string(),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::app_state_connection::ConnectionState;

    #[test]
    fn engine_kind_default_is_ccoeff() {
        assert_eq!(EngineKind::default(), EngineKind::Ccoeff);
    }

    #[test]
    fn app_default_connection_is_unknown() {
        let app = StudioApp::default();
        assert_eq!(app.connection().state, ConnectionState::Unknown);
    }
}
