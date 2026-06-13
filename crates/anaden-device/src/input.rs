//! デバイスへの入力コマンド送信。
//!
//! `anaden-core::InputAction` を ADB の `input` コマンドに変換して実行する。

use tracing::debug;

use crate::client::{AdbClient, AdbError};
use anaden_core::InputAction;

/// デバイスへの入力実行を担当する。
pub struct InputExecutor {
    client: AdbClient,
}

impl InputExecutor {
    pub fn new(client: AdbClient) -> Self {
        Self { client }
    }

    /// `InputAction` を実行する。
    pub async fn execute(&self, action: &InputAction) -> Result<(), AdbError> {
        match action {
            InputAction::Tap(point) => self.tap(point.x, point.y).await,
            InputAction::Swipe {
                from,
                to,
                duration_ms,
            } => self.swipe(from.x, from.y, to.x, to.y, *duration_ms).await,
            InputAction::LongPress(point, duration_ms) => {
                self.long_press(point.x, point.y, *duration_ms).await
            }
            InputAction::Wait(duration) => {
                debug!("Waiting for {:?}", duration);
                tokio::time::sleep(*duration).await;
                Ok(())
            }
        }
    }

    /// 指定座標をタップする。
    async fn tap(&self, x: u32, y: u32) -> Result<(), AdbError> {
        debug!("Tap: ({}, {})", x, y);
        self.client
            .shell(&format!("input tap {} {}", x, y))
            .await?;
        Ok(())
    }

    /// 指定座標間をスワイプする。
    async fn swipe(
        &self,
        x1: u32,
        y1: u32,
        x2: u32,
        y2: u32,
        duration_ms: u64,
    ) -> Result<(), AdbError> {
        debug!("Swipe: ({},{}) -> ({},{}) in {}ms", x1, y1, x2, y2, duration_ms);
        self.client
            .shell(&format!(
                "input swipe {} {} {} {} {}",
                x1, y1, x2, y2, duration_ms
            ))
            .await?;
        Ok(())
    }

    /// 指定座標を長押しする。
    async fn long_press(&self, x: u32, y: u32, duration_ms: u64) -> Result<(), AdbError> {
        debug!("LongPress: ({}, {}) for {}ms", x, y, duration_ms);
        // ADB の input swipe で同一座標を指定することで長押しをシミュレートする
        self.client
            .shell(&format!(
                "input swipe {} {} {} {} {}",
                x, y, x, y, duration_ms
            ))
            .await?;
        Ok(())
    }
}
