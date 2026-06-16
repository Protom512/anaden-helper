//! テンプレートマッチングによる画像認識層。
//!
//! 画像からゲーム状態への変換を担当する。
//! デバイス通信・入力実行は行わない。

mod ccoeff;
mod collector;
mod engine;
mod matcher;
mod pipeline;
mod scale;
mod scene_detector;
mod template_store;

pub use collector::{
    ScreenGroup, TileCandidate, VerifyResult,
    collect_templates, compute_similarity, extract_stable_tiles,
    group_captures, verify_templates,
};
pub use ccoeff::CcoeffVisionEngine;
pub use engine::{SseVisionEngine, VisionEngine};
pub use matcher::{MatchResult, TemplateMatcher};
pub use pipeline::{Action, Algorithm, StepOutcome, TaskDef, TaskDefError, load_pipeline, run_step};
pub use scale::ScreenScaler;
pub use scene_detector::SceneDetector;
pub use template_store::{TemplateEntry, TemplateStore};
