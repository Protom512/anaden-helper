//! ADB デバイス通信層。
//!
//! Android デバイスとの通信（スクリーンショット取得、入力コマンド送信）を担当する。
//! ゲームロジックは一切持たず、`anaden-core` の型のみを使用する。

mod app_control;
mod client;
mod display;
mod input;
mod screenshot;
#[cfg(feature = "capture-scrcpy")]
mod scrcpy;
#[cfg(feature = "capture-scrcpy")]
mod scrcpy_session;

pub use app_control::{
    AppController, EnsureOutcome, GAME_ACTIVITY, GAME_PACKAGE, build_launch_command,
    ensure_app_open_with, parse_foreground_package,
};
pub use client::{AdbClient, AdbError};
pub use display::DisplayController;
pub use input::InputExecutor;
pub use screenshot::ScreenshotCapture;
#[cfg(feature = "capture-scrcpy")]
pub use scrcpy::{ScrcpyCapture, ScrcpyConfig};
#[cfg(feature = "capture-scrcpy")]
pub use scrcpy_session::{
    ScrcpySession, ScrcpySessionConfig, TouchAction, ACTION_DOWN, ACTION_MOVE, ACTION_UP,
};
