//! ADB デバイス通信層。
//!
//! Android デバイスとの通信（スクリーンショット取得、入力コマンド送信）を担当する。
//! ゲームロジックは一切持たず、`anaden-core` の型のみを使用する。

mod client;
mod input;
mod screenshot;

pub use client::AdbClient;
pub use input::InputExecutor;
pub use screenshot::ScreenshotCapture;
