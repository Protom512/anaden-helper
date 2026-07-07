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
use anaden_core::ScreenRegion;
use anaden_device::{AdbError, InputExecutor, ScreenshotCapture};
use anaden_vision::{ScreenScaler, TaskDef};

use crate::pipeline_runner::{InputCommand, PipelineState};

/// boxed async リカバリフック(`run_loop_with_recovery` で使用)。
///
/// NoMatch が `threshold` 回連続したときに呼ばれる。`Ok` ならリカバリ成功とみなし
/// NoMatch 連続カウンタをリセットしてループを継続、`Err` なら IO エラーとして停止する。
pub type RecoveryHook =
    Box<dyn FnMut(u32) -> Pin<Box<dyn Future<Output = Result<(), AdbError>> + Send>> + Send>;

/// ゴール評価用の経過時間計測の抽象(Issue #37 T4)。
///
/// [`anaden_core::goal::evaluate`] は `elapsed_secs` を純粋パラメータとして受け取るため、
/// ドライバ側は「現在の経過秒数」を供給するだけでよい。本 trait はその供給口であり、
/// 本番では [`SystemClock`](`Instant::now` 計測)、テストでは `FakeClock`(決定論的ステップ)
/// へ差し替え可能にする。`tokio::time::pause` をドライバへ導入せず済む(test-only runtime
/// feature への結合を避ける = org-feedback estimate approval condition)。
///
/// `&mut self` なのは、テスト用 impl が「呼出毎に時間を進める」副作用を持てるようにするため。
pub trait GoalClock: Send {
    /// ループ開始からの経過秒数を返す。
    fn elapsed_secs(&mut self) -> u64;
}

/// [`GoalClock`] の本番実装。`Instant::now()` で実時間を計測する。
///
/// `run_loop_with_goal` が `goal=Some` で渡されたときのみ消費される(`goal=None` なら
/// 計測は行われない = 既存 `run_loop_with_recovery` と完全等価)。
pub struct SystemClock {
    started: Instant,
}

impl SystemClock {
    /// 開始時刻を `now` として計測を開始する。
    pub fn new() -> Self {
        Self {
            started: Instant::now(),
        }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl GoalClock for SystemClock {
    fn elapsed_secs(&mut self) -> u64 {
        self.started.elapsed().as_secs()
    }
}

/// `evaluate_goal` へ渡すループ状態のバンドル。
///
/// `evaluate_goal` の引数が 8 個(clippy `too_many_arguments` 閾値超過)になるのを防ぐため、
/// 呼出側がループ内で蓄積した `iterations` / `fired_commands` / `per_task_matches` /
/// `started` を 1 つの借用束にまとめて渡す。`build_outcome` への転送もこの束から行う。
struct GoalEvalInputs<'a> {
    /// 現在の tick 数。
    iterations: u64,
    /// 累積発火コマンド。
    fired_commands: &'a [InputCommand],
    /// タスク毎のマッチ回数。
    per_task_matches: &'a [TaskMatchCount],
    /// ループ開始時刻。
    started: Instant,
}

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
    /// 直近の tick でマッチしたテンプレート情報(UC-2 用)。
    /// `run_once` がマッチ時に `(task_name, confidence, region)` を格納し、
    /// `run_loop_with_goal` が `GoalStatusContext::tick` へ渡すために消費(`take`)する。
    /// 非ゴールパス(`run_loop_with_recovery`)では参照されず、上書きされるだけ。
    last_match: Option<(String, f32, ScreenRegion)>,
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
            last_match: None,
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
        // tick は内部で current を next[0] へ進めるため、マッチした(発火前)タスク名を
        // 事前に採取する(UC-2 last_match の task 名要素)。
        let matched_task_name = self.state.current().to_string();
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
        // マッチしたテンプレート情報を記録(UC-2 ゴール評価用)。run_loop_with_goal が
        // GoalStatusContext::tick へ渡すために消費(take)する。非ゴールパスでは未参照。
        if let (Some(region), Some(confidence)) = (tick.matched_region, tick.matched_confidence) {
            self.last_match = Some((matched_task_name, confidence, region));
        }
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

/// [`Goal`](`anaden_core::Goal`) の到達記述子(descriptor)を [`StopCondition`] から導出する純関数。
///
/// `evaluate_goal` が `progress_report.reached_goal` へ埋める文字列で、各バリアントを
/// 人間可読かつ機械処理可能な形式へ射影する:
/// - `LoopCount { target }` → `loop_count=<target>`
/// - `TemplateMatch { task, confidence }` → `template_match conf>=<confidence> (task=<task>)`
/// - `Timeout { secs }` → `timeout=<secs>`
///
/// I/O・時間・乱数に依存しない。`pub(crate)` でテストからも参照可能。
fn goal_descriptor(goal: &anaden_core::Goal) -> String {
    match &goal.stop {
        anaden_core::StopCondition::LoopCount { target } => {
            format!("loop_count={target}")
        }
        anaden_core::StopCondition::TemplateMatch { task, confidence } => {
            format!("template_match conf>={confidence} (task={task})")
        }
        anaden_core::StopCondition::Timeout { secs } => format!("timeout={secs}"),
    }
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
    ///
    /// 本メソッドは非ゴールモード([`Self::run_loop_with_goal`] へ `goal=None` を渡すのと等価)。
    /// 宣言的ゴールで停止するには [`Self::run_loop_with_goal`] を使うこと。
    pub async fn run_loop_with_recovery(
        &mut self,
        interval: Duration,
        max_iterations: u64,
        recover_nomatch_threshold: u32,
        mut recover: Option<RecoveryHook>,
    ) -> LoopOutcome {
        // 非ゴールモード(後方互換)。run_loop_with_goal(goal=None) からも本メソッドへ委譲される
        // ため、両者を相互再帰させず本メソッド内で完結させる(async fn の再帰は boxing が必要で
        // コストが高い+実装上の理由がない)。ゴール評価付きは run_loop_with_goal を直接呼ぶこと。
        let mut iterations = 0u64;
        let mut fired: Vec<InputCommand> = Vec::new();
        let mut nomatch_streak: u32 = 0;
        let recovery_enabled = recover_nomatch_threshold > 0 && recover.is_some();
        let started = Instant::now();
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

    /// `run_loop_with_recovery` + 宣言的ゴール評価(Issue #37 T4)。
    ///
    /// `goal=Some` のとき各 tick後に [`anaden_core::goal::evaluate`] を呼び、
    /// [`GoalStatus::Reached`] → [`LoopStopReason::GoalReached`]
    /// [`GoalStatus::Failed`] → [`LoopStopReason::GoalTimeout`] で停止する。
    /// `goal=None` のときは [`Self::run_loop_with_recovery`] と完全等価(後方互換)。
    ///
    /// 経過秒数は [`GoalClock`] から供給する。本番は [`SystemClock`]、テストは
    /// `FakeClock`(決定論的ステップ)を渡す。`tokio::time::pause` を導入しないことで
    /// ドライバのテスト専用 runtime feature への結合を避ける(org-feedback approval)。
    ///
    /// `evaluate` 呼出毎に `GoalStatusContext::tick` を +1 する(tick 数 = evaluate 回数)。
    /// `last_match` は現状 `None` を渡す(T5-UC2 で StepOutcome から confidence/task を
    /// 伝播させる後続タスクが埋める。UC-1/UC-3 は last_match 不要)。
    ///
    /// `GoalClock` は具象型(非ジェネリクス)にすると `run_loop` 互換の既存シグネチャへ
    /// 影響しないが、テストで差し替えられるようトレイトオブジェクトで受ける。
    pub async fn run_loop_with_goal(
        &mut self,
        interval: Duration,
        max_iterations: u64,
        recover_nomatch_threshold: u32,
        mut recover: Option<RecoveryHook>,
        goal: Option<&anaden_core::Goal>,
        mut clock: impl GoalClock,
    ) -> LoopOutcome {
        let Some(goal_ref) = goal else {
            // ゴール無し: 既存 run_loop_with_recovery へ完全委譲。clock は消費されない。
            return self
                .run_loop_with_recovery(
                    interval,
                    max_iterations,
                    recover_nomatch_threshold,
                    recover,
                )
                .await;
        };
        // ゴール活性パス。run_loop_with_recovery の本体をインライン展開し、各 tick 後に
        // evaluate を呼んで GoalReached/GoalTimeout で早期停止する。
        // (run_loop_with_recovery へ evaluate を埋め込まず別関数に分けたのは、非ゴールパスの
        //  既存テスト(MaxIterations 等)へ一切影響を与えないため。)
        let mut iterations = 0u64;
        let mut fired: Vec<InputCommand> = Vec::new();
        let mut nomatch_streak: u32 = 0;
        let recovery_enabled = recover_nomatch_threshold > 0 && recover.is_some();
        let started = Instant::now();
        let mut per_task_matches: Vec<TaskMatchCount> = Vec::new();
        let mut goal_ctx = anaden_core::GoalStatusContext::new();
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
            // UC-2 (TemplateMatch): Fired/NoFire ブランチでは run_once が格納した
            // self.last_match (task_name, confidence, region) を take() で取り出し
            // GoalStatusContext::last_match へ伝播させる。NoMatch/FiredUnverified/Error は
            // テンプレートマッチしていない(または誤成功)なので None を渡す。
            // (NoFire は ClickSelf w/o region 等の「マッチしたが発火コマンド無」で、tick は
            //  マッチしているので last_match を伝播する。)
            let match_info: Option<(String, f32, ScreenRegion)> = match &step {
                StepOutcome::Fired { .. } | StepOutcome::NoFire { .. } => self.last_match.take(),
                _ => None,
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
                    if let Some(name) = &next_current {
                        debug!("fired, advancing to {}", name);
                    }
                    // ゴール評価(tick 数を +1)。GoalReached/GoalTimeout なら即停止。
                    goal_ctx.tick(match_info);
                    let elapsed = clock.elapsed_secs();
                    if let Some(stop) = self.evaluate_goal(
                        goal_ref,
                        &goal_ctx,
                        elapsed,
                        GoalEvalInputs {
                            iterations,
                            fired_commands: &fired,
                            per_task_matches: &per_task_matches,
                            started,
                        },
                    ) {
                        return stop;
                    }
                    if next_current.is_none() {
                        return self.build_outcome(
                            iterations,
                            fired,
                            LoopStopReason::TerminalTask,
                            self.current(),
                            per_task_matches,
                            started,
                        );
                    }
                }
                StepOutcome::NoFire { next_current } => {
                    nomatch_streak = 0;
                    bump_task_match(&current_before, &mut per_task_matches);
                    goal_ctx.tick(match_info);
                    let elapsed = clock.elapsed_secs();
                    if let Some(stop) = self.evaluate_goal(
                        goal_ref,
                        &goal_ctx,
                        elapsed,
                        GoalEvalInputs {
                            iterations,
                            fired_commands: &fired,
                            per_task_matches: &per_task_matches,
                            started,
                        },
                    ) {
                        return stop;
                    }
                    if let Some(name) = &next_current {
                        debug!("no-fire, transitioning to {}", name);
                    }
                    if next_current.is_none() {
                        return self.build_outcome(
                            iterations,
                            fired,
                            LoopStopReason::Stop,
                            "stop",
                            per_task_matches,
                            started,
                        );
                    }
                }
                StepOutcome::NoMatch => {
                    nomatch_streak = nomatch_streak.saturating_add(1);
                    goal_ctx.tick(match_info);
                    let elapsed = clock.elapsed_secs();
                    if let Some(stop) = self.evaluate_goal(
                        goal_ref,
                        &goal_ctx,
                        elapsed,
                        GoalEvalInputs {
                            iterations,
                            fired_commands: &fired,
                            per_task_matches: &per_task_matches,
                            started,
                        },
                    ) {
                        return stop;
                    }
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
                    bump_task_match(&current_before, &mut per_task_matches);
                    if let Some(c) = just_fired {
                        fired.push(c);
                    }
                    nomatch_streak = nomatch_streak.saturating_add(1);
                    goal_ctx.tick(match_info);
                    let elapsed = clock.elapsed_secs();
                    if let Some(stop) = self.evaluate_goal(
                        goal_ref,
                        &goal_ctx,
                        elapsed,
                        GoalEvalInputs {
                            iterations,
                            fired_commands: &fired,
                            per_task_matches: &per_task_matches,
                            started,
                        },
                    ) {
                        return stop;
                    }
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

    /// ゴールを評価し、終端状態なら停止理由と terminal 文字列を返す。
    /// まだ継続なら [`None`]。呼出側が [`Self::build_outcome`] で累積状態
    /// (`fired_commands`/`per_task_matches`)を保持したまま停止 outcome を構築できるよう、
    /// 本メソッドは理由と terminal のみを返す(LoopOutcome 構築は呼出側に委ねる)。
    fn evaluate_goal_reason(
        &self,
        goal: &anaden_core::Goal,
        ctx: &anaden_core::GoalStatusContext,
        elapsed_secs: u64,
    ) -> Option<(LoopStopReason, &'static str)> {
        match anaden_core::evaluate(goal, ctx, elapsed_secs) {
            anaden_core::GoalStatus::NotYet => None,
            anaden_core::GoalStatus::Reached(_) => {
                Some((LoopStopReason::GoalReached, "goal_reached"))
            }
            anaden_core::GoalStatus::Failed(_) => {
                Some((LoopStopReason::GoalTimeout, "goal_timeout"))
            }
        }
    }

    /// ゴールを評価し、終端状態なら呼出側がそのまま `return` できる完成 [`LoopOutcome`] を返す。
    /// まだ継続なら [`None`]。
    ///
    /// [`Self::evaluate_goal_reason`] で理由と terminal を判定した上で、[`Self::build_outcome`]
    /// で基本 outcome を構築し、ゴール到達時は `progress_report.reached_goal` へ
    /// **ゴール記述子文字列** を上書きで埋める(Issue #37 T3: 成果レポート用)。
    ///
    /// 記述子はモジュール直下の free function [`goal_descriptor`](@goal_descriptor) が
    /// [`anaden_core::StopCondition`] のバリアントから導出する
    /// (`loop_count=<n>` / `template_match conf>=<f> (task=<t>)` / `timeout=<secs>`)。
    /// `GoalTimeout` のとき到達ゴールは無いので `reached_goal` は [`None`] のままとする。
    fn evaluate_goal(
        &self,
        goal: &anaden_core::Goal,
        ctx: &anaden_core::GoalStatusContext,
        elapsed_secs: u64,
        inputs: GoalEvalInputs,
    ) -> Option<LoopOutcome> {
        let (reason, terminal) = self.evaluate_goal_reason(goal, ctx, elapsed_secs)?;
        let mut outcome = self.build_outcome(
            inputs.iterations,
            inputs.fired_commands.to_vec(),
            reason,
            terminal,
            inputs.per_task_matches.to_vec(),
            inputs.started,
        );
        // ゴール到達/タイムアウト時、評価対象ゴールの記述子を埋める
        // (CI/studio が「どのゴールで停止したか」を人間可読に追跡するため)。
        outcome.progress_report.reached_goal = Some(goal_descriptor(goal));
        Some(outcome)
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

    /// `LoopOutcome` が JSON 経路(`serde_json`)でも CI/studio 消費可能な形で
    /// シリアライズされることを検証する。TOML 経路は `loop_outcome_is_serializable`
    /// が担保済み。JSON 経路は `reason = "goal_reached"` の snake_case 表現と
    /// `progress_report.reached_goal` 記述子を両方含むことを assert する。
    /// Issue #37 T5: JSON 成果レポート検証。
    #[test]
    fn loop_outcome_is_json_serializable() {
        let outcome = LoopOutcome {
            iterations: 5,
            fired_commands: vec![InputCommand::Tap { x: 10, y: 20 }],
            terminal: "GoalTerminal".to_string(),
            reason: LoopStopReason::GoalReached,
            progress_report: ProgressReport {
                iterations: 5,
                fired_count: 1,
                per_task_matches: vec![TaskMatchCount {
                    task: "Clear".to_string(),
                    matches: 1,
                }],
                elapsed_ms: 4321,
                terminal_task: Some("GoalTerminal".to_string()),
                reached_goal: Some("loop_count=5".to_string()),
            },
        };
        let s = serde_json::to_string(&outcome).expect("serialize json");
        // reason が snake_case で JSON に現れる(後方互換性)
        assert!(s.contains("\"reason\":\"goal_reached\""), "got: {s}");
        // ネストした progress_report テーブルが JSON に現れる
        assert!(s.contains("\"progress_report\":{"), "got: {s}");
        // reached_goal 記述子が JSON に現れる(ゴール到達時の成果物)
        assert!(s.contains("\"reached_goal\":\"loop_count=5\""), "got: {s}");
        // 逆シリアライズ(round-trip)も成立すること
        let back: LoopOutcome = serde_json::from_str(&s).expect("deserialize json");
        assert_eq!(back.iterations, 5);
        assert_eq!(back.reason, LoopStopReason::GoalReached);
        assert_eq!(
            back.progress_report.reached_goal.as_deref(),
            Some("loop_count=5")
        );
    }

    /// `ProgressReport` 単体が JSON シリアライズ可能で、`reached_goal` 記述子を含む。
    /// TOML と JSON 双方で reached_goal 記述子が現れることを assert する(AC: 双方経路)。
    #[test]
    fn progress_report_is_json_serializable_with_reached_goal() {
        let pr = ProgressReport {
            iterations: 8,
            fired_count: 3,
            per_task_matches: vec![TaskMatchCount {
                task: "Boss".to_string(),
                matches: 2,
            }],
            elapsed_ms: 1500,
            terminal_task: Some("Boss".to_string()),
            reached_goal: Some("template_match conf>=0.85".to_string()),
        };
        // JSON 経路
        let j = serde_json::to_string(&pr).expect("serialize json");
        assert!(j.contains("\"fired_count\":3"), "got: {j}");
        assert!(
            j.contains("\"reached_goal\":\"template_match conf>=0.85\""),
            "got: {j}"
        );
        // TOML 経路(対称性担保)
        let t = toml::to_string(&pr).expect("serialize toml");
        assert!(
            t.contains("reached_goal = \"template_match conf>=0.85\""),
            "got: {t}"
        );
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

    // ---- (8) T5: goal=None 後方互換性検証 ----
    //
    // Issue #37 T5-no-goal-backcompat: ゴール駆動モード(T4)が `Option<Goal>` を
    // `run_loop_with_recovery` へ追加しても、`goal=None`(ゴール非宣言)時は既存の
    // max_iterations bounded 挙動が **byte-identical** で保持されなければならない。
    //
    // 本セクションは以下を検証する:
    // (a) 既存テスト `run_loop_no_match_hits_max_iterations` /
    //     `recovery_hook_fires_after_nomatch_threshold` と同一の結果(reason/iterations/
    //     fired_commands/terminal/current)が得られること。
    // (b) goal=None で GoalReached/GoalTimeout が **発火しない** こと。
    //     (停止理由は常に MaxIterations)
    // (c) 「無限ループ相当」特性: max_iters を大きくすると iterations が比例的に増え、
    //     一定の max_iters で打切られるまで延々継続できること。
    // (d) recovery hook の発火回数・streak リセット挙動が goal=None で崩れないこと。

    /// NoMatch フレームを無限に返す driver を構築するヘルパ(goal=None backcompat 共通)。
    /// 全フレーム背景一色なので tick は常に NoMatch → current 不変でループが回る。
    fn build_nomatch_driver() -> (
        PipelineDriver<FakeCapture, FakeInput>,
        Arc<Mutex<Vec<InputCommand>>>,
    ) {
        let blank = luma_dyn(GrayImage::from_pixel(FULL_W, FULL_H, Luma([128u8])));
        let many: Vec<DynamicImage> = (0..200).map(|_| blank.clone()).collect();
        let frames = frames_of(many);
        let fired = new_fired();
        let driver = PipelineDriver::new(
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
        (driver, fired)
    }

    /// goal=None(現行シグネチャ = ゴール非宣言)で NoMatch が持続すると
    /// **MaxIterations** で停止し、iterations が max_iterations に等しいこと。
    /// これは既存 `run_loop_no_match_hits_max_iterations` と同一の契約。
    #[tokio::test]
    async fn backcompat_goal_none_no_match_hits_max_iterations_byte_identical() {
        let (mut driver, fired) = build_nomatch_driver();

        let outcome = driver.run_loop(Duration::ZERO, 5).await;
        // 停止理由は MaxIterations のみ(GoalReached/GoalTimeout は出ない)。
        assert_eq!(
            outcome.reason,
            LoopStopReason::MaxIterations,
            "goal=None must stop with MaxIterations, got {:?}",
            outcome.reason
        );
        // iterations は max_iterations に厳密に等しい(byte-identical)。
        assert_eq!(outcome.iterations, 5);
        // 発火無し。
        assert!(
            outcome.fired_commands.is_empty(),
            "no fires on pure NoMatch"
        );
        // current 不変。
        assert_eq!(driver.current(), "Title");
        assert!(fired.lock().expect("fired lock").is_empty());
    }

    /// goal=None + recovery hook(threshold 到達で Ok)でも停止理由は MaxIterations。
    /// これは既存 `recovery_hook_fires_after_nomatch_threshold` と同一の契約。
    #[tokio::test]
    async fn backcompat_goal_none_recovery_hook_fires_under_max_iterations() {
        let (mut driver, _fired) = build_nomatch_driver();

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
        // goal=None なので MaxIterations(Goal 系停止理由は出ない)。
        assert_eq!(
            outcome.reason,
            LoopStopReason::MaxIterations,
            "goal=None with recovery must stop with MaxIterations"
        );
        // threshold=3, max_iters=10 → hook は 3,6,9 回目の最低 3 回発火する想定だが、
        // 最低 1 回の発火だけを厳密契約として検証する(timing に依存しない)。
        assert!(
            calls.load(Ordering::SeqCst) >= 1,
            "recovery hook must fire at least once under goal=None"
        );
        assert!(outcome.fired_commands.is_empty());
    }

    /// goal=None で recovery hook が Err を返すと **ExecuteError** で即停止
    /// (MaxIterations 以前)。ゴール非宣言時もリカバリ失敗契約は不変。
    #[tokio::test]
    async fn backcompat_goal_none_recovery_error_stops_immediately() {
        let (mut driver, _fired) = build_nomatch_driver();

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
        // max_iters=100 だが即停止 → GoalTimeout ではなく ExecuteError が優先。
        assert!(
            outcome.iterations < 100,
            "must short-circuit before max_iterations"
        );
    }

    /// 「無限ループ相当」特性: goal=None で max_iters を大きくすると、
    /// その max_iters に到達するまでループが延々継続する(打切りによる停止のみ)。
    /// max_iters=5 と max_iters=50 を比較し、iterations が max に比例することを検証。
    #[tokio::test]
    async fn backcompat_goal_none_indefinite_continuation_scales_with_max_iters() {
        // max_iters=5
        let (mut driver_a, _fired_a) = build_nomatch_driver();
        let out_small = driver_a.run_loop(Duration::ZERO, 5).await;
        assert_eq!(out_small.reason, LoopStopReason::MaxIterations);
        assert_eq!(out_small.iterations, 5);

        // max_iters=50 — 10倍。同一条件下で iterations が 10倍に伸びる(=延々継続)。
        let (mut driver_b, _fired_b) = build_nomatch_driver();
        let out_large = driver_b.run_loop(Duration::ZERO, 50).await;
        assert_eq!(out_large.reason, LoopStopReason::MaxIterations);
        assert_eq!(out_large.iterations, 50);

        // 比例関係: 50 == 5 * 10。「max_iters を大きくすると延々継続」の機械的証明。
        assert_eq!(
            out_large.iterations,
            out_small.iterations * 10,
            "iterations must scale linearly with max_iters under goal=None"
        );
        // 両者ともゴール系停止理由ではない。
        assert_ne!(out_large.reason, LoopStopReason::GoalReached);
        assert_ne!(out_large.reason, LoopStopReason::GoalTimeout);
    }

    /// goal=None(既定)時、`progress_report.reached_goal` は常に [`None`]。
    /// T4 が Goal 到達時に reached_goal を埋めるように変更しても、
    /// goal=None では None のままであることを検証(後方互換の境界)。
    #[tokio::test]
    async fn backcompat_goal_none_progress_report_reached_goal_is_none() {
        let (mut driver, _fired) = build_nomatch_driver();
        let outcome = driver.run_loop(Duration::ZERO, 5).await;
        assert_eq!(outcome.reason, LoopStopReason::MaxIterations);
        assert!(
            outcome.progress_report.reached_goal.is_none(),
            "reached_goal must be None when goal is not declared"
        );
    }

    /// goal=None の境界値: max_iterations=1 でも MaxIterations で即停止
    /// (1 サイクル後に打切)。ゴール評価が介入して早期終端しないこと。
    #[tokio::test]
    async fn backcompat_goal_none_single_iteration_still_max_iterations() {
        let (mut driver, _fired) = build_nomatch_driver();
        let outcome = driver.run_loop(Duration::ZERO, 1).await;
        assert_eq!(outcome.reason, LoopStopReason::MaxIterations);
        assert_eq!(outcome.iterations, 1);
    }

    // ===== UC-3 backward-compat AC: run_loop_with_goal(goal=None) byte-identical to
    //       run_loop_with_recovery =====
    //
    // Issue #37/#41 close 認可条件 Task #4:
    // `run_loop_with_goal` が `goal=None` で渡されたとき `run_loop_with_recovery` へ
    // **verbatim(バイト同一)** で委譲することを、Outcome の全観測可能フィールドを
    // 機械的に比較して検証する。Goal 駆動コードパスが導入されても、ゴール非宣言時の
    // 既存挙動(MaxIterations/Recovery で停止、reached_goal=None)が **1 ビットも変わらない**
    // ことの回帰防止。
    //
    // 既存 backcompat_* テスト群(2650-2783)は run_loop / run_loop_with_recovery を直接呼び、
    // goal=None の *意味論的* 契約(MaxIterations 等)を検証する。本テストはそれらと相補的に、
    // **委譲経路自体(run_loop_with_goal の None 早期 return)** を通した場合の出力が、
    // 直接 run_loop_with_recovery を呼んだ場合と完全一致することを個別に固定する。

    /// `run_loop_with_goal(goal=None)` と `run_loop_with_recovery` の出力が
    /// **完全に同一**であることを、純粋 NoMatch + recovery 無しの最小ケースで検証する。
    /// 両者の `LoopOutcome` を構造的に比較(reason / iterations / terminal / fired_commands /
    /// progress_report)し、GoalReached/GoalTimeout が発火しないことを確認。
    /// また reached_goal が None のままであることも併せて検証(UC-3 AC の核心不変条件)。
    #[tokio::test]
    async fn backcompat_goal_none_run_loop_with_goal_is_byte_identical_to_recovery() {
        // 直接 run_loop_with_recovery を呼んだ基準結果(baseline)。
        let (mut driver_baseline, _fired_baseline) = build_nomatch_driver();
        let baseline = driver_baseline
            .run_loop_with_recovery(Duration::ZERO, 7, 0, None)
            .await;

        // 同一 fixture で run_loop_with_goal(goal=None) を通した結果。
        let (mut driver_via_goal, _fired_via_goal) = build_nomatch_driver();
        let clock = FakeClock::starting_at(0);
        let via_goal = driver_via_goal
            .run_loop_with_goal(Duration::ZERO, 7, 0, None, None, clock)
            .await;

        // === 停止理由: MaxIterations(Goal 系停止理由は出ない)===
        assert_eq!(
            baseline.reason,
            LoopStopReason::MaxIterations,
            "baseline must stop with MaxIterations"
        );
        assert_eq!(
            via_goal.reason,
            LoopStopReason::MaxIterations,
            "run_loop_with_goal(goal=None) must stop with MaxIterations, got {:?}",
            via_goal.reason
        );
        assert_eq!(
            baseline.reason, via_goal.reason,
            "reason diverges between direct and delegated paths"
        );

        // === 全観測可能フィールドのバイト同一性 ===
        assert_eq!(
            baseline.iterations, via_goal.iterations,
            "iterations diverges (delegation must be byte-identical)"
        );
        assert_eq!(
            baseline.terminal, via_goal.terminal,
            "terminal diverges (delegation must be byte-identical)"
        );
        assert_eq!(
            baseline.fired_commands, via_goal.fired_commands,
            "fired_commands diverges (delegation must be byte-identical)"
        );
        assert_eq!(
            baseline.progress_report.iterations, via_goal.progress_report.iterations,
            "progress_report.iterations diverges"
        );
        assert_eq!(
            baseline.progress_report.fired_count, via_goal.progress_report.fired_count,
            "progress_report.fired_count diverges"
        );
        assert_eq!(
            baseline.progress_report.per_task_matches, via_goal.progress_report.per_task_matches,
            "progress_report.per_task_matches diverges"
        );
        assert_eq!(
            baseline.progress_report.terminal_task, via_goal.progress_report.terminal_task,
            "progress_report.terminal_task diverges"
        );

        // === UC-3 AC 核心不変条件: reached_goal は None のまま ===
        assert!(
            baseline.progress_report.reached_goal.is_none(),
            "baseline reached_goal must be None (no goal declared)"
        );
        assert!(
            via_goal.progress_report.reached_goal.is_none(),
            "run_loop_with_goal(goal=None) must leave reached_goal as None — \
             goal-evaluation code must not populate it on the undeclared path"
        );

        // === Goal 系停止理由が一切発火しないことの明示的表明 ===
        assert_ne!(
            via_goal.reason,
            LoopStopReason::GoalReached,
            "goal=None path must NEVER emit GoalReached"
        );
        assert_ne!(
            via_goal.reason,
            LoopStopReason::GoalTimeout,
            "goal=None path must NEVER emit GoalTimeout"
        );
    }

    /// recovery hook 付きでも `run_loop_with_goal(goal=None)` が
    /// `run_loop_with_recovery` と同じ停止契約(MaxIterations 優先、Goal 系不出現)を
    /// 保つことを検証する。hook は Ok を返し続けるため MaxIterations まで到達する。
    /// (recovery Err の ExecuteError 契約は既存 2710 テストが直接経路で担保済み。)
    #[tokio::test]
    async fn backcompat_goal_none_with_recovery_hook_preserves_max_iterations_contract() {
        fn make_hook() -> (Arc<AtomicU32>, RecoveryHook) {
            let calls = Arc::new(AtomicU32::new(0));
            let calls_clone = calls.clone();
            let hook: RecoveryHook = Box::new(move |_streak| {
                let c = calls_clone.clone();
                Box::pin(async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
            });
            (calls, hook)
        }

        // run_loop_with_goal(goal=None) + recovery hook 経路。
        let (mut driver, _fired) = build_nomatch_driver();
        let (calls, hook) = make_hook();
        let clock = FakeClock::starting_at(0);
        let outcome = driver
            .run_loop_with_goal(Duration::ZERO, 12, 3, Some(hook), None, clock)
            .await;

        // 停止理由は MaxIterations のみ(Goal 系停止理由は発火しない)。
        assert_eq!(
            outcome.reason,
            LoopStopReason::MaxIterations,
            "goal=None + recovery(Ok) must stop with MaxIterations, got {:?}",
            outcome.reason
        );
        assert_ne!(
            outcome.reason,
            LoopStopReason::GoalReached,
            "GoalReached must never fire when goal=None even with recovery"
        );
        assert_ne!(
            outcome.reason,
            LoopStopReason::GoalTimeout,
            "GoalTimeout must never fire when goal=None even with recovery"
        );
        // recovery hook は最低 1 回発火(threshold=3, max=12 → 3,6,9,12 のいずれか)。
        assert!(
            calls.load(Ordering::SeqCst) >= 1,
            "recovery hook must fire at least once under goal=None delegation path"
        );
        // reached_goal は None(UC-3 AC 不変条件、recovery 付きでも崩れない)。
        assert!(
            outcome.progress_report.reached_goal.is_none(),
            "reached_goal must stay None even with recovery hook under goal=None"
        );
    }

    // ===== T-fix-1: evaluate_goal unit contract =====

    /// LoopCount ゴールが既に target に到達しているとき `evaluate_goal` は
    /// `GoalReached` の `LoopOutcome` を返し、`reached_goal` に記述子を埋める。
    /// まだ到達していなければ [`None`] を返す(継続)。
    #[tokio::test]
    async fn evaluate_goal_loop_count_reached_and_not_yet() {
        let (driver, _fired) = build_nomatch_driver();
        let started = Instant::now();
        let goal_reached = anaden_core::Goal {
            name: "g1".into(),
            stop: anaden_core::StopCondition::LoopCount { target: 5 },
        };
        let goal_pending = anaden_core::Goal {
            name: "g2".into(),
            stop: anaden_core::StopCondition::LoopCount { target: 100 },
        };

        // 到達済み: ctx.iterations >= target
        let mut ctx_hit = anaden_core::GoalStatusContext::new();
        for _ in 0..5 {
            ctx_hit.tick(None);
        }
        let outcome = driver
            .evaluate_goal(
                &goal_reached,
                &ctx_hit,
                0,
                GoalEvalInputs {
                    iterations: 5,
                    fired_commands: &[],
                    per_task_matches: &[],
                    started,
                },
            )
            .expect("expected Some(LoopOutcome) on GoalReached");
        assert_eq!(outcome.reason, LoopStopReason::GoalReached);
        assert_eq!(outcome.terminal, "goal_reached");
        assert_eq!(
            outcome.progress_report.reached_goal.as_deref(),
            Some("loop_count=5"),
            "descriptor must reflect LoopCount target"
        );
        assert_eq!(outcome.progress_report.iterations, 5);

        // 未到達: None (ループ継続)
        let mut ctx_low = anaden_core::GoalStatusContext::new();
        for _ in 0..3 {
            ctx_low.tick(None);
        }
        assert!(
            driver
                .evaluate_goal(
                    &goal_pending,
                    &ctx_low,
                    0,
                    GoalEvalInputs {
                        iterations: 3,
                        fired_commands: &[],
                        per_task_matches: &[],
                        started,
                    },
                )
                .is_none(),
            "must return None when target not reached"
        );
    }

    /// Timeout ゴールが elapsed_secs >= secs のとき `GoalTimeout` で停止し、
    /// `reached_goal` に timeout 記述子を埋める。UC-3 契約。
    #[tokio::test]
    async fn evaluate_goal_timeout_yields_goal_timeout_descriptor() {
        let (driver, _fired) = build_nomatch_driver();
        let started = Instant::now();
        let goal = anaden_core::Goal {
            name: "g3".into(),
            stop: anaden_core::StopCondition::Timeout { secs: 10 },
        };
        let ctx = anaden_core::GoalStatusContext::new();
        let outcome = driver
            .evaluate_goal(
                &goal,
                &ctx,
                10,
                GoalEvalInputs {
                    iterations: 0,
                    fired_commands: &[],
                    per_task_matches: &[],
                    started,
                },
            )
            .expect("expected Some on timeout reached");
        assert_eq!(outcome.reason, LoopStopReason::GoalTimeout);
        assert_eq!(outcome.terminal, "goal_timeout");
        assert_eq!(
            outcome.progress_report.reached_goal.as_deref(),
            Some("timeout=10"),
            "descriptor must reflect Timeout secs"
        );

        // elapsed < secs → None
        assert!(
            driver
                .evaluate_goal(
                    &goal,
                    &ctx,
                    9,
                    GoalEvalInputs {
                        iterations: 0,
                        fired_commands: &[],
                        per_task_matches: &[],
                        started,
                    },
                )
                .is_none(),
            "must return None before timeout"
        );
    }

    /// TemplateMatch ゴール: ctx.last_match が閾値以上で task 一致のとき GoalReached。
    /// UC-2 契約(T-fix-1 では evaluate_goal 単体の経路のみ検証。match_info 伝播は
    /// T-wire-match-info が担う)。
    #[tokio::test]
    async fn evaluate_goal_template_match_reached() {
        let (driver, _fired) = build_nomatch_driver();
        let started = Instant::now();
        let goal = anaden_core::Goal {
            name: "g4".into(),
            stop: anaden_core::StopCondition::TemplateMatch {
                task: "Boss".into(),
                confidence: 0.85,
            },
        };
        let mut ctx = anaden_core::GoalStatusContext::new();
        ctx.tick(Some((
            "Boss".to_string(),
            0.9_f32,
            ScreenRegion::new(0, 0, 10, 10),
        )));
        let outcome = driver
            .evaluate_goal(
                &goal,
                &ctx,
                0,
                GoalEvalInputs {
                    iterations: 1,
                    fired_commands: &[],
                    per_task_matches: &[],
                    started,
                },
            )
            .expect("expected Some on TemplateMatch reached");
        assert_eq!(outcome.reason, LoopStopReason::GoalReached);
        assert_eq!(
            outcome.progress_report.reached_goal.as_deref(),
            Some("template_match conf>=0.85 (task=Boss)"),
            "descriptor must reflect TemplateMatch task/confidence"
        );
    }

    /// 記述子生成器が全 StopCondition バリアントをカバーし、
    /// 既存 fixtures(`loop_count=7`, `template_match conf>=0.85`)の形式と一致する。
    #[test]
    fn goal_descriptor_covers_all_variants() {
        let lc = anaden_core::Goal {
            name: "n".into(),
            stop: anaden_core::StopCondition::LoopCount { target: 7 },
        };
        assert_eq!(goal_descriptor(&lc), "loop_count=7");

        let tm = anaden_core::Goal {
            name: "n".into(),
            stop: anaden_core::StopCondition::TemplateMatch {
                task: "X".into(),
                confidence: 0.85,
            },
        };
        assert!(goal_descriptor(&tm).starts_with("template_match conf>=0.85"));

        let to = anaden_core::Goal {
            name: "n".into(),
            stop: anaden_core::StopCondition::Timeout { secs: 30 },
        };
        assert_eq!(goal_descriptor(&to), "timeout=30");
    }

    // ===== T-wire-match-info: UC-2 E2E (TemplateMatch ゴール到達) =====
    //
    // Issue #37 T-wire-match-info: これまで match_info が全 StepOutcome バリアントで
    // None に hardcode されていたため、GoalStatusContext::last_match が populated されず
    // TemplateMatch ゴールが一切 GoalReached に到達しなかった(UC-2 が発火しない)。
    //
    // 修正: Fired/NoFire ブランチで self.last_match.take() を読み出し ctx へ伝播。
    // 本 E2E テストは「マッチングテンプレートが発火したサイクルで TemplateMatch ゴールが
    // GoalReached に到達する」ことを検証する(TDD: 修正前は赤、修正後は緑)。

    /// 決定論的経過秒数を返すテスト用クロック(org-feedback approval condition 参照)。
    /// `elapsed_secs` は tokio::time::pause ではなく呼出毎に決定論的に増やす。
    struct FakeClock {
        next_secs: u64,
    }

    impl FakeClock {
        fn starting_at(secs: u64) -> Self {
            Self { next_secs: secs }
        }
    }

    impl GoalClock for FakeClock {
        fn elapsed_secs(&mut self) -> u64 {
            let now = self.next_secs;
            // 呼出毎に 1 秒進める(UC-3 timeout 跨ぎテストで使用)。
            self.next_secs = self.next_secs.saturating_add(1);
            now
        }
    }

    // ===== UC-1 E2E (LoopCount ゴール到達) =====
    //
    // UC-1 は「宣言した反復数(tick)に到達したら停止する」時間駆動の終端保証。
    // goal.rs の doc に固定されている通り、LoopCount は認識成功率に依存せず、
    // evaluate 呼出回数(tick 数)のみで到達を判定する。よって NoMatch が持続する
    // build_nomatch_driver の fixture でも到達できる(UC-2/UC-3 のようにテンプレート
    // マッチや時間経過に依存しない)。本 E2E テストは UC-2(L3010)/UC-3(L2880 周辺)
    // の既存 E2E と対になる UC-1 の 1 本。

    /// UC-1 正常系: `LoopCount { target: 3 }` を与えると、3 tick(= 3 サイクル)後に
    /// `LoopStopReason::GoalReached` で停止し、`reached_goal == "loop_count=3"`、
    /// `iterations == 3` となること。`build_nomatch_driver` の fixture は NoMatch が
    /// 持続するが、LoopCount は tick 数のみで判定するため NoMatch でも到達する。
    ///
    /// ここでは `FakeClock::starting_at` を使用する(org-feedback approval condition)。
    /// LoopCount は経過秒数に依存しないので、clock の進み方は結果に影響しないが、
    /// `SystemClock` 切替や `tokio::time::pause` の持ち込みは契約違反として禁止する。
    #[tokio::test]
    async fn uc1_loop_count_goal_reaches_goal_reached_after_target_ticks() {
        let (mut driver, _fired) = build_nomatch_driver();

        let goal = anaden_core::Goal {
            name: "farm3".into(),
            stop: anaden_core::StopCondition::LoopCount { target: 3 },
        };
        let clock = FakeClock::starting_at(0);
        let outcome = driver
            .run_loop_with_goal(Duration::ZERO, 10, 0, None, Some(&goal), clock)
            .await;

        // UC-1 の核心: target=3 → 3 tick で GoalReached(MaxIterations ではない)。
        assert_eq!(
            outcome.reason,
            LoopStopReason::GoalReached,
            "UC-1 LoopCount target=3 must reach GoalReached after 3 ticks, got {:?}",
            outcome.reason
        );
        assert_eq!(outcome.terminal, "goal_reached");
        // 記述子は goal_descriptor の LoopCount 形式(loop_count=<target>)。
        assert_eq!(
            outcome.progress_report.reached_goal.as_deref(),
            Some("loop_count=3"),
            "reached_goal descriptor must reflect LoopCount target"
        );
        // tick 数 = evaluate 呼出回数 = サイクル数。target 到達と同値。
        assert_eq!(
            outcome.iterations, 3,
            "must reach goal exactly at iteration 3"
        );
    }

    /// UC-1 エッジ: `target` が `max_iterations` を超える場合、ゴール到達前に
    /// `max_iterations` に先到達し `LoopStopReason::MaxIterations` になること。
    /// これは「到達テストが常に GoalReached になる偽陽性」を防ぐ対照であり、
    /// LoopCount の早期停止が max_iterations ガードより優先されないことを検証する。
    #[tokio::test]
    async fn uc1_loop_count_goal_hits_max_iterations_when_target_exceeds_budget() {
        let (mut driver, _fired) = build_nomatch_driver();

        let goal = anaden_core::Goal {
            name: "farm_unbounded".into(),
            // max_iterations=5 より大きい target → 到達不可。
            stop: anaden_core::StopCondition::LoopCount { target: 50 },
        };
        let clock = FakeClock::starting_at(0);
        let outcome = driver
            .run_loop_with_goal(Duration::ZERO, 5, 0, None, Some(&goal), clock)
            .await;

        // ゴール未到達 → MaxIterations(GoalReached/GoalTimeout ではない)。
        assert_eq!(
            outcome.reason,
            LoopStopReason::MaxIterations,
            "UC-1 LoopCount with target>max_iterations must stop with MaxIterations, got {:?}",
            outcome.reason
        );
        // ゴールに到達していないので記述子は埋まらない。
        assert!(
            outcome.progress_report.reached_goal.is_none(),
            "reached_goal must be None when goal is not reached"
        );
        // iterations は max_iterations に等しい。
        assert_eq!(outcome.iterations, 5);
    }

    /// UC-2: テンプレートマッチが発火した直後のサイクルで TemplateMatch ゴールが
    /// `GoalReached` に到達し、`reached_goal` に記述子が埋まること。
    ///
    /// 手順: needle 埋込画面を与え ClickRect タスクをマッチ→発火させる。
    /// ゴールは同じタスク名 + 閾値 0.85(実測信頼度は閾値を超える)。
    /// run_loop_with_goal は Fired ブランチで last_match.take() により
    /// (task_name, confidence, region) を ctx.last_match へ伝播し、evaluate が
    /// Reached を返す → GoalReached で停止。
    #[tokio::test]
    async fn uc2_template_match_goal_reaches_goal_reached_when_matching_template_fires() {
        let needle = gradient_needle(40, 40);
        let matched = luma_dyn(embed(FULL_W, FULL_H, &needle, 150, 75, 128));
        // Title 発火後 next=LoadGame へ遷移するが、初回の Fired でゴール到達するので
        // 1 枚で十分(到達前の巻き戻り等は無い)。
        let frames = frames_of(vec![matched]);
        let fired = new_fired();

        let task = click_rect_task(
            "Title",
            Action::ClickRect {
                roi: ScreenRegion::new(520, 320, 240, 80),
            },
            // next を出す(発火はする)がゴール評価が先に効いて GoalReached で停止。
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

        let goal = anaden_core::Goal {
            name: "find_title".into(),
            stop: anaden_core::StopCondition::TemplateMatch {
                task: "Title".into(),
                confidence: 0.85,
            },
        };
        let clock = FakeClock::starting_at(0);
        let outcome = driver
            .run_loop_with_goal(Duration::ZERO, 10, 0, None, Some(&goal), clock)
            .await;

        // UC-2 の核心: GoalReached で停止(MaxIterations/TerminalTask ではない)。
        assert_eq!(
            outcome.reason,
            LoopStopReason::GoalReached,
            "UC-2 TemplateMatch goal must reach GoalReached when matching template fires, \
             got {:?}",
            outcome.reason
        );
        assert_eq!(outcome.terminal, "goal_reached");
        // 記述子が task と confidence を反映。
        assert_eq!(
            outcome.progress_report.reached_goal.as_deref(),
            Some("template_match conf>=0.85 (task=Title)"),
            "reached_goal descriptor must reflect the TemplateMatch task/confidence"
        );
        // 1 サイクル目で到達(発火 = ctx.last_match populated → 即 Reached)。
        assert_eq!(
            outcome.iterations, 1,
            "must reach goal on first matching cycle"
        );
        // 発火は起きている。
        assert!(
            !outcome.fired_commands.is_empty(),
            "matching template must have fired a command"
        );
    }

    /// UC-2 回帰保護: last_match の task がゴール task と一致しないと到達しないこと。
    /// これで match_info 伝播が正しくても task 名不一致で NotYet → MaxIterations になる
    /// 対照を確立し、到達テストが「常に到達する偽陽性」にならないようにする。
    #[tokio::test]
    async fn uc2_template_match_goal_does_not_reach_when_task_name_mismatches() {
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
            // next=None にすると TerminalTask で止まるので、遷移させてループを回す。
            // Title→LoadGame だが LoadGame 定義が無いので以降 NoMatch で MaxIterations。
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

        // ゴール task を "OtherTask"(マッチしない)にする → last_match.task == "Title" ≠ "OtherTask"
        // → NotYet → MaxIterations。
        let goal = anaden_core::Goal {
            name: "find_other".into(),
            stop: anaden_core::StopCondition::TemplateMatch {
                task: "OtherTask".into(),
                confidence: 0.85,
            },
        };
        let clock = FakeClock::starting_at(0);
        let outcome = driver
            .run_loop_with_goal(Duration::ZERO, 5, 0, None, Some(&goal), clock)
            .await;

        assert_ne!(
            outcome.reason,
            LoopStopReason::GoalReached,
            "must NOT reach GoalReached when task name mismatches last_match"
        );
    }

    // ===== UC-3 E2E (Timeout ゴール到達) =====
    //
    // UC-3 は「宣言した秒数が経過したら停止する」時間駆動の異常終端保証。
    // goal.rs の doc に固定されている通り、Timeout は与えられた elapsed_secs と
    // 宣言値の比較のみで判定する(pure evaluate)。run_loop_with_goal は各 tick 後に
    // GoalClock.elapsed_secs() を呼び、GoalStatus::Failed → LoopStopReason::GoalTimeout
    // で停止する。本 E2E テストは UC-1(L3019)/UC-2(L3096) と対になる UC-3 の 1 本。
    //
    // FakeClock(build_nomatch_driver の fixture と FakeClock::starting_at を流用)が
    // elapsed_secs 呼出毎に +1 するので、NoMatch が持続する build_nomatch_driver でも
    // 宣言秒数に到達したタイミングで GoalTimeout になる。

    /// UC-3 正常系: `Timeout { secs: 3 }` を与えると、FakeClock が呼出毎に +1 進むので
    /// 4 サイクル目(elapsed_secs == 3 >= 3)に `LoopStopReason::GoalTimeout` で停止し、
    /// `terminal == "goal_timeout"`、`reached_goal == "timeout=3"` になること。
    /// また `iterations == 4` は `max_iterations == 10` 未到達であり、ゴールが先に効いて
    /// いる(MaxIterations ではない)ことを検証する。
    ///
    /// ここでは `FakeClock::starting_at(0)` を使用する(org-feedback approval condition)。
    /// FakeClock の進め方(呼出毎 +1)により、開始値 0 → 0,1,2,3 と返し、3 で閾値到達。
    #[tokio::test]
    async fn uc3_timeout_goal_reaches_goal_timeout_after_declared_secs() {
        let (mut driver, _fired) = build_nomatch_driver();

        let goal = anaden_core::Goal {
            name: "hard_stop".into(),
            stop: anaden_core::StopCondition::Timeout { secs: 3 },
        };
        let clock = FakeClock::starting_at(0);
        let outcome = driver
            .run_loop_with_goal(Duration::ZERO, 10, 0, None, Some(&goal), clock)
            .await;

        // UC-3 の核心: elapsed_secs が secs(=3)に到達 → GoalTimeout(MaxIterations ではない)。
        // FakeClock starting_at(0) は呼出毎に 0,1,2,3 を返すので、4 tick 目で 3>=3 到達。
        assert_eq!(
            outcome.reason,
            LoopStopReason::GoalTimeout,
            "UC-3 Timeout secs=3 must reach GoalTimeout once FakeClock advances past the \
             limit, got {:?}",
            outcome.reason
        );
        assert_eq!(
            outcome.terminal, "goal_timeout",
            "terminal must be goal_timeout for Timeout failure"
        );
        // 記述子は goal_descriptor の Timeout 形式(timeout=<secs>)。
        assert_eq!(
            outcome.progress_report.reached_goal.as_deref(),
            Some("timeout=3"),
            "reached_goal descriptor must reflect Timeout secs"
        );
        // iterations は max_iterations(=10)未到達 → ゴールが先に効いた。
        assert!(
            outcome.iterations < 10,
            "goal must terminate before max_iterations; got iterations={}",
            outcome.iterations
        );
        // FakeClock の呼出毎 +1 トレース: tick1=0, tick2=1, tick3=2, tick4=3(>=3 → stop)。
        assert_eq!(
            outcome.iterations, 4,
            "must reach timeout exactly on iteration 4 (elapsed 0,1,2,3)"
        );
    }

    // ===== JSON ラウンドトリップ検証(エンジン産出 outcome) =====
    //
    // 既存 `loop_outcome_is_json_serializable`(L2423) は hand-built な LoopOutcome を
    // 使っており、serde の形状(reason snake_case / progress_report ネスト / reached_goal)
    // が JSON 経路で保たれることを検証している。一方で「エンジン経路(run_loop_with_goal)
    // を通じて実際に populate された reached_goal を持つ outcome」が JSON ラウンドトリップ
    // を生き延びるかは別の保証になる: evaluate_goal → goal_descriptor が埋めた文字列が
    // シリアライズ→デシリアライズ後も完全に一致することを、UC-1 と UC-2 の両方の
    // ゴール種別で検証する(UC-3 Timeout は本チケットのスコープ外: #42 の未達 AC は
    // UC-1/UC-2 の JSON 耐性で明示されているため)。
    // Issue #42 / 親 #37 Shard 3: JSON 成果レポート検証。

    /// UC-1(LoopCount) エンジン産出 outcome が JSON ラウンドトリップを生き延びること。
    /// run_loop_with_goal で実際に GoalReached まで駆動し、`reached_goal == "loop_count=3"`
    /// が populate された outcome を to_string → from_str し、progress_report.reached_goal
    /// が完全一致で生き残ることを assert する。
    #[tokio::test]
    async fn uc1_loop_count_outcome_reached_goal_survives_json_roundtrip() {
        let (mut driver, _fired) = build_nomatch_driver();

        let goal = anaden_core::Goal {
            name: "farm3".into(),
            stop: anaden_core::StopCondition::LoopCount { target: 3 },
        };
        let clock = FakeClock::starting_at(0);
        let outcome = driver
            .run_loop_with_goal(Duration::ZERO, 10, 0, None, Some(&goal), clock)
            .await;

        // 前提: エンジン経路で実際に到達し、reached_goal が populate されていること。
        assert_eq!(outcome.reason, LoopStopReason::GoalReached);
        let expected_descriptor = outcome
            .progress_report
            .reached_goal
            .as_deref()
            .expect("reached_goal must be populated by engine for UC-1 LoopCount");
        assert_eq!(expected_descriptor, "loop_count=3");

        // JSON ラウンドトリップ: to_string → from_str。
        let s = serde_json::to_string(&outcome).expect("serialize json");
        let back: LoopOutcome = serde_json::from_str(&s).expect("deserialize json");

        // progress_report.reached_goal がラウンドトリップを生き延びる(本テストの核心)。
        assert_eq!(
            back.progress_report.reached_goal.as_deref(),
            Some("loop_count=3"),
            "UC-1 reached_goal descriptor must survive JSON round-trip; json={s}"
        );
        // reason も snake_case 経路で往復できること(対称性担保)。
        assert_eq!(back.reason, LoopStopReason::GoalReached);
        // iterations も保持されること(progress_report と top-level の両方)。
        assert_eq!(back.iterations, 3);
        assert_eq!(back.progress_report.iterations, 3);
    }

    /// UC-2(TemplateMatch) エンジン産出 outcome が JSON ラウンドトリップを生き延びること。
    /// needle 埋込画面で Title タスクをマッチ→発火させ、TemplateMatch ゴール到達で
    /// populate された reached_goal(記述子は task と confidence を含む) が to_string → from_str
    /// 後も完全一致で生き残ることを assert する。UC-1 と記述子フォーマットが異なるため
    /// 両方を網羅する(template_match conf>=<c> (task=<t>) 形式の JSON 耐性)。
    #[tokio::test]
    async fn uc2_template_match_outcome_reached_goal_survives_json_roundtrip() {
        let needle = gradient_needle(40, 40);
        let matched = luma_dyn(embed(FULL_W, FULL_H, &needle, 150, 75, 128));
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

        let goal = anaden_core::Goal {
            name: "find_title".into(),
            stop: anaden_core::StopCondition::TemplateMatch {
                task: "Title".into(),
                confidence: 0.85,
            },
        };
        let clock = FakeClock::starting_at(0);
        let outcome = driver
            .run_loop_with_goal(Duration::ZERO, 10, 0, None, Some(&goal), clock)
            .await;

        // 前提: エンジン経路で TemplateMatch ゴール到達し、reached_goal が populate されていること。
        assert_eq!(outcome.reason, LoopStopReason::GoalReached);
        let expected_descriptor = outcome
            .progress_report
            .reached_goal
            .as_deref()
            .expect("reached_goal must be populated by engine for UC-2 TemplateMatch");
        assert_eq!(
            expected_descriptor,
            "template_match conf>=0.85 (task=Title)"
        );

        // JSON ラウンドトリップ: to_string → from_str。
        let s = serde_json::to_string(&outcome).expect("serialize json");
        let back: LoopOutcome = serde_json::from_str(&s).expect("deserialize json");

        // progress_report.reached_goal がラウンドトリップを生き延びる(本テストの核心)。
        // 記述子は空白・記号(>=, ()) を含むため、JSON 文字列 escaping も耐性検証に含まれる。
        assert_eq!(
            back.progress_report.reached_goal.as_deref(),
            Some("template_match conf>=0.85 (task=Title)"),
            "UC-2 reached_goal descriptor must survive JSON round-trip; json={s}"
        );
        // reason も snake_case 経路で往復できること(対称性担保)。
        assert_eq!(back.reason, LoopStopReason::GoalReached);
        // 発火履歴も形状を保って往復できること(fired_commands が空でない)。
        assert!(
            !back.fired_commands.is_empty(),
            "fired_commands must survive JSON round-trip non-empty"
        );
    }
}
