//! PC 版 (Windows) デバイス通信層。
//!
//! Another Eden PC 版ウィンドウに対するキャプチャ (PrintWindow)・入力注入
//! (SendInput / PostMessage)・起動 (プロセス起動・生存監視) を担当する。
//! ゲームロジックは一切持たず、`anaden-core` の型のみを使用する。
//!
//! Issue #188 で Android (ADB / scrcpy / minitouch) 経路を削除し、
//! PC (Windows) 専用化した。履歴は git 履歴を参照のこと。

mod ensure;
mod error;
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

/// ゲーム起動保証の成果物 enum ([`Win32Launch::ensure_open`] 等)。
///
/// ```
/// use anaden_device::EnsureOutcome;
/// assert_ne!(EnsureOutcome::AlreadyOpen, EnsureOutcome::Timeout);
/// ```
pub use ensure::EnsureOutcome;
/// capture / input / launch 共通のデバイス操作エラー。
pub use error::DeviceError;
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
