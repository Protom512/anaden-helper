//! デバイス (PC 版 Windows バックエンド) 共通のエラー型。
//!
//! Issue #188 で Android (ADB) 経路を削除し、PC (Windows) 専用化したことに
//! 伴い、旧 `AdbError` を `DeviceError` へ改名・スリム化した。capture / input /
//! launch の各 Win32 バックエンドが共通で使う。

use thiserror::Error;

/// デバイス操作 (キャプチャ・入力注入・起動) の失敗。
#[derive(Debug, Error)]
pub enum DeviceError {
    /// Win32 API 呼び出し・プロセス解決等の操作失敗。詳細メッセージを保持する。
    #[error("Device operation failed: {message}")]
    CommandFailed { message: String },
}
