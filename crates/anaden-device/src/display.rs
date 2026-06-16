//! 画面表示(ON/OFF・タイムアウト)の制御。
//!
//! 画面OFF(Doze)で `screencap` が純黒フレームを返す問題(Pixel 7a 実証) を防ぐため、
//! ループ開始前に `screen_off_timeout` を最大値へ延長する。ループ内で毎回 WAKEUP を送るのは
//! 1サイクルあたりの adb 呼び出しを増やし性能回帰の要因になるため行わない。
//!
//! 設計: このモジュールは ADB 通信のみを担い、ゲームロジックを持たない。
//! CLI(`anaden run`)と Studio(`anaden-studio::LiveCapture`) の双方から再利用される。

use tracing::{debug, warn};

use crate::client::{AdbClient, AdbError};

/// `screen_off_timeout` に設定する最大値(INT_MAX ms)。実質的に「自動で消灯しない」。
const SCREEN_OFF_TIMEOUT_MAX: &str = "2147483647";

/// 画面表示の制御(`screen_off_timeout` の取得・設定・復元)。
#[derive(Clone)]
pub struct DisplayController {
    client: AdbClient,
}

impl DisplayController {
    pub fn new(client: AdbClient) -> Self {
        Self { client }
    }

    /// 現在の `settings system screen_off_timeout` を読む。取得失敗時は None。
    pub async fn read_screen_off_timeout(&self) -> Result<Option<String>, AdbError> {
        let raw = self
            .client
            .shell("settings get system screen_off_timeout")
            .await?;
        let s = String::from_utf8_lossy(&raw).trim().to_string();
        if s.is_empty() || s == "null" {
            Ok(None)
        } else {
            Ok(Some(s))
        }
    }

    /// `screen_off_timeout` を最大値(INT_MAX)にして画面OFFを抑制する。
    pub async fn set_screen_off_timeout_max(&self) -> Result<(), AdbError> {
        debug!("setting screen_off_timeout -> {}", SCREEN_OFF_TIMEOUT_MAX);
        self.client
            .shell(&format!(
                "settings put system screen_off_timeout {}",
                SCREEN_OFF_TIMEOUT_MAX
            ))
            .await?;
        Ok(())
    }

    /// `screen_off_timeout` を指定値に戻す。セッション終了時に呼ぶ。
    pub async fn restore_screen_off_timeout(&self, value: &str) -> Result<(), AdbError> {
        debug!("restoring screen_off_timeout -> {}", value);
        self.client
            .shell(&format!(
                "settings put system screen_off_timeout {}",
                value
            ))
            .await?;
        Ok(())
    }

    /// ループ開始前に1回だけ呼ぶ画面ON確保 + タイムアウト延長。
    ///
    /// 現在の `screen_off_timeout` を読んで返し(後で復元できるように)、最大値へ書き換える。
    /// 読み取り失敗時は None を返す(復元しない)。設定書き換えの失敗は warn するが継続する
    /// (画面OFF対策が効かなくても動作自体は可能なため)。
    ///
    /// この関数は WAKEUP keyevent を送らない。ゲーム起動保証(`AppController::ensure_app_open`)が
    /// 既に前景化を済ませている前提。ループ中の毎回 keyevent 送信は adb 呼び出し増で性能劣化する。
    pub async fn ensure_stay_on(&self) -> Option<String> {
        let original = match self.read_screen_off_timeout().await {
            Ok(v) => v,
            Err(e) => {
                warn!("screen_off_timeout 読込失敗(延長は続行): {e}");
                None
            }
        };
        if let Err(e) = self.set_screen_off_timeout_max().await {
            warn!("screen_off_timeout 延長失敗(続行): {e}");
        }
        original
    }
}
