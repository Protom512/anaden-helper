//! スクリーンショットの取得。

use image::DynamicImage;
use tracing::debug;

use crate::client::{AdbClient, AdbError};

/// デバイスのスクリーンショットを取得する。
pub struct ScreenshotCapture {
    client: AdbClient,
}

impl ScreenshotCapture {
    pub fn new(client: AdbClient) -> Self {
        Self { client }
    }

    /// デバイスの画面をキャプチャして画像として返す。
    ///
    /// `adb exec-out screencap -p` を使用する。
    /// `exec-out` は PTY を経由しないため、バイナリデータ（PNG）が
    /// 改行コード変換で壊れない。
    pub async fn capture(&self) -> Result<DynamicImage, AdbError> {
        debug!("Capturing screenshot from device {}", self.client.serial());

        let png_data = self.client.exec_out("screencap -p").await?;

        let image = image::load_from_memory(&png_data).map_err(|e| AdbError::CommandFailed {
            message: format!("Failed to decode screenshot PNG: {}", e),
        })?;

        debug!("Screenshot captured: {}x{}", image.width(), image.height());

        Ok(image)
    }
}
