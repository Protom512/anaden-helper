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

/// ゲームアプリの起動制御（Android）。
///
/// [`AppController`] によるアプリ起動・フォアグラウンド確認、
/// [`build_launch_command`] / [`ensure_app_open_with`] / [`parse_foreground_package`]
/// ヘルパ、[`GAME_PACKAGE`] / [`GAME_ACTIVITY`] 定数、
/// [`EnsureOutcome`] 起動結果を再エクスポートする。
///
/// ```
/// use anaden_device::{AppController, GAME_PACKAGE};
/// assert!(GAME_PACKAGE.contains('.'));
/// ```
pub use app_control::{
    AppController, EnsureOutcome, GAME_ACTIVITY, GAME_PACKAGE, build_launch_command,
    ensure_app_open_with, parse_foreground_package,
};
/// ADB クライアント（サブプロセス `adb` 実行ラッパ）と [`AdbError`]。
///
/// ```
/// use anaden_device::AdbClient;
/// let client = AdbClient::new("emulator-5554");
/// ```
pub use client::{AdbClient, AdbError};
/// ディスプレイ解像度・DPI 情報の取得コントローラ。
pub use display::DisplayController;
/// `adb shell input` 系コマンドの送信 executor。
pub use input::InputExecutor;
/// 常駐 scrcpy プロセスからフレームを受信する高速キャプチャ（`capture-scrcpy` feature）。
#[cfg(feature = "capture-scrcpy")]
pub use scrcpy::{ScrcpyCapture, ScrcpyConfig};
/// scrcpy 制御ソケット経由のタッチ注入セッション（`capture-scrcpy` feature）。
///
/// `adb input tap` がアンチチートで無視されるための代替経路。
/// `ACTION_DOWN` / `ACTION_MOVE` / `ACTION_UP` と [`TouchAction`] を含む。
#[cfg(feature = "capture-scrcpy")]
pub use scrcpy_session::{
    ACTION_DOWN, ACTION_MOVE, ACTION_UP, ScrcpySession, ScrcpySessionConfig, TouchAction,
};
/// `adb exec-out screencap` によるスクリーンショット取得。
pub use screenshot::ScreenshotCapture;
/// PC版(Windows) `PrintWindow` ベースのキャプチャ（`DEFAULT_PROCESS_NAME` = 対象プロセス名）。
#[cfg(windows)]
pub use win32_capture::{DEFAULT_PROCESS_NAME, Win32Capture};
/// PC版(Windows) `SendInput` ベースの入力注入（[`InputMethod`] で切替）。
#[cfg(windows)]
pub use win32_input::{InputMethod, Win32InputExecutor};
/// PC版(Windows) ゲームプロセスの起動（launcher/child/workdir/wait デフォルト定数付き）。
#[cfg(windows)]
pub use win32_launch::{
    DEFAULT_CHILD, DEFAULT_LAUNCHER, DEFAULT_WAIT, DEFAULT_WORKDIR, Win32Launch,
};
