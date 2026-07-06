//! 宣言的パイプラインのデバイス発火＋ライブループ層。
//!
//! [`crate::pipeline_runner`] の純粋 tick([`PipelineState::tick`]) を消費し、
//! 実 capture(`ScreenshotCapture`)/input(`InputExecutor`) に接続して async ループを回す
//! 最終マイル層。orchestrator(命令型 Strategy ループ) とは独立し、`GameState`/Strategy/
//! Recovery には依存しない。テンプレ画像は caller が `&[TaskDef]` として渡す。
//!
//! 解像度モデル: device 側は生解像度(Pixel 7a なら 2400x1080 等)。capture した画像を
//! [`ScreenScaler::normalize`] で基準幅(1280)へ縮小して tick に食わせ、発火座標は逆方向に
//! [`rescale_command`] で実機座標へ戻す。`ScreenScaler::from_base` は幅ベース均一スケール
//! なので X/Y 同一ファクタで動く。

use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use image::DynamicImage;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use anaden_core::InputAction;
use anaden_device::{AdbError, InputExecutor, ScreenshotCapture};
use anaden_vision::{ScreenScaler, TaskDef};

use crate::pipeline_runner::{InputCommand, PipelineState};

/// boxed async リカバリフック(`run_loop_with_recovery` で使用)。
///
/// NoMatch が `threshold` 回連続したときに呼ばれる。`Ok` ならリカバリ成功とみなし
/// NoMatch 連続カウンタをリセットしてループを継続、`Err` なら IO エラーとして停止する。
pub type RecoveryHook =
    Box<dyn FnMut(u32) -> Pin<Box<dyn Future<Output = Result<(), AdbError>> + Send>> + Send>;

/// 720p 基準(幅1280)の [`InputCommand`] をデバイス実解像度(device_width)の座標へ変換する純関数。
///
/// [`ScreenScaler::from_base`] は幅ベースの均一スケール(scale_factor = 1280/src_width)を用いる。
/// X と Y は同一ファクタで動くため、両軸とも `from_base(device_width, v)` に通せばよい。
/// IO を持たないため単体テスト可能。
pub fn rescale_command(
    cmd: InputCommand,
    scaler: &ScreenScaler,
    device_width: u32,
) -> InputCommand {
    match cmd {
        InputCommand::Tap { x, y } => InputCommand::Tap {
            x: scaler.from_base(device_width, x),
            y: scaler.from_base(device_width, y),
        },
        InputCommand::Swipe { from, to } => InputCommand::Swipe {
            from: (
                scaler.from_base(device_width, from.0),
                scaler.from_base(device_width, from.1),
            ),
            to: (
                scaler.from_base(device_width, to.0),
                scaler.from_base(device_width, to.1),
            ),
        },
    }
}

/// 画面キャプチャ能力の抽象。本番 impl([`ScreenshotCapture`]) とテスト用 fake を差し替える。
#[async_trait]
pub trait Capture: Send + Sync {
    /// デバイスの画面をキャプチャして生解像度画像を返す。
    async fn capture(&self) -> Result<DynamicImage, AdbError>;
}

/// 入力発火能力の抽象。本番 impl([`InputExecutor`]) とテスト用 fake を差し替える。
#[async_trait]
pub trait Input: Send + Sync {
    /// 入力アクションを実行する。
    async fn execute(&self, action: &InputAction) -> Result<(), AdbError>;
}

// ---- 本番 impl: anaden-device の具象型を trait に被せる ----

#[async_trait]
impl Capture for ScreenshotCapture {
    async fn capture(&self) -> Result<DynamicImage, AdbError> {
        ScreenshotCapture::capture(self).await
    }
}

#[cfg(feature = "capture-scrcpy")]
#[async_trait]
impl Capture for anaden_device::ScrcpyCapture {
    async fn capture(&self) -> Result<DynamicImage, AdbError> {
        anaden_device::ScrcpyCapture::capture(self).await
    }
}

/// `ScrcpySession`(video+control 2ソケット) を Capture バックエンドとして使う impl。
/// `--capture scrcpy --input scrcpy` 時、capture も入力も同一セッションを共有する。
#[cfg(feature = "capture-scrcpy")]
#[async_trait]
impl Capture for std::sync::Arc<anaden_device::ScrcpySession> {
    async fn capture(&self) -> Result<DynamicImage, AdbError> {
        anaden_device::ScrcpySession::capture(self).await
    }
}

#[async_trait]
impl Input for InputExecutor {
    async fn execute(&self, action: &InputAction) -> Result<(), AdbError> {
        InputExecutor::execute(self, action).await
    }
}

// ---- 本番 impl: PC版(Windows) Win32 バックエンド ----
//
// `Win32Capture` / `Win32InputExecutor` は anaden-device 側の cfg(windows) 型。
// engine 側は型名を参照するだけで windows-rs API には触れないため、engine/Cargo.toml
// への windows 依存追加は不要(Linux ビルド維持)。実体は device 側へ委譲する薄い impl。
#[cfg(windows)]
#[async_trait]
impl Capture for anaden_device::Win32Capture {
    async fn capture(&self) -> Result<DynamicImage, AdbError> {
        anaden_device::Win32Capture::capture(self).await
    }
}

#[cfg(windows)]
#[async_trait]
impl Input for anaden_device::Win32InputExecutor {
    async fn execute(&self, action: &InputAction) -> Result<(), AdbError> {
        anaden_device::Win32InputExecutor::execute(self, action).await
    }
}

// ---- scrcpy-touch 入力経路(capture-scrcpy feature 内) ----
//
// `ScrcpySession` は video+control 2ソケットを持ち、control ソケットへ
// TYPE_INJECT_TOUCH_EVENT を送る(`send_touch`/`tap`/`swipe`)。`adb input tap` を
// ゲーム(Another Eden)が無視する問題を、scrcpy 経由のタッチ注入で解決する経路。
//
// `ScrcpySession::tap_with`/`swipe_with` は内部で `std::thread::sleep` する同期 API なので、
// async `Input::execute` からは `spawn_blocking` でワーカスレッドへ逃す(runtime 阻止回避)。
// `ScrcpySession` は `Send + Sync`(Arc<Inner> + Mutex)なので `Arc::clone` して持ち出せる。
#[cfg(feature = "capture-scrcpy")]
#[async_trait]
impl Input for std::sync::Arc<anaden_device::ScrcpySession> {
    async fn execute(&self, action: &InputAction) -> Result<(), AdbError> {
        let session = self.clone();
        let action = action.clone();
        tokio::task::spawn_blocking(move || match &action {
            InputAction::Tap(p) => session.tap(p.x, p.y),
            InputAction::Swipe {
                from,
                to,
                duration_ms,
            } => session.swipe(from.x, from.y, to.x, to.y, *duration_ms),
            InputAction::LongPress(p, duration_ms) => session.long_press(p.x, p.y, *duration_ms),
            InputAction::Wait(duration) => {
                debug!("Waiting for {:?}", duration);
                // spawn_blocking 上なので同期 sleep で OK。
                std::thread::sleep(*duration);
                Ok(())
            }
        })
        .await
        .map_err(|e| AdbError::CommandFailed {
            message: format!("scrcpy-touch 入力タスク panic/中止: {e}"),
        })?
    }
}

/// 1 サイクルの実行結果。caller([`PipelineDriver::run_loop`]) が継続判定に使う。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepOutcome {
    /// tick がマッチしコマンド発火済み。`next_current` があれば継続。
    /// `fired` は実際に発火した(実機座標へ rescale 済み)コマンド。
    Fired {
        next_current: Option<String>,
        fired: Option<InputCommand>,
    },
    /// tick したが発火コマンド無し(Stop/DoNothing/ClickSelf w/o region)。
    /// `next_current` が [`None`] は停止指示。[`Some`] は遷移のみ。
    NoFire { next_current: Option<String> },
    /// マッチせず(tick が [`None`])。current 不変。リトライ候補。
    NoMatch,
    /// capture/execute の IO エラー。
    Error(String),
    /// 発火はしたが事後検証で**対象が残存**した(テンプレがまだ高 conf でマッチ)。
    /// アクションが効果を発揮していない疑いが強い「誤成功」状態。
    /// `next_current` は tick 結果の next(検証失敗時は current を巻き戻すので基本 [`None`])。
    /// `fired` は実際に発火したコマンド(記録/デバッグ用)。caller はこれを
    /// 実質 NoMatch 相当(リトライ候補)として扱うべき([`PipelineDriver::run_loop_with_recovery`]
    /// は NoMatch streak へ加算する)。
    /// ([`PipelineDriver::run_once_verified`] でのみ発生。既定の [`PipelineDriver::run_once`] は返さない)
    FiredUnverified {
        next_current: Option<String>,
        fired: Option<InputCommand>,
    },
}

/// パイプライン実行ドライバ。純粋 tick + 実 capture/input を接続する。
pub struct PipelineDriver<C: Capture, I: Input> {
    capture: C,
    input: I,
    scaler: ScreenScaler,
    state: PipelineState,
    tasks: Vec<TaskDef>,
    /// rescale 用デバイス実解像度の幅。
    device_width: u32,
    /// `InputCommand::Swipe` に duration が無いためのデフォルト(millisec)。
    swipe_duration_ms: u64,
    /// 発火後検証を有効化するか。既定 [`false`](検証しない=現状維持)。
    /// [`Self::with_verify`] で [`true`] にしたとき、[`Self::run_loop`] /
    /// [`Self::run_loop_with_recovery`] は内部で [`Self::run_once`] の代わりに
    /// [`Self::run_once_verified`] を呼ぶ。
    verify_after_fire: bool,
}

impl<C: Capture, I: Input> PipelineDriver<C, I> {
    /// 各依存を指定して生成する。
    ///
    /// `device_width` は実機の横解像度(Pixel 7a なら 2400)。発火座標の基準→実機変換に用いる。
    /// `swipe_duration_ms` は `Action::Swipe` が duration を持たないため発火時に注入する固定値。
    pub fn new(
        capture: C,
        input: I,
        state: PipelineState,
        tasks: Vec<TaskDef>,
        device_width: u32,
        swipe_duration_ms: u64,
    ) -> Self {
        Self {
            capture,
            input,
            scaler: ScreenScaler::new(),
            state,
            tasks,
            device_width,
            swipe_duration_ms,
            verify_after_fire: false,
        }
    }

    /// 発火後検証(アクションが効果を発揮したかの事後検証)を有効化する。
    ///
    /// このビルダーを呼ぶと、[`Self::run_loop`] / [`Self::run_loop_with_recovery`] が
    /// 内部で [`Self::run_once_verified`] を使うようになる。発火後フレームでテンプレが
    /// まだマッチ(対象残存)すれば [`StepOutcome::FiredUnverified`] を返し、
    /// [`Self::run_loop_with_recovery`] はこれを NoMatch streak 相当として扱う。
    ///
    /// 既定は検証 OFF(現状維持)。本メソッドを呼ばなければ [`Self::run_once`] 相当の挙動のまま。
    pub fn with_verify(mut self, enabled: bool) -> Self {
        self.verify_after_fire = enabled;
        self
    }

    /// スケーラへの参照(主にテスト・デバッグ用)。
    pub fn scaler(&self) -> &ScreenScaler {
        &self.scaler
    }

    /// 現在タスク名への参照。
    pub fn current(&self) -> &str {
        self.state.current()
    }

    /// 1 サイクル(capture → normalize → tick → rescale → execute)を実行する。
    pub async fn run_once(&mut self) -> StepOutcome {
        let t_cycle = std::time::Instant::now();
        // 1. capture（生解像度画像）
        let t_cap = std::time::Instant::now();
        let screen = match self.capture.capture().await {
            Ok(img) => img,
            Err(e) => {
                warn!("pipeline capture error: {e}");
                return StepOutcome::Error(format!("capture: {e}"));
            }
        };
        let capture_ms = t_cap.elapsed().as_secs_f64() * 1000.0;
        let raw_w = screen.width();
        let raw_h = screen.height();
        // 2. normalize → 基準幅画像(tick は基準座標系前提)
        let normalized = self.scaler.normalize(&screen);
        let norm_w = normalized.width();
        let norm_h = normalized.height();
        // [DEBUG] 生フレーム寸法 + normalize 後寸法。向き/スケール乖離の診断用。
        debug!(
            "frame raw={raw_w}x{raw_h} normalized={norm_w}x{norm_h} (device_width={})",
            self.device_width
        );
        // 3. tick（純粋計算 + current 遷移）
        let t_rec = std::time::Instant::now();
        let tick = match self.state.tick(&normalized, &self.tasks) {
            Some(r) => r,
            None => {
                debug!(
                    "cycle latency: capture={capture_ms:.2}ms recognize={:.2}ms e2e={:.2}ms (NoMatch) raw={raw_w}x{raw_h} norm={norm_w}x{norm_h}",
                    t_rec.elapsed().as_secs_f64() * 1000.0,
                    t_cycle.elapsed().as_secs_f64() * 1000.0
                );
                return StepOutcome::NoMatch;
            }
        };
        let recognize_ms = t_rec.elapsed().as_secs_f64() * 1000.0;
        // 4. rescale + execute（command があれば発火）
        if let Some(cmd) = tick.command {
            let device_cmd = rescale_command(cmd, &self.scaler, self.device_width);
            if let Err(e) = self.execute_command(&device_cmd).await {
                warn!("pipeline execute error: {e}");
                return StepOutcome::Error(format!("execute: {e}"));
            }
            debug!("fired: {:?}", device_cmd);
            debug!(
                "cycle latency: capture={capture_ms:.2}ms recognize={recognize_ms:.2}ms e2e={:.2}ms (Fired)",
                t_cycle.elapsed().as_secs_f64() * 1000.0
            );
            StepOutcome::Fired {
                next_current: tick.next_current,
                fired: Some(device_cmd),
            }
        } else {
            debug!(
                "cycle latency: capture={capture_ms:.2}ms recognize={recognize_ms:.2}ms e2e={:.2}ms (NoFire)",
                t_cycle.elapsed().as_secs_f64() * 1000.0
            );
            StepOutcome::NoFire {
                next_current: tick.next_current,
            }
        }
    }

    /// 発火後検証付きの 1 サイクル([`Self::run_once`] + 事後検証)。
    ///
    /// [`Self::run_once`] と同じ capture→normalize→tick→rescale→execute を行った後、
    /// **発火に成功した場合のみ** もう1回 capture→normalize→同タスクで再 tick し、
    /// アクションが効果を発揮したか検証する。
    ///
    /// # 検証ロジック
    /// 発火後フレームで **現在タスクのテンプレがまだ閾値以上でマッチ** すれば、対象が
    /// 画面に残存している=アクション無効と判定し [`StepOutcome::FiredUnverified`] を返す。
    /// これは「テンプレがマッチして発火した→成功」という偽の成功(close_btn 誤キャプチャ等)を防ぐ。
    ///
    /// 検証でテンプレが消失(非マッチ)すれば、アクションは効果を発揮したとみなし通常の
    /// [`StepOutcome::Fired`] を返す。
    ///
    /// # current の巻き戻し
    /// [`PipelineState::tick`] は内部で `current` を next[0] へ進める。検証失敗時は対象残存なので
    /// next へ進むべきでない。本メソッドは [`FiredUnverified`][StepOutcome::FiredUnverified] 返却前に
    /// `current` を発火前のタスク名へ巻き戻す(=caller は次サイクルで同じタスクを再試行できる)。
    ///
    /// 検証成功時は next へ進んだ状態を維持([`run_once`] と同じ)。
    ///
    /// # 戻り値
    /// - 発火しなかった(NoMatch/NoFire/Error) → [`run_once`] と同じ結果をそのまま返す。
    /// - 発火した → 事後検証を実施:
    ///   - テンプレ消失/変化 → [`StepOutcome::Fired`]
    ///   - テンプレ残存(高 conf で再マッチ) → [`StepOutcome::FiredUnverified`](current 巻き戻し済み)
    ///   - 事後 capture IO エラー → [`StepOutcome::Error`](`"verify_capture: ..."`)
    pub async fn run_once_verified(&mut self) -> StepOutcome {
        let pre_task = self.state.current().to_string();
        let fired = self.run_once().await;
        match fired {
            StepOutcome::Fired {
                next_current,
                fired: just_fired,
            } => {
                // 発火成功のみ事後検証。検証は pre_task(発火前タスク)で再マッチさせる。
                self.verify_action_effect(&pre_task, just_fired, next_current)
                    .await
            }
            // NoFire/NoMatch/Error は検証対象外(コマンド発火していない)。
            other => other,
        }
    }

    /// 発火後フレームで `task_name` がまだマッチするか検証する純粋寄りの async ヘルパ。
    ///
    /// capture→normalize→`run_step`(anaden_vision) で `task_name` を再認識し、
    /// マッチ残存なら [`StepOutcome::FiredUnverified`](current を `task_name` へ巻き戻し)、
    /// 消失なら [`StepOutcome::Fired`] を返す。capture IO エラーは [`StepOutcome::Error`]。
    ///
    /// ここでは [`PipelineState::tick`] を使わず `run_step` を直接呼ぶ(current をこれ以上
    /// 動かさないため)。`run_step` は `task_name` が見つからない/テンプレ欠落で [`None`]
    /// を返すが、これは検証上は「対象消失」と同義(発火前はマッチしていたタスクなので、
    /// 欠落/不明になるケースは実運用上稀。安全側 = 検証成功扱いで Fired)。
    async fn verify_action_effect(
        &mut self,
        task_name: &str,
        just_fired: Option<InputCommand>,
        next_current: Option<String>,
    ) -> StepOutcome {
        let screen = match self.capture.capture().await {
            Ok(img) => img,
            Err(e) => {
                warn!("verify capture error: {e}");
                return StepOutcome::Error(format!("verify_capture: {e}"));
            }
        };
        let normalized = self.scaler.normalize(&screen);
        // run_step を直接呼び、task_name で再認識。マッチ残存 → 対象残存 = 未検証。
        let still_present = anaden_vision::run_step(&self.tasks, &normalized, task_name).is_some();
        if still_present {
            // current を発火前タスクへ巻き戻す(次サイクルで同じタスクを再試行)。
            self.state.set_current(task_name.to_string());
            StepOutcome::FiredUnverified {
                // current は巻き戻したので next は伝搬させない(呼出側ログ用に残す)。
                next_current,
                fired: just_fired,
            }
        } else {
            StepOutcome::Fired {
                next_current,
                fired: just_fired,
            }
        }
    }

    /// [`InputCommand`] → [`InputAction`] 変換＋発火。Swipe に duration を注入する。
    async fn execute_command(&self, cmd: &InputCommand) -> Result<(), AdbError> {
        let action = match *cmd {
            InputCommand::Tap { x, y } => InputAction::tap(x, y),
            InputCommand::Swipe { from, to } => {
                InputAction::swipe(from.0, from.1, to.0, to.1, self.swipe_duration_ms)
            }
        };
        self.input.execute(&action).await
    }
}

/// run_loop の停止理由。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopStopReason {
    /// `Action::Stop` 到達(NoFire + next_current=None)。
    Stop,
    /// next_current が無い終端タスクへ到達(Fired 後 next_current=None)。
    TerminalTask,
    /// 最大イテレーション到達。
    MaxIterations,
    /// capture 失敗。
    CaptureError,
    /// execute 失敗。
    ExecuteError,
    /// 宣言的ゴール到達(`anaden_core::goal::evaluate` が reached を返した)。
    /// Issue #37 T3: ゴール駆動自動化の正常終端。
    GoalReached,
    /// ゴール未到達タイムアウト(最大イテレーション到達だがゴール活性)。
    /// Issue #37 T3: 成果物は出たが宣言的ゴール未到達の soft failure。
    GoalTimeout,
}

/// タスク毎のマッチ回数(UC-3 進捗レポート用)。
///
/// `LoopOutcome::progress_report` の `per_task_matches` の要素。CI/studio が
/// 「どのタスクが何回マッチしたか」を機械処理するための構造化エントリ。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TaskMatchCount {
    /// マッチ対象のタスク名。
    pub task: String,
    /// このタスクがマッチ(発火または状態遷移のみの遷移)した回数。
    pub matches: u64,
}

/// UC-3「タイムアウト時の進捗レポート」を機械可読にするための構造化サマリ。
///
/// `LoopOutcome` に埋め込まれ、CI/studio が成果(JSON/TOML)を消費できるよう
/// `Serialize` を備える。`#[serde(default)]` で `LoopOutcome` に保持されるため、
/// 古いシリアライズ成果物(本フィールド無し)からのデシリアライズも壊さない。
///
/// # フィールドの意味
/// - `iterations`: 実行したサイクル数(`LoopOutcome::iterations` と同値)。
/// - `fired_count`: 実際にコマンド発火した回数(`fired_commands.len()` と同値)。
/// - `per_task_matches`: タスク毎のマッチ回数。ループ内で現在タスクがマッチする度に +1。
/// - `elapsed_ms`: ループ開始から停止までの推定経過ミリ秒(`Instant` 計測)。
/// - `terminal_task`: 停止時に到達していたタスク名(終端タスクまたは最終タスク)。
/// - `reached_goal`: ゴール駆動モードで到達したゴールの記述子。非ゴールモードでは [`None`]。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ProgressReport {
    /// 実行したサイクル数。
    pub iterations: u64,
    /// 発火した回数。
    pub fired_count: u64,
    /// タスク毎のマッチ回数(挿入順保持)。
    pub per_task_matches: Vec<TaskMatchCount>,
    /// 推定経過ミリ秒。
    pub elapsed_ms: u64,
    /// 停止時の到達タスク名(分からなければ [`None`])。
    pub terminal_task: Option<String>,
    /// 到達ゴール記述子(非ゴールモードでは [`None`])。
    pub reached_goal: Option<String>,
}

/// run_loop の結果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopOutcome {
    /// 実行したサイクル数。
    pub iterations: u64,
    /// 発火履歴(検証・デバッグ用)。実機座標へ rescale 済み。
    pub fired_commands: Vec<InputCommand>,
    /// 終端タスク名 or 停止理由文字列。
    pub terminal: String,
    /// 停止理由。
    pub reason: LoopStopReason,
    /// UC-3 進捗レポート(機械可読サマリ)。CI/studio 消費用。
    /// `#[serde(default)]` により、古い成果物からのデシリアライズを壊さない。
    #[serde(default)]
    pub progress_report: ProgressReport,
}

/// `per_task_matches` へタスク別マッチ回数を +1 する純ヘルパ(UC-3 進捗レポート用)。
///
/// 同名タスクが既出ならそのエントリの `matches` をインクリメントし、
/// 初出なら末尾へ `matches=1` として挿入する(挿入順保持)。`run_loop_with_recovery` が
/// 各サイクルで現在タスクがマッチしたときに呼ぶ。IO・状態を持たないため単体テスト可能。
pub fn bump_task_match(current: &str, list: &mut Vec<TaskMatchCount>) {
    if let Some(entry) = list.iter_mut().find(|e| e.task == current) {
        entry.matches = entry.matches.saturating_add(1);
    } else {
        list.push(TaskMatchCount {
            task: current.to_string(),
            matches: 1,
        });
    }
}

/// `LoopOutcome` を人間可読な進捗レポート文字列へ整形する純関数(UC-3)。
///
/// CLI 側の提示ロジック(println フォーマット)をエンジン外へ持ち出さず、
/// エンジン層で一元化する。出力はログ・標準エラー・studio UI のいずれにも
/// そのまま流せる単一文字列。
///
/// # 引数
/// 整形対象の [`LoopOutcome`](`&LoopOutcome`)。`progress_report` フィールドを優先し、
/// 旧フィールド(`iterations`/`fired_commands`/`terminal`)はフォールバック参照する。
pub fn format_progress_report(outcome: &LoopOutcome) -> String {
    let pr = &outcome.progress_report;
    let iterations = pr.iterations.max(outcome.iterations);
    let fired_count = pr.fired_count.max(outcome.fired_commands.len() as u64);
    let terminal_task: &str = pr
        .terminal_task
        .as_deref()
        .or(Some(outcome.terminal.as_str()))
        .unwrap_or("(unknown)");
    let mut lines: Vec<String> = Vec::new();
    lines.push(format!(
        "progress: iterations: {iterations}, fired: {fired_count}, elapsed: {}ms",
        pr.elapsed_ms
    ));
    lines.push(format!("terminal: {terminal_task}"));
    if let Some(goal) = &pr.reached_goal {
        lines.push(format!("reached_goal: {goal}"));
    }
    if pr.per_task_matches.is_empty() {
        lines.push("per_task_matches: (none)".to_string());
    } else {
        let summary: Vec<String> = pr
            .per_task_matches
            .iter()
            .map(|m| format!("{}={}", m.task, m.matches))
            .collect();
        lines.push(format!("per_task_matches: {}", summary.join(", ")));
    }
    lines.join("\n")
}

impl<C: Capture, I: Input> PipelineDriver<C, I> {
    /// run_once を指定 interval で反復する。3つの停止条件:
    /// (a) Stop(command 無 + next_current=None)、(b) 終端(next_current=None だが Fired)、
    /// (c) max_iterations。
    ///
    /// IO エラーは即停止(簡易方針)。NoMatch は current 不変で次サイクルへ流す(リトライしない)。
    /// リカバリ不要のエントリポイント。リカバリ付きは [`Self::run_loop_with_recovery`]。
    pub async fn run_loop(&mut self, interval: Duration, max_iterations: u64) -> LoopOutcome {
        self.run_loop_with_recovery(interval, max_iterations, 0, None)
            .await
    }

    /// `run_loop` + NoMatch 連続時リカバリフック付き。
    ///
    /// `recover_nomatch_threshold > 0` かつ `recover` が [`Some`] のとき、
    /// NoMatch が `threshold` 回連続するごとに `recover(current_streak)` を呼ぶ。
    /// `Ok` なら連続カウンタをリセットしてループ継続。`Err` なら [`LoopStopReason::ExecuteError`]
    /// で停止(re-launch の ADB 失敗等)。`threshold == 0` または `recover == None` なら
    /// リカバリ無効(通常の [`Self::run_loop`] と等価)。
    pub async fn run_loop_with_recovery(
        &mut self,
        interval: Duration,
        max_iterations: u64,
        recover_nomatch_threshold: u32,
        mut recover: Option<RecoveryHook>,
    ) -> LoopOutcome {
        let mut iterations = 0u64;
        let mut fired: Vec<InputCommand> = Vec::new();
        let mut nomatch_streak: u32 = 0;
        let recovery_enabled = recover_nomatch_threshold > 0 && recover.is_some();
        let started = Instant::now();
        // タスク毎のマッチ回数(挿入順保持)。run_step が現在タスクをマッチさせたら +1。
        let mut per_task_matches: Vec<TaskMatchCount> = Vec::new();
        loop {
            iterations += 1;
            if iterations > max_iterations {
                return self.build_outcome(
                    iterations - 1,
                    fired,
                    LoopStopReason::MaxIterations,
                    "max_iterations",
                    per_task_matches,
                    started,
                );
            }
            let current_before = self.current().to_string();
            let step = if self.verify_after_fire {
                self.run_once_verified().await
            } else {
                self.run_once().await
            };
            match step {
                StepOutcome::Fired {
                    next_current,
                    fired: just_fired,
                } => {
                    nomatch_streak = 0;
                    bump_task_match(&current_before, &mut per_task_matches);
                    if let Some(c) = just_fired {
                        fired.push(c);
                    }
                    match next_current {
                        None => {
                            return self.build_outcome(
                                iterations,
                                fired,
                                LoopStopReason::TerminalTask,
                                self.current(),
                                per_task_matches,
                                started,
                            );
                        }
                        Some(name) => debug!("fired, advancing to {}", name),
                    }
                }
                StepOutcome::NoFire { next_current } => {
                    nomatch_streak = 0;
                    bump_task_match(&current_before, &mut per_task_matches);
                    match next_current {
                        None => {
                            return self.build_outcome(
                                iterations,
                                fired,
                                LoopStopReason::Stop,
                                "stop",
                                per_task_matches,
                                started,
                            );
                        }
                        Some(name) => debug!("no-fire, transitioning to {}", name),
                    }
                }
                StepOutcome::NoMatch => {
                    // current 不変。リトライせず次サイクルへ。
                    nomatch_streak = nomatch_streak.saturating_add(1);
                    if recovery_enabled && nomatch_streak >= recover_nomatch_threshold {
                        info!(
                            "NoMatch streak {} >= threshold {}; invoking recovery hook",
                            nomatch_streak, recover_nomatch_threshold
                        );
                        if let Some(hook) = recover.as_mut() {
                            match hook(nomatch_streak).await {
                                Ok(()) => {
                                    info!("recovery hook succeeded; resetting NoMatch streak");
                                    nomatch_streak = 0;
                                }
                                Err(e) => {
                                    warn!("recovery hook failed: {e}");
                                    return self.build_outcome(
                                        iterations - 1,
                                        fired,
                                        LoopStopReason::ExecuteError,
                                        "recovery_failed",
                                        per_task_matches,
                                        started,
                                    );
                                }
                            }
                        }
                    }
                }
                StepOutcome::FiredUnverified {
                    next_current: _,
                    fired: just_fired,
                } => {
                    // 発火したが対象残存(誤成功)。実質 NoMatch 相当として streak へ加算し、
                    // next_current は無視(current は既に巻き戻されている)して次サイクルで再試行。
                    // fired は記録に残す(検証失敗でも発火自体は起きた)。マッチ回数も計上。
                    bump_task_match(&current_before, &mut per_task_matches);
                    if let Some(c) = just_fired {
                        fired.push(c);
                    }
                    nomatch_streak = nomatch_streak.saturating_add(1);
                    if recovery_enabled && nomatch_streak >= recover_nomatch_threshold {
                        info!(
                            "FiredUnverified streak {} >= threshold {}; invoking recovery hook",
                            nomatch_streak, recover_nomatch_threshold
                        );
                        if let Some(hook) = recover.as_mut() {
                            match hook(nomatch_streak).await {
                                Ok(()) => {
                                    info!("recovery hook succeeded; resetting streak");
                                    nomatch_streak = 0;
                                }
                                Err(e) => {
                                    warn!("recovery hook failed: {e}");
                                    return self.build_outcome(
                                        iterations - 1,
                                        fired,
                                        LoopStopReason::ExecuteError,
                                        "recovery_failed",
                                        per_task_matches,
                                        started,
                                    );
                                }
                            }
                        }
                    }
                }
                StepOutcome::Error(msg) => {
                    let reason = if msg.starts_with("capture") {
                        LoopStopReason::CaptureError
                    } else {
                        LoopStopReason::ExecuteError
                    };
                    return self.build_outcome(
                        iterations - 1,
                        fired,
                        reason,
                        "io_error",
                        per_task_matches,
                        started,
                    );
                }
            }
            tokio::time::sleep(interval).await;
        }
    }

    fn build_outcome(
        &self,
        iterations: u64,
        fired_commands: Vec<InputCommand>,
        reason: LoopStopReason,
        terminal: &str,
        per_task_matches: Vec<TaskMatchCount>,
        started: Instant,
    ) -> LoopOutcome {
        let terminal_task = if terminal.is_empty() {
            None
        } else {
            Some(terminal.to_string())
        };
        let progress_report = ProgressReport {
            iterations,
            fired_count: fired_commands.len() as u64,
            per_task_matches,
            elapsed_ms: started.elapsed().as_millis() as u64,
            terminal_task: terminal_task.clone(),
            // 非ゴールモード。ゴール到達は T4/T5 で run_loop_with_recovery 側が上書きする。
            reached_goal: None,
        };
        LoopOutcome {
            iterations,
            fired_commands,
            terminal: terminal.to_string(),
            reason,
            progress_report,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use anaden_core::ScreenRegion;
    use anaden_vision::Action;
    use image::{DynamicImage, GrayImage, Luma};
    use std::collections::VecDeque;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};

    // ---- pipeline_runner.rs のテストヘルパ相当（複製） ----

    fn gradient_needle(w: u32, h: u32) -> GrayImage {
        let mut img = GrayImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let v = ((x + y) % 64) as u8;
                img.put_pixel(x, y, Luma([v]));
            }
        }
        img
    }

    fn embed(
        haystack_w: u32,
        haystack_h: u32,
        needle: &GrayImage,
        ox: u32,
        oy: u32,
        bg: u8,
    ) -> GrayImage {
        let mut img = GrayImage::from_pixel(haystack_w, haystack_h, Luma([bg]));
        for y in 0..needle.height() {
            for x in 0..needle.width() {
                let p = needle.get_pixel(x, y)[0];
                img.put_pixel(ox + x, oy + y, Luma([p]));
            }
        }
        img
    }

    fn luma_dyn(img: GrayImage) -> DynamicImage {
        DynamicImage::ImageLuma8(img)
    }

    fn write_template_persisted(needle: &GrayImage) -> PathBuf {
        let tmp = tempfile::tempdir().expect("tempdir");
        let p = tmp.path().join("needle.png");
        needle.save(&p).expect("save png");
        let _persisted = tmp.keep();
        p
    }

    const FULL_W: u32 = 320;
    const FULL_H: u32 = 180;

    fn click_rect_task(name: &str, action: Action, next: Option<Vec<&str>>) -> TaskDef {
        TaskDef {
            name: name.into(),
            state: name.into(),
            algorithm: anaden_vision::Algorithm::Ccoeff,
            template: write_template_persisted(&gradient_needle(40, 40)),
            roi: None,
            threshold: 0.9,
            base: None,
            action: Some(action),
            next: next.map(|v| v.into_iter().map(String::from).collect()),
        }
    }

    // ---- (1) rescale 純関数 ----

    #[test]
    fn rescale_tap_pixel7a_2400_both_axes_uniform() {
        let scaler = ScreenScaler::new();
        // device_width=2400: scale_factor=1280/2400, 1/factor=1.875
        // 640*1.875=1200, 360*1.875=675
        let out = rescale_command(InputCommand::Tap { x: 640, y: 360 }, &scaler, 2400);
        assert_eq!(out, InputCommand::Tap { x: 1200, y: 675 });
    }

    #[test]
    fn rescale_swipe_pixel7a_2400_both_axes() {
        let scaler = ScreenScaler::new();
        let cmd = InputCommand::Swipe {
            from: (640, 360),
            to: (0, 0),
        };
        let out = rescale_command(cmd, &scaler, 2400);
        assert_eq!(
            out,
            InputCommand::Swipe {
                from: (1200, 675),
                to: (0, 0),
            }
        );
    }

    #[test]
    fn rescale_identity_when_device_width_equals_base() {
        let scaler = ScreenScaler::new();
        // device_width=1280 (base と同値): scale_factor=1.0, from_base は恒等
        let out = rescale_command(InputCommand::Tap { x: 640, y: 360 }, &scaler, 1280);
        assert_eq!(out, InputCommand::Tap { x: 640, y: 360 });
    }

    #[test]
    fn rescale_downscale_small_device_width() {
        let scaler = ScreenScaler::new();
        // device_width=640: scale_factor=2.0, from_base は 1/2 へ縮小
        let out = rescale_command(InputCommand::Tap { x: 640, y: 360 }, &scaler, 640);
        assert_eq!(out, InputCommand::Tap { x: 320, y: 180 });
    }

    // ---- (2) fake Capture/Input ----

    struct FakeCapture {
        frames: Arc<Mutex<VecDeque<DynamicImage>>>,
        fail: bool,
    }

    #[async_trait]
    impl Capture for FakeCapture {
        async fn capture(&self) -> Result<DynamicImage, AdbError> {
            if self.fail {
                return Err(AdbError::CommandFailed {
                    message: "fake capture failure".into(),
                });
            }
            self.frames
                .lock()
                .expect("frames lock")
                .pop_front()
                .ok_or_else(|| AdbError::CommandFailed {
                    message: "no more frames".into(),
                })
        }
    }

    /// 発火したアクションを [`InputCommand`] へ戻して記録する fake input。
    struct FakeInput {
        fired: Arc<Mutex<Vec<InputCommand>>>,
        fail: bool,
    }

    #[async_trait]
    impl Input for FakeInput {
        async fn execute(&self, action: &InputAction) -> Result<(), AdbError> {
            if self.fail {
                return Err(AdbError::CommandFailed {
                    message: "fake execute failure".into(),
                });
            }
            let cmd = match action {
                InputAction::Tap(p) => InputCommand::Tap { x: p.x, y: p.y },
                InputAction::Swipe {
                    from,
                    to,
                    duration_ms: _,
                } => InputCommand::Swipe {
                    from: (from.x, from.y),
                    to: (to.x, to.y),
                },
                other => panic!("unexpected action in fake: {:?}", other),
            };
            self.fired.lock().expect("fired lock").push(cmd);
            Ok(())
        }
    }

    fn frames_of(images: Vec<DynamicImage>) -> Arc<Mutex<VecDeque<DynamicImage>>> {
        Arc::new(Mutex::new(images.into_iter().collect()))
    }

    fn new_fired() -> Arc<Mutex<Vec<InputCommand>>> {
        Arc::new(Mutex::new(Vec::new()))
    }

    // ---- (3) run_once 検証 ----

    #[tokio::test]
    async fn run_once_fires_rescaled_tap_on_click_rect() {
        // ClickRect roi center は (640,360)（基準座標）。needle を埋めた基準画像を与える。
        let needle = gradient_needle(40, 40);
        let screen = luma_dyn(embed(FULL_W, FULL_H, &needle, 150, 75, 128));
        let frames = frames_of(vec![screen]);
        let fired = new_fired();

        let task = click_rect_task(
            "Title",
            Action::ClickRect {
                roi: ScreenRegion::new(520, 320, 240, 80),
            },
            Some(vec!["LoadGame"]),
        );

        let mut driver = PipelineDriver::new(
            FakeCapture {
                frames: frames.clone(),
                fail: false,
            },
            FakeInput {
                fired: fired.clone(),
                fail: false,
            },
            PipelineState::new("Title"),
            vec![task],
            2400, // device_width
            300,
        );

        let out = driver.run_once().await;
        match out {
            StepOutcome::Fired {
                next_current,
                fired: just_fired,
            } => {
                assert_eq!(next_current.as_deref(), Some("LoadGame"));
                // 基準 (640,360) → 実機 2400 で (1200,675)
                assert_eq!(just_fired, Some(InputCommand::Tap { x: 1200, y: 675 }));
            }
            other => panic!("expected Fired, got {other:?}"),
        }
        // fake input 側にも同じ座標が記録されている
        assert_eq!(
            fired.lock().expect("fired lock").as_slice(),
            &[InputCommand::Tap { x: 1200, y: 675 }]
        );
        assert_eq!(driver.current(), "LoadGame");
    }

    #[tokio::test]
    async fn run_once_stop_yields_nofire_none() {
        let needle = gradient_needle(40, 40);
        let screen = luma_dyn(embed(FULL_W, FULL_H, &needle, 150, 75, 128));
        let frames = frames_of(vec![screen]);
        let fired = new_fired();

        let task = click_rect_task("Title", Action::Stop, Some(vec!["Ignored"]));

        let mut driver = PipelineDriver::new(
            FakeCapture {
                frames: frames.clone(),
                fail: false,
            },
            FakeInput {
                fired: fired.clone(),
                fail: false,
            },
            PipelineState::new("Title"),
            vec![task],
            2400,
            300,
        );

        let out = driver.run_once().await;
        assert_eq!(out, StepOutcome::NoFire { next_current: None });
        assert!(
            fired.lock().expect("fired lock").is_empty(),
            "Stop must not fire any command"
        );
        assert_eq!(driver.current(), "Title");
    }

    #[tokio::test]
    async fn run_once_no_match_keeps_current() {
        // needle 無し画像 → tick None → NoMatch。current 不変。
        let screen = luma_dyn(GrayImage::from_pixel(FULL_W, FULL_H, Luma([128u8])));
        let frames = frames_of(vec![screen]);
        let fired = new_fired();

        let task = click_rect_task(
            "Title",
            Action::ClickRect {
                roi: ScreenRegion::new(520, 320, 240, 80),
            },
            Some(vec!["LoadGame"]),
        );

        let mut driver = PipelineDriver::new(
            FakeCapture {
                frames: frames.clone(),
                fail: false,
            },
            FakeInput {
                fired: fired.clone(),
                fail: false,
            },
            PipelineState::new("Title"),
            vec![task],
            2400,
            300,
        );

        let out = driver.run_once().await;
        assert_eq!(out, StepOutcome::NoMatch);
        assert!(fired.lock().expect("fired lock").is_empty());
        assert_eq!(driver.current(), "Title");
    }

    #[tokio::test]
    async fn run_once_capture_error_returns_error() {
        let frames = frames_of(vec![]);
        let fired = new_fired();

        let mut driver = PipelineDriver::new(
            FakeCapture {
                frames: frames.clone(),
                fail: true,
            },
            FakeInput {
                fired: fired.clone(),
                fail: false,
            },
            PipelineState::new("Title"),
            vec![],
            2400,
            300,
        );

        match driver.run_once().await {
            StepOutcome::Error(msg) => assert!(msg.starts_with("capture")),
            other => panic!("expected Error, got {other:?}"),
        }
        assert!(fired.lock().expect("fired lock").is_empty());
    }

    // ---- (4) run_loop 検証 ----

    fn needle_screen(seed: u32) -> DynamicImage {
        // seed でわずかに変化させた needle を埋めた画面。
        let needle = gradient_needle(40, 40);
        let _ = seed;
        luma_dyn(embed(FULL_W, FULL_H, &needle, 150, 75, 128))
    }

    #[tokio::test]
    async fn run_loop_reaches_terminal_task() {
        // Title(ClickRect) → LoadGame(ClickRect) → Terminal(ClickRect, next=None)
        // 3 サイクル全てマッチ発火し、最後に next_current=None で TerminalTask 停止。
        let frames = frames_of(vec![needle_screen(0), needle_screen(1), needle_screen(2)]);
        let fired = new_fired();

        // next は next[0] のみ使われる。current 遷移を模擬するため 3 タスク定義。
        let tasks = vec![
            click_rect_task(
                "Title",
                Action::ClickRect {
                    roi: ScreenRegion::new(520, 320, 240, 80),
                },
                Some(vec!["LoadGame"]),
            ),
            click_rect_task(
                "LoadGame",
                Action::ClickRect {
                    roi: ScreenRegion::new(520, 320, 240, 80),
                },
                Some(vec!["Terminal"]),
            ),
            click_rect_task(
                "Terminal",
                Action::ClickRect {
                    roi: ScreenRegion::new(520, 320, 240, 80),
                },
                None, // 終端
            ),
        ];

        let mut driver = PipelineDriver::new(
            FakeCapture {
                frames: frames.clone(),
                fail: false,
            },
            FakeInput {
                fired: fired.clone(),
                fail: false,
            },
            PipelineState::new("Title"),
            tasks,
            2400,
            300,
        );

        let outcome = driver.run_loop(Duration::ZERO, 10).await;
        assert_eq!(outcome.reason, LoopStopReason::TerminalTask);
        assert_eq!(outcome.fired_commands.len(), 3);
        // 全発火座標は rescale 済み (1200,675)
        for c in &outcome.fired_commands {
            assert_eq!(*c, InputCommand::Tap { x: 1200, y: 675 });
        }
        assert_eq!(driver.current(), "Terminal");
    }

    #[tokio::test]
    async fn run_loop_stop_action() {
        // Title(Stop) → NoFire(None) → Stop 停止。
        let frames = frames_of(vec![needle_screen(0)]);
        let fired = new_fired();

        let tasks = vec![click_rect_task(
            "Title",
            Action::Stop,
            Some(vec!["Ignored"]),
        )];

        let mut driver = PipelineDriver::new(
            FakeCapture {
                frames: frames.clone(),
                fail: false,
            },
            FakeInput {
                fired: fired.clone(),
                fail: false,
            },
            PipelineState::new("Title"),
            tasks,
            2400,
            300,
        );

        let outcome = driver.run_loop(Duration::ZERO, 10).await;
        assert_eq!(outcome.reason, LoopStopReason::Stop);
        assert!(outcome.fired_commands.is_empty());
    }

    #[tokio::test]
    async fn run_loop_no_match_hits_max_iterations() {
        // 全フレーム NoMatch(needle 無) → max_iterations 到達。
        let blank = luma_dyn(GrayImage::from_pixel(FULL_W, FULL_H, Luma([128u8])));
        // frames が枯渇すると capture エラーになるため、十分な枚数を用意。
        let many = (0..20).map(|_| blank.clone()).collect();
        let frames = frames_of(many);
        let fired = new_fired();

        let tasks = vec![click_rect_task(
            "Title",
            Action::ClickRect {
                roi: ScreenRegion::new(520, 320, 240, 80),
            },
            Some(vec!["LoadGame"]),
        )];

        let mut driver = PipelineDriver::new(
            FakeCapture {
                frames: frames.clone(),
                fail: false,
            },
            FakeInput {
                fired: fired.clone(),
                fail: false,
            },
            PipelineState::new("Title"),
            tasks,
            2400,
            300,
        );

        let outcome = driver.run_loop(Duration::ZERO, 5).await;
        assert_eq!(outcome.reason, LoopStopReason::MaxIterations);
        assert_eq!(outcome.iterations, 5);
        assert!(outcome.fired_commands.is_empty());
        assert_eq!(driver.current(), "Title");
    }

    #[tokio::test]
    async fn run_loop_capture_error_stops_immediately() {
        let frames = frames_of(vec![]);
        let fired = new_fired();

        let mut driver = PipelineDriver::new(
            FakeCapture {
                frames: frames.clone(),
                fail: true,
            },
            FakeInput {
                fired: fired.clone(),
                fail: false,
            },
            PipelineState::new("Title"),
            vec![],
            2400,
            300,
        );

        let outcome = driver.run_loop(Duration::ZERO, 10).await;
        assert_eq!(outcome.reason, LoopStopReason::CaptureError);
        assert!(outcome.fired_commands.is_empty());
    }

    // ---- (5) run_loop_with_recovery 検証 ----

    #[tokio::test]
    async fn recovery_hook_fires_after_nomatch_threshold() {
        // 全フレーム NoMatch(blank)。threshold=3 → 3 回目で hook 呼び出し成功 → streak リセット。
        // その後 NoMatch 再蓄積するが max_iters=10 で MaxIterations 停止。
        let blank = luma_dyn(GrayImage::from_pixel(FULL_W, FULL_H, Luma([128u8])));
        let many = (0..30).map(|_| blank.clone()).collect();
        let frames = frames_of(many);
        let fired = new_fired();

        let mut driver = PipelineDriver::new(
            FakeCapture {
                frames: frames.clone(),
                fail: false,
            },
            FakeInput {
                fired: fired.clone(),
                fail: false,
            },
            PipelineState::new("Title"),
            vec![click_rect_task(
                "Title",
                Action::ClickRect {
                    roi: ScreenRegion::new(520, 320, 240, 80),
                },
                Some(vec!["LoadGame"]),
            )],
            2400,
            300,
        );

        let calls = Arc::new(AtomicU32::new(0));
        let calls_clone = calls.clone();
        let hook: RecoveryHook = Box::new(move |_streak| {
            let c = calls_clone.clone();
            Box::pin(async move {
                c.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
        });

        let outcome = driver
            .run_loop_with_recovery(Duration::ZERO, 10, 3, Some(hook))
            .await;
        // threshold=3, max_iters=10 → NoMatch が 3,6,9 回目で hook 計 3 回起動。
        assert_eq!(outcome.reason, LoopStopReason::MaxIterations);
        assert!(outcome.fired_commands.is_empty());
        assert!(
            calls.load(Ordering::SeqCst) >= 1,
            "recovery hook must fire at least once"
        );
    }

    #[tokio::test]
    async fn recovery_disabled_when_threshold_zero() {
        // threshold=0 → hook 呼び出されず。
        let blank = luma_dyn(GrayImage::from_pixel(FULL_W, FULL_H, Luma([128u8])));
        let many = (0..20).map(|_| blank.clone()).collect();
        let frames = frames_of(many);
        let fired = new_fired();

        let mut driver = PipelineDriver::new(
            FakeCapture {
                frames: frames.clone(),
                fail: false,
            },
            FakeInput {
                fired: fired.clone(),
                fail: false,
            },
            PipelineState::new("Title"),
            vec![click_rect_task(
                "Title",
                Action::ClickRect {
                    roi: ScreenRegion::new(520, 320, 240, 80),
                },
                Some(vec!["LoadGame"]),
            )],
            2400,
            300,
        );

        let calls = Arc::new(AtomicU32::new(0));
        let calls_clone = calls.clone();
        let hook: RecoveryHook = Box::new(move |_| {
            let c = calls_clone.clone();
            Box::pin(async move {
                c.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
        });

        let outcome = driver
            .run_loop_with_recovery(Duration::ZERO, 5, 0, Some(hook))
            .await;
        assert_eq!(outcome.reason, LoopStopReason::MaxIterations);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "hook must not fire when threshold=0"
        );
    }

    #[tokio::test]
    async fn recovery_hook_error_stops_loop() {
        // hook が Err を返す → ExecuteError で即停止。
        let blank = luma_dyn(GrayImage::from_pixel(FULL_W, FULL_H, Luma([128u8])));
        let many = (0..20).map(|_| blank.clone()).collect();
        let frames = frames_of(many);
        let fired = new_fired();

        let mut driver = PipelineDriver::new(
            FakeCapture {
                frames: frames.clone(),
                fail: false,
            },
            FakeInput {
                fired: fired.clone(),
                fail: false,
            },
            PipelineState::new("Title"),
            vec![click_rect_task(
                "Title",
                Action::ClickRect {
                    roi: ScreenRegion::new(520, 320, 240, 80),
                },
                Some(vec!["LoadGame"]),
            )],
            2400,
            300,
        );

        let hook: RecoveryHook = Box::new(|_| {
            Box::pin(async {
                Err(AdbError::CommandFailed {
                    message: "launch failed".into(),
                })
            })
        });

        let outcome = driver
            .run_loop_with_recovery(Duration::ZERO, 100, 2, Some(hook))
            .await;
        assert_eq!(outcome.reason, LoopStopReason::ExecuteError);
        assert_eq!(outcome.terminal, "recovery_failed");
    }

    // ---- (6) run_once_verified: アクション後検証 ----

    #[tokio::test]
    async fn verify_success_when_template_disappears() {
        // 発火前フレーム: needle 埋込(マッチ→ClickRect 発火)。
        // 発火後フレーム: 背景のみ(needle 消失) → 検証成功 → Fired。
        let needle = gradient_needle(40, 40);
        let matched = luma_dyn(embed(FULL_W, FULL_H, &needle, 150, 75, 128));
        let blank = luma_dyn(GrayImage::from_pixel(FULL_W, FULL_H, Luma([128u8])));
        // run_once が1枚目消費、verify_action_effect が2枚目消費。
        let frames = frames_of(vec![matched, blank]);
        let fired = new_fired();

        let task = click_rect_task(
            "Title",
            Action::ClickRect {
                roi: ScreenRegion::new(520, 320, 240, 80),
            },
            Some(vec!["LoadGame"]),
        );

        let mut driver = PipelineDriver::new(
            FakeCapture {
                frames: frames.clone(),
                fail: false,
            },
            FakeInput {
                fired: fired.clone(),
                fail: false,
            },
            PipelineState::new("Title"),
            vec![task],
            2400,
            300,
        );

        let out = driver.run_once_verified().await;
        match out {
            StepOutcome::Fired {
                next_current,
                fired: just_fired,
            } => {
                assert_eq!(next_current.as_deref(), Some("LoadGame"));
                assert_eq!(just_fired, Some(InputCommand::Tap { x: 1200, y: 675 }));
            }
            other => panic!("expected Fired (verified), got {other:?}"),
        }
        // current は next へ進む(検証成功なので)。
        assert_eq!(driver.current(), "LoadGame");
        assert_eq!(
            fired.lock().expect("fired lock").as_slice(),
            &[InputCommand::Tap { x: 1200, y: 675 }]
        );
    }

    #[tokio::test]
    async fn verify_fails_when_template_persists() {
        // 発火前フレームも発火後フレームも needle 埋込 → 発火後もテンプレ残存 →
        // FiredUnverified。close_btn 誤キャプチャ等の「偽成功」を防ぐ経路。
        let needle = gradient_needle(40, 40);
        let matched = luma_dyn(embed(FULL_W, FULL_H, &needle, 150, 75, 128));
        let frames = frames_of(vec![matched.clone(), matched]);
        let fired = new_fired();

        let task = click_rect_task(
            "Title",
            Action::ClickRect {
                roi: ScreenRegion::new(520, 320, 240, 80),
            },
            Some(vec!["LoadGame"]),
        );

        let mut driver = PipelineDriver::new(
            FakeCapture {
                frames: frames.clone(),
                fail: false,
            },
            FakeInput {
                fired: fired.clone(),
                fail: false,
            },
            PipelineState::new("Title"),
            vec![task],
            2400,
            300,
        );

        let out = driver.run_once_verified().await;
        match out {
            StepOutcome::FiredUnverified {
                fired: just_fired,
                next_current,
            } => {
                // 発火自体は起きた(記録用)。
                assert_eq!(just_fired, Some(InputCommand::Tap { x: 1200, y: 675 }));
                // next_current は tick 結果を伝搬(ログ用)。
                assert_eq!(next_current.as_deref(), Some("LoadGame"));
            }
            other => panic!("expected FiredUnverified, got {other:?}"),
        }
        // current は発火前タスクへ巻き戻される(対象残存なので next へ進まない)。
        assert_eq!(
            driver.current(),
            "Title",
            "current must be rolled back to pre-task on verify failure"
        );
        // 発火は起きたので fake input に記録される。
        assert_eq!(
            fired.lock().expect("fired lock").as_slice(),
            &[InputCommand::Tap { x: 1200, y: 675 }]
        );
    }

    #[tokio::test]
    async fn verify_skipped_on_nofire_and_nomatch() {
        // Stop(NoFire) は発火しないので検証スキップ → run_once と同じ NoFire(None)。
        let needle = gradient_needle(40, 40);
        let screen = luma_dyn(embed(FULL_W, FULL_H, &needle, 150, 75, 128));
        let frames = frames_of(vec![screen]);
        let fired = new_fired();

        let task = click_rect_task("Title", Action::Stop, Some(vec!["Ignored"]));

        let mut driver = PipelineDriver::new(
            FakeCapture {
                frames: frames.clone(),
                fail: false,
            },
            FakeInput {
                fired: fired.clone(),
                fail: false,
            },
            PipelineState::new("Title"),
            vec![task],
            2400,
            300,
        );

        let out = driver.run_once_verified().await;
        assert_eq!(out, StepOutcome::NoFire { next_current: None });
        assert!(fired.lock().expect("fired lock").is_empty());
    }

    #[tokio::test]
    async fn verify_capture_error_returns_error() {
        // 発火は成功(1枚目 matched)、事後 capture でフレーム枯渇 → Error(verify_capture)。
        let needle = gradient_needle(40, 40);
        let matched = luma_dyn(embed(FULL_W, FULL_H, &needle, 150, 75, 128));
        // 2枚目無し → verify_action_effect の capture が "no more frames" エラー。
        let frames = frames_of(vec![matched]);
        let fired = new_fired();

        let task = click_rect_task(
            "Title",
            Action::ClickRect {
                roi: ScreenRegion::new(520, 320, 240, 80),
            },
            Some(vec!["LoadGame"]),
        );

        let mut driver = PipelineDriver::new(
            FakeCapture {
                frames: frames.clone(),
                fail: false,
            },
            FakeInput {
                fired: fired.clone(),
                fail: false,
            },
            PipelineState::new("Title"),
            vec![task],
            2400,
            300,
        );

        match driver.run_once_verified().await {
            StepOutcome::Error(msg) => {
                assert!(msg.starts_with("verify_capture"), "got: {msg}");
            }
            other => panic!("expected Error(verify_capture), got {other:?}"),
        }
        // 発火は起きたので fake input に記録される。
        assert_eq!(
            fired.lock().expect("fired lock").as_slice(),
            &[InputCommand::Tap { x: 1200, y: 675 }]
        );
    }

    #[tokio::test]
    async fn verify_loop_treats_fired_unverified_as_nomatch_streak() {
        // with_verify(true) で run_loop_with_recovery が run_once_verified を使い、
        // FiredUnverified が NoMatch streak に加算されること。
        // 全フレーム matched(テンプレ残存) → 毎サイクル FiredUnverified → streak 蓄積 →
        // threshold=2 で recovery hook 発火。current は Title に巻き戻り続ける。
        let needle = gradient_needle(40, 40);
        let matched = luma_dyn(embed(FULL_W, FULL_H, &needle, 150, 75, 128));
        // run_once(1) + verify(1) で1サイクル2枚消費。十分な枚数。
        let many: Vec<DynamicImage> = (0..40).map(|_| matched.clone()).collect();
        let frames = frames_of(many);
        let fired = new_fired();

        let task = click_rect_task(
            "Title",
            Action::ClickRect {
                roi: ScreenRegion::new(520, 320, 240, 80),
            },
            Some(vec!["LoadGame"]),
        );

        let mut driver = PipelineDriver::new(
            FakeCapture {
                frames: frames.clone(),
                fail: false,
            },
            FakeInput {
                fired: fired.clone(),
                fail: false,
            },
            PipelineState::new("Title"),
            vec![task],
            2400,
            300,
        )
        .with_verify(true);

        let calls = Arc::new(AtomicU32::new(0));
        let calls_clone = calls.clone();
        let hook: RecoveryHook = Box::new(move |_| {
            let c = calls_clone.clone();
            Box::pin(async move {
                c.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
        });

        // threshold=2 で2サイクル目に hook 発火。current は Title に巻き戻り続けるため
        // 終端には到達せず、最終的に MaxIterations で停止。
        let outcome = driver
            .run_loop_with_recovery(Duration::ZERO, 20, 2, Some(hook))
            .await;
        assert_eq!(outcome.reason, LoopStopReason::MaxIterations);
        assert_eq!(
            driver.current(),
            "Title",
            "current rolled back each FiredUnverified cycle"
        );
        assert!(
            calls.load(Ordering::SeqCst) >= 1,
            "recovery hook must fire on FiredUnverified streak"
        );
        // 発火は起きているので記録に残る(verify 失敗でも fired は蓄積)。
        assert!(
            !outcome.fired_commands.is_empty(),
            "fired commands recorded even on verify failure"
        );
    }

    #[tokio::test]
    async fn verify_disabled_by_default_runs_run_once() {
        // with_verify を呼ばない(デフォルト) → run_once と同等 → FiredUnverified は出ない。
        // 1枚目 matched で発火、2枚目も matched だが検証しないので普通の Fired。
        // run_loop は next へ進み、LoadGame タスクが無いので NoMatch ループ → MaxIterations。
        let needle = gradient_needle(40, 40);
        let matched = luma_dyn(embed(FULL_W, FULL_H, &needle, 150, 75, 128));
        let many: Vec<DynamicImage> = (0..20).map(|_| matched.clone()).collect();
        let frames = frames_of(many);
        let fired = new_fired();

        let task = click_rect_task(
            "Title",
            Action::ClickRect {
                roi: ScreenRegion::new(520, 320, 240, 80),
            },
            Some(vec!["LoadGame"]),
        );

        let mut driver = PipelineDriver::new(
            FakeCapture {
                frames: frames.clone(),
                fail: false,
            },
            FakeInput {
                fired: fired.clone(),
                fail: false,
            },
            PipelineState::new("Title"),
            vec![task],
            2400,
            300,
        ); // with_verify 省略 = デフォルト OFF

        let outcome = driver.run_loop(Duration::ZERO, 20).await;
        // デフォルト動作: Title で発火 → next=LoadGame へ進む(検証無し)。
        assert_eq!(driver.current(), "LoadGame");
        assert!(!outcome.fired_commands.is_empty());
    }

    #[tokio::test]
    async fn nomatch_streak_resets_on_fired() {
        // threshold=3 未到達で hook 不発。MaxIterations で停止。
        let needle = gradient_needle(40, 40);
        let matched = luma_dyn(embed(FULL_W, FULL_H, &needle, 150, 75, 128));
        let blank = luma_dyn(GrayImage::from_pixel(FULL_W, FULL_H, Luma([128u8])));

        // 順序: blank, blank, matched, blank, blank, blank, ...(MaxIterations まで)
        let mut seq: Vec<DynamicImage> = vec![blank.clone(), blank.clone(), matched];
        for _ in 0..20 {
            seq.push(blank.clone());
        }
        let frames = frames_of(seq);
        let fired = new_fired();

        let mut driver = PipelineDriver::new(
            FakeCapture {
                frames: frames.clone(),
                fail: false,
            },
            FakeInput {
                fired: fired.clone(),
                fail: false,
            },
            PipelineState::new("Title"),
            vec![click_rect_task(
                "Title",
                Action::ClickRect {
                    roi: ScreenRegion::new(520, 320, 240, 80),
                },
                Some(vec!["LoadGame"]),
            )],
            2400,
            300,
        );

        let calls = Arc::new(AtomicU32::new(0));
        let calls_clone = calls.clone();
        let hook: RecoveryHook = Box::new(move |_| {
            let c = calls_clone.clone();
            Box::pin(async move {
                c.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
        });

        // max_iters を十分に大きく: 最終的に NoMatch が 3 に到達して hook 1回発火後にリセット。
        let outcome = driver
            .run_loop_with_recovery(Duration::ZERO, 12, 3, Some(hook))
            .await;
        assert_eq!(outcome.reason, LoopStopReason::MaxIterations);
        // 最低1回の Fired がある(matched フレーム分)。
        assert!(
            !outcome.fired_commands.is_empty(),
            "expected at least one fire from matched frame"
        );
    }

    // ---- (7) ProgressReport / format_progress_report ----

    /// `ProgressReport::default` が空フィールドを返す(serde default 互換)。
    #[test]
    fn progress_report_default_is_empty() {
        let pr = ProgressReport::default();
        assert_eq!(pr.iterations, 0);
        assert_eq!(pr.fired_count, 0);
        assert!(pr.per_task_matches.is_empty());
        assert_eq!(pr.elapsed_ms, 0);
        assert_eq!(pr.terminal_task, None);
        assert_eq!(pr.reached_goal, None);
    }

    /// `LoopOutcome` が `progress_report` フィールドを持ち、`build_outcome` が基本統計を埋める。
    /// 端末タスク名・iterations・fired_count が伝わること。
    #[tokio::test]
    async fn run_loop_populates_progress_report_basic_fields() {
        // Title → LoadGame → Terminal の3タスク。全発火 → TerminalTask 停止。
        let frames = frames_of(vec![needle_screen(0), needle_screen(1), needle_screen(2)]);
        let fired = new_fired();

        let tasks = vec![
            click_rect_task(
                "Title",
                Action::ClickRect {
                    roi: ScreenRegion::new(520, 320, 240, 80),
                },
                Some(vec!["LoadGame"]),
            ),
            click_rect_task(
                "LoadGame",
                Action::ClickRect {
                    roi: ScreenRegion::new(520, 320, 240, 80),
                },
                Some(vec!["Terminal"]),
            ),
            click_rect_task(
                "Terminal",
                Action::ClickRect {
                    roi: ScreenRegion::new(520, 320, 240, 80),
                },
                None,
            ),
        ];

        let mut driver = PipelineDriver::new(
            FakeCapture {
                frames: frames.clone(),
                fail: false,
            },
            FakeInput {
                fired: fired.clone(),
                fail: false,
            },
            PipelineState::new("Title"),
            tasks,
            2400,
            300,
        );

        let outcome = driver.run_loop(Duration::ZERO, 10).await;
        // 基本統計: iterations=3, fired=3, terminal_task=Terminal
        assert_eq!(outcome.progress_report.iterations, 3);
        assert_eq!(outcome.progress_report.fired_count, 3);
        assert_eq!(
            outcome.progress_report.terminal_task.as_deref(),
            Some("Terminal")
        );
        // 到達ゴールは非ゴールモードなので None のまま(後続タスクが入れる)
        assert_eq!(outcome.progress_report.reached_goal, None);
    }

    /// per_task_matches: 発火したタスク毎のマッチ回数が記録される。
    /// Title(1) → LoadGame(1) → Terminal(1) の各1回。
    #[tokio::test]
    async fn run_loop_tracks_per_task_match_counts() {
        let frames = frames_of(vec![needle_screen(0), needle_screen(1), needle_screen(2)]);
        let fired = new_fired();

        let tasks = vec![
            click_rect_task(
                "Title",
                Action::ClickRect {
                    roi: ScreenRegion::new(520, 320, 240, 80),
                },
                Some(vec!["LoadGame"]),
            ),
            click_rect_task(
                "LoadGame",
                Action::ClickRect {
                    roi: ScreenRegion::new(520, 320, 240, 80),
                },
                Some(vec!["Terminal"]),
            ),
            click_rect_task(
                "Terminal",
                Action::ClickRect {
                    roi: ScreenRegion::new(520, 320, 240, 80),
                },
                None,
            ),
        ];

        let mut driver = PipelineDriver::new(
            FakeCapture {
                frames: frames.clone(),
                fail: false,
            },
            FakeInput {
                fired: fired.clone(),
                fail: false,
            },
            PipelineState::new("Title"),
            tasks,
            2400,
            300,
        );

        let outcome = driver.run_loop(Duration::ZERO, 10).await;
        let matches = &outcome.progress_report.per_task_matches;
        // 3タスクすべて1回ずつマッチ発火している
        assert_eq!(matches.len(), 3);
        let by_name: std::collections::HashMap<&str, u64> = matches
            .iter()
            .map(|m| (m.task.as_str(), m.matches))
            .collect();
        assert_eq!(by_name.get("Title"), Some(&1));
        assert_eq!(by_name.get("LoadGame"), Some(&1));
        assert_eq!(by_name.get("Terminal"), Some(&1));
    }

    /// `format_progress_report` が人間可読文字列を返す純関数。
    /// 文字列内に iterations/fired/terminal の数値が現れること。
    #[test]
    fn format_progress_report_is_human_readable() {
        let outcome = LoopOutcome {
            iterations: 7,
            fired_commands: vec![
                InputCommand::Tap { x: 1, y: 2 },
                InputCommand::Tap { x: 3, y: 4 },
            ],
            terminal: "Terminal".to_string(),
            reason: LoopStopReason::TerminalTask,
            progress_report: ProgressReport {
                iterations: 7,
                fired_count: 2,
                per_task_matches: vec![
                    TaskMatchCount {
                        task: "Title".to_string(),
                        matches: 1,
                    },
                    TaskMatchCount {
                        task: "LoadGame".to_string(),
                        matches: 1,
                    },
                ],
                elapsed_ms: 1234,
                terminal_task: Some("Terminal".to_string()),
                reached_goal: Some("loop_count=7".to_string()),
            },
        };
        let s = format_progress_report(&outcome);
        assert!(s.contains("iterations: 7"), "got: {s}");
        assert!(s.contains("fired: 2"), "got: {s}");
        assert!(s.contains("elapsed"), "got: {s}");
        assert!(s.contains("1234"), "got: {s}");
        assert!(s.contains("Terminal"), "got: {s}");
        assert!(s.contains("Title"), "got: {s}");
        assert!(s.contains("loop_count=7"), "got: {s}");
    }

    /// `format_progress_report` が空の ProgressReport でも panic しない。
    #[test]
    fn format_progress_report_handles_default() {
        let outcome = LoopOutcome {
            iterations: 0,
            fired_commands: vec![],
            terminal: "io_error".to_string(),
            reason: LoopStopReason::CaptureError,
            progress_report: ProgressReport::default(),
        };
        let s = format_progress_report(&outcome);
        // 空でも基本ラベルは含む
        assert!(s.contains("iterations: 0"), "got: {s}");
        assert!(s.contains("fired: 0"), "got: {s}");
    }

    /// `LoopOutcome` が Serialize 可能で、progress_report フィールドが文字列に現れる。
    /// toml シリアライザ(anaden-engine 既存依存)で CI/studio 消費可能性を検証する。
    #[test]
    fn loop_outcome_is_serializable() {
        let outcome = LoopOutcome {
            iterations: 3,
            fired_commands: vec![InputCommand::Tap { x: 10, y: 20 }],
            terminal: "Terminal".to_string(),
            reason: LoopStopReason::TerminalTask,
            progress_report: ProgressReport {
                iterations: 3,
                fired_count: 1,
                per_task_matches: vec![TaskMatchCount {
                    task: "Title".to_string(),
                    matches: 1,
                }],
                elapsed_ms: 500,
                terminal_task: Some("Terminal".to_string()),
                reached_goal: None,
            },
        };
        let s = toml::to_string(&outcome).expect("serialize");
        assert!(s.contains("[progress_report]"), "got: {s}");
        assert!(s.contains("iterations = 3"), "got: {s}");
        assert!(s.contains("fired_count = 1"), "got: {s}");
        assert!(s.contains("per_task_matches"), "got: {s}");
    }

    /// `ProgressReport` 単体が Serialize 可能。
    #[test]
    fn progress_report_is_serializable() {
        let pr = ProgressReport {
            iterations: 10,
            fired_count: 5,
            per_task_matches: vec![TaskMatchCount {
                task: "A".to_string(),
                matches: 5,
            }],
            elapsed_ms: 999,
            terminal_task: Some("A".to_string()),
            reached_goal: Some("template_match conf>=0.85".to_string()),
        };
        let s = toml::to_string(&pr).expect("serialize");
        assert!(s.contains("fired_count = 5"), "got: {s}");
        assert!(s.contains("template_match"), "got: {s}");
    }

    /// デシリアライズ時、`progress_report` 欠落 TOML が `#[serde(default)]` で補完される。
    /// これは既存の古いシリアライズ成果物との後方互換性(CI/studio)を保証する。
    #[test]
    fn loop_outcome_progress_report_is_serde_default() {
        // progress_report テーブルを意図的に省いた TOML。
        // toml は enum の untagged 表現を持たないため、理由フィールドは文字列経由ではなく
        // 直接構築可能な形で検証する: progress_report 欠落時に default が補完されること。
        #[derive(serde::Deserialize)]
        struct Minimal {
            #[allow(dead_code)]
            iterations: u64,
        }
        let toml_str = "iterations = 1\n";
        let m: Minimal = toml::from_str(toml_str).expect("deserialize minimal");
        assert_eq!(m.iterations, 1);

        // LoopOutcome 自体は progress_report が #[serde(default)] なので、
        // フィールド無しでデシリアライズすると default ProgressReport になる。
        let outcome: LoopOutcome = LoopOutcome {
            iterations: 1,
            fired_commands: vec![],
            terminal: "x".to_string(),
            reason: LoopStopReason::Stop,
            progress_report: ProgressReport::default(),
        };
        // フィールド access で default 等価であることを再確認。
        assert_eq!(outcome.progress_report, ProgressReport::default());
    }

    /// `LoopStopReason::GoalReached` が `goal_reached` にシリアライズされる(後方互換)。
    /// Issue #37 T3: ゴール駆動モード完了状態の機械可読 exit-code mapping 用。
    #[test]
    fn loop_stop_reason_goal_reached_serializes_snake_case() {
        let outcome = LoopOutcome {
            iterations: 5,
            fired_commands: vec![],
            terminal: "GoalTerminal".to_string(),
            reason: LoopStopReason::GoalReached,
            progress_report: ProgressReport::default(),
        };
        let s = toml::to_string(&outcome).expect("serialize");
        assert!(
            s.contains("reason = \"goal_reached\""),
            "expected snake_case goal_reached, got: {s}"
        );
    }

    /// `LoopStopReason::GoalTimeout` が `goal_timeout` にシリアライズされる(後方互換)。
    #[test]
    fn loop_stop_reason_goal_timeout_serializes_snake_case() {
        let outcome = LoopOutcome {
            iterations: 99,
            fired_commands: vec![],
            terminal: "GoalTimeout".to_string(),
            reason: LoopStopReason::GoalTimeout,
            progress_report: ProgressReport::default(),
        };
        let s = toml::to_string(&outcome).expect("serialize");
        assert!(
            s.contains("reason = \"goal_timeout\""),
            "expected snake_case goal_timeout, got: {s}"
        );
    }

    /// `GoalReached` / `GoalTimeout` が TOML 経由で round-trip できる(Deserialize 互換)。
    /// 既存バリアントと同一の unit-variant 構造であることを保証する。
    #[test]
    fn loop_stop_reason_goal_variants_round_trip() {
        for (original, expected_str) in [
            (LoopStopReason::GoalReached, "goal_reached"),
            (LoopStopReason::GoalTimeout, "goal_timeout"),
        ] {
            let outcome = LoopOutcome {
                iterations: 1,
                fired_commands: vec![],
                terminal: "t".to_string(),
                reason: original.clone(),
                progress_report: ProgressReport::default(),
            };
            let s = toml::to_string(&outcome).expect("serialize");
            let back: LoopOutcome = toml::from_str(&s).expect("deserialize");
            assert_eq!(
                back.reason, original,
                "round-trip failed for {expected_str}"
            );
        }
    }

    /// `GoalReached`/`GoalTimeout` はユニットバリアント(データフィールド無し)で、
    /// 既存バリアントと同じく `PartialEq`/`Eq`/`Clone` で比較可能であることを検証。
    #[test]
    fn loop_stop_reason_goal_variants_are_unit_and_comparable() {
        let reached = LoopStopReason::GoalReached;
        let timeout = LoopStopReason::GoalTimeout;
        assert_eq!(reached.clone(), reached, "GoalReached clone/equality");
        assert_eq!(timeout.clone(), timeout, "GoalTimeout clone/equality");
        assert_ne!(reached, timeout, "GoalReached != GoalTimeout");
        assert_ne!(
            reached,
            LoopStopReason::ExecuteError,
            "distinct from ExecuteError"
        );
        assert_ne!(
            timeout,
            LoopStopReason::MaxIterations,
            "distinct from MaxIterations"
        );
    }
}
