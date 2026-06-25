//! ADB クライアント。デバイスとの接続管理を担当する。

use std::process::Command;

use thiserror::Error;
use tracing::{debug, info, warn};

#[derive(Debug, Error)]
pub enum AdbError {
    #[error("Device I/O failed: {message}")]
    CommandFailed { message: String },

    #[error("Device not found: {serial}")]
    DeviceNotFound { serial: String },

    #[error("ADB not found in PATH. Install Android SDK Platform Tools.")]
    AdbNotFound,

    #[error("Timeout waiting for device response")]
    Timeout,

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// ADB 経由でデバイスと通信するクライアント。
///
/// 設計意図: ADB との通信をカプセル化し、`adb` コマンドの詳細を上位層から隠す。
/// 将来的に `adb_client` crate（純 Rust ADB）に移行する際も、
/// このモジュールの差し替えで対応できる。
///
/// `Clone` 可能: 保持するのはシリアル・adb パスのみで共有リソースを持たない。
/// アプリ制御(`AppController::ensure_app_open`)が `'static` な非同期クロージャへ
/// クライアントを持ち出すために使用する。
#[derive(Clone)]
pub struct AdbClient {
    /// デバイスのシリアル番号または接続先（例: "emulator-5554", "localhost:5555"）
    serial: String,
    /// adb コマンドのパス
    adb_path: String,
}

impl AdbClient {
    /// 新しい ADB クライアントを作成する。
    pub fn new(serial: impl Into<String>) -> Self {
        Self {
            serial: serial.into(),
            adb_path: "adb".to_string(),
        }
    }

    /// adb コマンドのパスを明示的に指定する。
    pub fn with_adb_path(mut self, path: impl Into<String>) -> Self {
        self.adb_path = path.into();
        self
    }

    /// デバイスに接続できるか確認する。
    pub async fn check_connection(&self) -> Result<(), AdbError> {
        let output = self.run_adb_command(&["devices"]).await?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let connected = stdout
            .lines()
            .any(|line| line.starts_with(&self.serial) && line.contains("device"));

        if connected {
            info!("Device {} connected", self.serial);
            Ok(())
        } else {
            Err(AdbError::DeviceNotFound {
                serial: self.serial.clone(),
            })
        }
    }

    /// デバイス上でシェルコマンドを実行し、その出力を返す。
    pub async fn shell(&self, command: &str) -> Result<Vec<u8>, AdbError> {
        let output = self
            .run_adb_command(&["-s", &self.serial, "shell", command])
            .await?;
        Ok(output.stdout)
    }

    /// デバイスからファイルをプルする。
    pub async fn pull(&self, remote_path: &str, local_path: &str) -> Result<(), AdbError> {
        self.run_adb_command(&["-s", &self.serial, "pull", remote_path, local_path])
            .await?;
        Ok(())
    }

    /// `adb exec-out` 経由でコマンドを実行し、生のバイナリ出力を返す。
    ///
    /// `shell` と異なり、PTY の改行コード変換（CR/LF）が行われない。
    /// スクリーンショット等のバイナリデータ取得に必須。
    pub async fn exec_out(&self, command: &str) -> Result<Vec<u8>, AdbError> {
        let output = self
            .run_adb_command(&["-s", &self.serial, "exec-out", command])
            .await?;
        Ok(output.stdout)
    }

    /// デバイスのシリアルを返す。
    pub fn serial(&self) -> &str {
        &self.serial
    }

    /// adb コマンドのパスを返す。
    pub fn adb_path(&self) -> &str {
        &self.adb_path
    }

    /// 指定した引数で adb コマンドを実行し、生の [`std::process::Output`] を返す。
    ///
    /// `push` / `forward` / サーバ起動(`app_process`)等、[`Self::shell`] 経由では表現できない
    /// コマンドを必要とする上位モジュール(scrcpy capture 等)向けの公開エントリ。
    /// シリアルの差し込みは呼び出し側の責務(`-s <serial>` を args に含める)。
    pub async fn run_adb_raw(&self, args: &[&str]) -> Result<std::process::Output, AdbError> {
        self.run_adb_command(args).await
    }

    /// adb コマンドを実行する内部メソッド。
    async fn run_adb_command(&self, args: &[&str]) -> Result<std::process::Output, AdbError> {
        debug!("Running: {} {}", self.adb_path, args.join(" "));

        // NOTE: 現状は同期 `Command` を使用。将来的に `tokio::process::Command` に移行する。
        let output = Command::new(&self.adb_path)
            .args(args)
            .output()
            .map_err(|_| AdbError::AdbNotFound)?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            warn!("ADB command failed: {}", stderr);
            return Err(AdbError::CommandFailed {
                message: stderr.to_string(),
            });
        }

        Ok(output)
    }
}
