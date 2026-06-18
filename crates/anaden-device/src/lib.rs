//! ADB デバイス通信層。
//!
//! Android デバイスとの通信（スクリーンショット取得、入力コマンド送信）を担当する。
//! ゲームロジックは一切持たず、`anaden-core` の型のみを使用する。

mod app_control;
mod client;
mod display;
mod input;
#[cfg(feature = "capture-scrcpy")]
mod scrcpy;
#[cfg(feature = "capture-scrcpy")]
mod scrcpy_session;
mod screenshot;
// PC版(Windows) Win32 バックエンド。capture/input/launch の3モジュール。
// 全体を cfg(windows) で gating し、Linux ではコンパイル対象外とする。
#[cfg(windows)]
mod win32_capture;
#[cfg(windows)]
mod win32_input;
#[cfg(windows)]
mod win32_launch;
// PC版(Windows) プロセス列挙の共通ヘルパ。capture/input/launch から参照。
// cfg(windows) で gating し、Linux ではコンパイル対象外とする。
#[cfg(windows)]
mod win32_proc;

pub use app_control::{
    AppController, EnsureOutcome, GAME_ACTIVITY, GAME_PACKAGE, build_launch_command,
    ensure_app_open_with, parse_foreground_package,
};
pub use client::{AdbClient, AdbError};
pub use display::DisplayController;
pub use input::InputExecutor;
#[cfg(feature = "capture-scrcpy")]
pub use scrcpy::{ScrcpyCapture, ScrcpyConfig};
#[cfg(feature = "capture-scrcpy")]
pub use scrcpy_session::{
    ACTION_DOWN, ACTION_MOVE, ACTION_UP, ScrcpySession, ScrcpySessionConfig, TouchAction,
};
pub use screenshot::ScreenshotCapture;
#[cfg(windows)]
pub use win32_capture::{DEFAULT_PROCESS_NAME, Win32Capture};
#[cfg(windows)]
pub use win32_input::{InputMethod, Win32InputExecutor};
#[cfg(windows)]
pub use win32_launch::{
    DEFAULT_CHILD, DEFAULT_LAUNCHER, DEFAULT_WAIT, DEFAULT_WORKDIR, Win32Launch,
};
