//! Another Eden 自動操作のドメイン型と trait 定義。
//!
//! このクレートは副作用を持たない。すべての I/O（画像読み込み、ADB 通信等）は
//! 上位クレート（`anaden-device`, `anaden-vision`）に委ねる。

pub mod action;
pub mod game_state;
pub mod goal;
pub mod recognition;
pub mod region;
pub mod strategy;

pub use action::{InputAction, ScreenPoint};
pub use game_state::{BattlePhase, DialogType, GameState, MenuTab, MiniGameType};
pub use goal::{
    Goal, GoalError, GoalReport, GoalStatus, GoalStatusContext, StopCondition, evaluate,
};
pub use recognition::{MatchConfidence, RecognitionResult, TemplateMatch};
pub use region::ScreenRegion;
pub use strategy::MiniGameStrategy;
