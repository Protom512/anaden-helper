//! テンプレートマッチングによる画像認識層。
//!
//! 画像からゲーム状態への変換を担当する。
//! デバイス通信・入力実行は行わない。

mod matcher;
mod scene_detector;
mod template_store;

pub use matcher::TemplateMatcher;
pub use scene_detector::SceneDetector;
pub use template_store::TemplateStore;
