//! テンプレートマッチングによる画像認識層。
//!
//! 画像からゲーム状態への変換を担当する。
//! デバイス通信・入力実行は行わない。

mod ccoeff;
mod collector;
mod diagnose;
mod engine;
mod letterbox;
mod matcher;
mod pipeline;
mod scale;
mod scene_detector;
mod template_store;

pub use ccoeff::CcoeffVisionEngine;
pub use collector::{
    ScreenGroup, TileCandidate, VerifyResult, collect_templates, compute_similarity,
    extract_stable_tiles, group_captures, verify_templates,
};
pub use diagnose::{DiagnoseEntry, diagnose_all, diagnose_task, format_diagnose_report};
pub use engine::{SseVisionEngine, VisionEngine};
pub use letterbox::{CropInfo, crop_to_content, crop_to_content_with_info};
pub use matcher::{MatchResult, TemplateMatcher};
pub use pipeline::{
    Action, Algorithm, PipelineManifest, StepOutcome, TaskDef, TaskDefError, load_pipeline,
    load_pipeline_manifest, run_step,
};
pub use scale::{BASE_HEIGHT, BASE_WIDTH, ScreenScaler, roi_to_normalized};
pub use scene_detector::SceneDetector;
pub use template_store::{TemplateEntry, TemplateStore, TemplateStoreError};
