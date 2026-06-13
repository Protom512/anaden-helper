//! 自動化のメインオーケストレーター。
//!
//! Sense → Think → Act ループを駆動し、ゲームの自動操作を実現する。

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tracing::{error, info, warn};

use anaden_core::{GameState, InputAction, MatchConfidence};
use anaden_device::{AdbClient, InputExecutor, ScreenshotCapture};
use anaden_strategies::StrategyRegistry;
use anaden_vision::SceneDetector;

use crate::recovery::RecoveryPolicy;
use crate::state_machine::GameStateMachine;

/// 自動化の実行設定。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutomationConfig {
    /// デバイスのシリアルまたは接続先
    pub device_serial: String,
    /// テンプレート画像のディレクトリパス
    pub template_dir: String,
    /// メインループの間隔（ミリ秒）
    pub loop_interval_ms: u64,
    /// テンプレートマッチの信頼度閾値
    pub confidence_threshold: f32,
    /// Unknown 状態の最大リトライ回数
    pub max_unknown_retries: u32,
    /// 最大実行時間（秒）。0 で無制限。
    pub max_runtime_secs: u64,
}

impl Default for AutomationConfig {
    fn default() -> Self {
        Self {
            device_serial: "localhost:5555".to_string(),
            template_dir: "./templates/scenes".to_string(),
            loop_interval_ms: 500,
            confidence_threshold: 0.85,
            max_unknown_retries: 5,
            max_runtime_secs: 0,
        }
    }
}

/// 実行結果のサマリー。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunSummary {
    /// 総ループ回数
    pub total_loops: u64,
    /// 各状態の滞在回数
    pub state_counts: Vec<(String, u64)>,
    /// 実行時間
    pub elapsed_secs: f64,
    /// 終了理由
    pub termination_reason: String,
}

/// 自動化オーケストレーター。
///
/// この構造体がアプリケーションの心臓部。
/// デバイス、認識、戦略、状態管理を統合してループを駆動する。
pub struct Orchestrator {
    config: AutomationConfig,
    state_machine: GameStateMachine,
    recovery: RecoveryPolicy,
    strategies: StrategyRegistry,
    threshold: MatchConfidence,
}

impl Orchestrator {
    /// 設定からオーケストレーターを構築する。
    pub fn new(config: AutomationConfig) -> Self {
        let threshold = MatchConfidence::new(config.confidence_threshold);
        Self {
            state_machine: GameStateMachine::new(GameState::Unknown),
            recovery: RecoveryPolicy::new(config.max_unknown_retries),
            strategies: StrategyRegistry::new(),
            threshold,
            config,
        }
    }

    /// 戦略を登録する。
    pub fn register_strategy(
        &mut self,
        strategy: Box<dyn anaden_core::MiniGameStrategy>,
    ) {
        info!("Registered strategy: {}", strategy.name());
        self.strategies.register(strategy);
    }

    /// 自動化ループを開始する。
    pub async fn run(&mut self) -> anyhow::Result<RunSummary> {
        let start = Instant::now();
        let mut total_loops: u64 = 0;
        let mut state_counts: std::collections::HashMap<String, u64> =
            std::collections::HashMap::new();

        // デバイス接続の初期化
        let client = AdbClient::new(&self.config.device_serial);
        client.check_connection().await?;

        let input = InputExecutor::new(AdbClient::new(&self.config.device_serial));
        let screenshot = ScreenshotCapture::new(AdbClient::new(&self.config.device_serial));

        // テンプレート読み込み
        let mut store = anaden_vision::TemplateStore::new();
        let template_path = std::path::Path::new(&self.config.template_dir);
        if template_path.exists() {
            store.load_from_directory(template_path)?;
        } else {
            warn!("Template directory not found: {:?}", template_path);
        }

        let detector = SceneDetector::with_defaults(store);

        info!(
            "Starting automation loop ({} templates loaded)",
            detector.template_count()
        );

        loop {
            total_loops += 1;
            let elapsed = start.elapsed().as_secs_f64();

            // === タイムアウトチェック ===
            if self.config.max_runtime_secs > 0
                && elapsed > self.config.max_runtime_secs as f64
            {
                info!("Max runtime reached ({}s)", self.config.max_runtime_secs);
                return Ok(self.build_summary(
                    total_loops,
                    state_counts,
                    elapsed,
                    "max_runtime_reached",
                ));
            }

            // === SENSE: 画面キャプチャ → 状態認識 ===
            let screen = match screenshot.capture().await {
                Ok(img) => img,
                Err(e) => {
                    error!("Screenshot capture failed: {}", e);
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    continue;
                }
            };

            let recognition = detector.detect_scene(&screen);
            let current_state = self
                .state_machine
                .transition(&recognition, &self.threshold);

            // 状態カウントの更新
            let state_name = format!("{:?}", current_state);
            *state_counts.entry(state_name).or_insert(0) += 1;

            // === 終了判定 ===
            if current_state.is_terminal() && total_loops > 1 {
                info!("Terminal state reached: {:?}", current_state);
                return Ok(self.build_summary(
                    total_loops,
                    state_counts,
                    elapsed,
                    "terminal_state",
                ));
            }

            // === THINK: 行動決定 ===
            let actions = self.decide_actions(&current_state);

            // === RECOVERY: Unknown 状態の回復 ===
            if matches!(current_state, GameState::Unknown) {
                let streak = self.state_machine.unknown_streak();
                match self.recovery.recover_actions(streak) {
                    crate::recovery::RecoveryAction::None => {}
                    crate::recovery::RecoveryAction::Actions(recovery_actions) => {
                        for action in &recovery_actions {
                            if let Err(e) = input.execute(action).await {
                                warn!("Recovery action failed: {}", e);
                            }
                        }
                    }
                    crate::recovery::RecoveryAction::GiveUp => {
                        warn!("Giving up after {} unknown states", streak);
                        return Ok(self.build_summary(
                            total_loops,
                            state_counts,
                            elapsed,
                            "recovery_gave_up",
                        ));
                    }
                }
                continue;
            }

            // === ACT: 行動の実行 ===
            for action in &actions {
                if let Err(e) = input.execute(action).await {
                    warn!("Action execution failed: {}", e);
                }
            }

            // ループ間隔の待機
            tokio::time::sleep(Duration::from_millis(self.config.loop_interval_ms)).await;
        }
    }

    /// 現在の状態に基づいて取るべき行動を決定する。
    fn decide_actions(&self, state: &GameState) -> Vec<InputAction> {
        // まず登録済みの戦略に問い合わせる
        if let Some(strategy) = self.strategies.find_for_state(state) {
            if let Some(actions) = strategy.decide_actions(state) {
                return actions;
            }
        }

        // デフォルトの行動: 状態に応じた汎用対応
        match state {
            GameState::TitleScreen => {
                // タイトル画面なら中央をタップして開始
                vec![
                    InputAction::wait_secs(2),
                    InputAction::tap(540, 1200),
                ]
            }
            GameState::Dialog(_) => {
                // ダイアログなら「はい」付近をタップ
                vec![
                    InputAction::wait_ms(500),
                    InputAction::tap(750, 1500),
                ]
            }
            GameState::Loading => {
                // ロード中は待機
                vec![InputAction::wait_secs(2)]
            }
            _ => {
                // その他は短い待機
                vec![InputAction::wait_ms(300)]
            }
        }
    }

    fn build_summary(
        &self,
        total_loops: u64,
        state_counts: std::collections::HashMap<String, u64>,
        elapsed_secs: f64,
        reason: &str,
    ) -> RunSummary {
        RunSummary {
            total_loops,
            state_counts: state_counts.into_iter().collect(),
            elapsed_secs,
            termination_reason: reason.to_string(),
        }
    }
}
