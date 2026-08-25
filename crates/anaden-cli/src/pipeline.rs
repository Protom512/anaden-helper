//! pipeline 開始/停止制御インターフェース(Issue #83 シャード2)。
//!
//! anaden-cli `main.rs` の pipeline 構築ロジック(PipelineDriver::new + CancellationToken
//! 配線)を GUI(anaden-studio)から再利用可能なライブラリ層へ切り出す。
//!
//! # 方式: in-process (シャード1選定)
//!
//! GUI プロセスが本モジュールを直接呼び、pipeline を同一プロセス内の tokio task として
//! 起動する。これにより:
//!
//! - **孤立防止**: pipeline は GUI と運命共同体(GUI プロセスが落ちれば道連れ)。
//!   子プロセス + Job Object 方式は不要。
//! - **二重起動防止**: [`PipelineController`] が単一の実行スロットを排他管理し、
//!   実行中の `try_start` は [`StartError::AlreadyRunning`] を返す(GUI は開始ボタンを無効化)。
//!
//! # 依存方向
//!
//! `anaden-studio` → `anaden_cli_contract::pipeline` → `anaden-engine` / `anaden-vision`
//! / `anaden-core`。一方向のみ(architecture-coupling-balance)。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

/// 実行ターゲット(=` `--target` の解決結果)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunTarget {
    /// ADB 実機(android)。serial 必須。
    Android,
    /// PC 版(Win32)。serial 不要。
    Windows,
}

/// 画面キャプチャ方式(=` `--capture` の解決結果)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureMode {
    /// adb exec-out screencap(既定)。
    Screencap,
    /// 常駐 scrcpy H.264 受信(`capture-scrcpy` feature 必須)。
    Scrcpy,
}

/// 入力方式(=` `--input` の解決結果)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    /// adb input tap(既定)。
    Adb,
    /// scrcpy control ソケット経由タッチ注入(`capture-scrcpy` feature 必須)。
    Scrcpy,
}

/// pipeline 実行オプション。`anaden run` の CLI フラグ群と1:1 対応する
/// GUI 由来の構造化版(GUI は clap を経由せずこれを直接組み立てる)。
#[derive(Debug, Clone)]
pub struct RunOptions {
    /// 実行ターゲット(android / windows)。
    pub target: RunTarget,
    /// ADB シリアル。`RunTarget::Android` 時必須、`RunTarget::Windows` 時不要。
    pub serial: Option<String>,
    /// `*.toml` を格納したパイプラインディレクトリ。
    pub pipeline_dir: PathBuf,
    /// 開始タスク名(PipelineState の初期 current)。
    pub start_task: String,
    /// algorithm 上書き(`sse` / `ccoeff`)。None なら TOML 尊重。
    pub algorithm: Option<String>,
    /// ループ間隔(秒)。1 以上。
    pub interval_secs: u64,
    /// 最大サイクル数。1 以上。
    pub max_iters: u64,
    /// device_width 手動指定(rescale 用)。None なら初回 capture で実測。
    pub width: Option<u32>,
    /// 接続時にゲームが未起動なら自動起動して前景化を待つ。
    pub ensure_open: bool,
    /// ゲーム前景化待ちタイムアウト(秒)。
    pub ensure_open_wait_secs: u64,
    /// NoMatch 連続時のゲーム再起動リカバリを有効化。
    pub recover_launch: bool,
    /// NoMatch 連続で再起動する閾値(回数)。
    pub recover_nomatch_threshold: u32,
    /// 画面キャプチャ方式。
    pub capture: CaptureMode,
    /// 入力方式。
    pub input: InputMode,
    /// scrcpy サーバ jar のローカルパス(scrcpy 系モード時)。
    pub scrcpy_jar: String,
    /// 発火後検証(誠実検証)を有効化。
    pub verify_after_fire: bool,
    /// 宣言的ゴール。None なら非ゴールモード。
    pub goal: Option<anaden_core::Goal>,
}

/// `--capture` 文字列を [`CaptureMode`] へ解決する純粋関数。
pub fn resolve_capture_mode(value: &str) -> Result<CaptureMode, String> {
    match value {
        "screencap" => Ok(CaptureMode::Screencap),
        "scrcpy" => Ok(CaptureMode::Scrcpy),
        other => Err(format!(
            "capture モードは `screencap` または `scrcpy` です(指定値: {other})"
        )),
    }
}

/// `--input` 文字列を [`InputMode`] へ解決する純粋関数。
pub fn resolve_input_mode(value: &str) -> Result<InputMode, String> {
    match value {
        "adb" => Ok(InputMode::Adb),
        "scrcpy" => Ok(InputMode::Scrcpy),
        other => Err(format!(
            "input モードは `adb` または `scrcpy` です(指定値: {other})"
        )),
    }
}

/// `--target` 文字列を [`RunTarget`] へ解決する純粋関数
/// (既存 `ensure_open` 用 [`crate::resolve_target`] の run 系対応版)。
pub fn resolve_run_target(value: &str) -> Result<RunTarget, String> {
    match value {
        "android" => Ok(RunTarget::Android),
        "windows" => Ok(RunTarget::Windows),
        other => Err(format!(
            "target は `android` または `windows` です(指定値: {other})"
        )),
    }
}

/// `--algorithm` 文字列を vision の Algorithm へ解決する純粋関数
/// (main.rs の `resolve_algorithm` の切り出し版)。
pub fn resolve_algorithm(value: &str) -> Result<anaden_vision::Algorithm, String> {
    match value {
        "sse" => Ok(anaden_vision::Algorithm::Sse),
        "ccoeff" => Ok(anaden_vision::Algorithm::Ccoeff),
        other => Err(format!(
            "algorithm は `sse` または `ccoeff` です(指定値: {other})"
        )),
    }
}

impl RunOptions {
    /// 開始前の不変量検証(純粋・デバイス I/O なし・panic なし)。
    ///
    /// 検証項目:
    /// - `start_task` が空でない
    /// - `interval_secs >= 1` / `max_iters >= 1`
    /// - `pipeline_dir` が存在するディレクトリで、`start_task` がそこに定義されている
    /// - `algorithm` 指定時 `sse`/`ccoeff` に解決できる
    /// - android 時 `serial` 必須 / windows 時 `serial` 指定はエラー(打ち間違い検出)
    /// - scrcpy 系モードは android + serial のみ(windows バックエンドに scrcpy は無い)
    /// - `goal` 指定時 `Goal::validate` が通る
    pub fn validate(&self) -> Result<(), anyhow::Error> {
        if self.start_task.trim().is_empty() {
            anyhow::bail!("start_task が空です(開始タスク名を指定してください)");
        }
        if self.interval_secs == 0 {
            anyhow::bail!("interval_secs は 1 以上を指定してください(指定値: 0)");
        }
        if self.max_iters == 0 {
            anyhow::bail!("max_iters は 1 以上を指定してください(指定値: 0)");
        }
        if !self.pipeline_dir.is_dir() {
            anyhow::bail!(
                "pipeline_dir がディレクトリではありません: {}",
                self.pipeline_dir.display()
            );
        }
        if let Some(a) = &self.algorithm {
            resolve_algorithm(a).map_err(|e| anyhow::anyhow!(e))?;
        }
        match self.target {
            RunTarget::Android => {
                if self
                    .serial
                    .as_deref()
                    .map(str::trim)
                    .unwrap_or("")
                    .is_empty()
                {
                    anyhow::bail!("android ターゲットでは serial が必須です");
                }
            }
            RunTarget::Windows => {
                if self.serial.is_some() {
                    anyhow::bail!(
                        "windows ターゲットでは serial を指定できません(ADB を使用しません)"
                    );
                }
            }
        }
        let scrcpy_mode = self.capture == CaptureMode::Scrcpy || self.input == InputMode::Scrcpy;
        if scrcpy_mode {
            if self.target != RunTarget::Android {
                anyhow::bail!("scrcpy capture/input は android ターゲットのみで使用できます");
            }
            if self.serial.is_none() {
                anyhow::bail!("scrcpy モードでは serial が必須です");
            }
        }
        crate::validate_goal(&self.goal)?;
        if !self.start_task_exists_in_pipeline()? {
            anyhow::bail!(
                "start_task `{}` が pipeline ディレクトリ {} に定義されていません",
                self.start_task,
                self.pipeline_dir.display()
            );
        }
        Ok(())
    }

    /// `start_task` が pipeline ディレクトリ内のいずれかの TaskDef.name と一致するか。
    fn start_task_exists_in_pipeline(&self) -> Result<bool, anyhow::Error> {
        let tasks = anaden_vision::load_pipeline(&self.pipeline_dir)
            .map_err(|e| anyhow::anyhow!("パイプライン読込失敗: {e}"))?;
        Ok(tasks.iter().any(|t| t.name == self.start_task))
    }
}

/// controller が管理する実行スロットの状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunState {
    /// 一度も実行していない(または前回実行の成果をまだ取得していない初期状態)。
    Idle,
    /// pipeline 実行中。
    Running,
    /// pipeline 実行完了(正常終端・停止・エラーを含む)。
    Finished,
}

/// [`PipelineController::try_start`] のエラー。
#[derive(Debug)]
pub enum StartError {
    /// 実行中に再度開始しようとした(二重起動防止)。
    AlreadyRunning,
    /// `RunOptions::validate` 失敗。メッセージを保持。
    InvalidOptions(anyhow::Error),
}

impl StartError {
    /// 二重起動エラーかどうか(GUI の開始ボタン無効化判定用)。
    pub fn is_already_running(&self) -> bool {
        matches!(self, StartError::AlreadyRunning)
    }
}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StartError::AlreadyRunning => {
                write!(f, "pipeline は既に実行中です(二重起動は防止されています)")
            }
            StartError::InvalidOptions(e) => write!(f, "実行オプションが不正です: {e}"),
        }
    }
}

impl std::error::Error for StartError {}

/// 注入される pipeline 実行関数の型。`RunOptions` とキャンセルトークンから
/// `LoopOutcome` を産出する future を返す(実機経路は main.rs 側で後続シャードが
/// 配線、テストは mock を注入)。
pub type RunFn = fn(
    RunOptions,
    CancellationToken,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<anaden_engine::LoopOutcome, anyhow::Error>> + Send>,
>;

/// 実行スロット(1 本のみ)。Mutex で排他し、二重起動を構造的に防止する。
#[derive(Default)]
struct Slot {
    cancel: Option<CancellationToken>,
    running: bool,
    finished: bool,
}

/// GUI からの pipeline 開始/停止制御口(in-process 方式)。
///
/// - [`PipelineController::try_start`]: 検証済みオプションで pipeline task を起動。
///   実行中は `Err(StartError::AlreadyRunning)`。
/// - [`PipelineController::cancel`]: 実行中の pipeline へキャンセルを発火
///   (driver は `LoopStopReason::Interrupted` で安全停止)。
/// - [`PipelineController::state`]: 現在状態(GUI のボタン有効/無効化用)。
///
/// 孤立防止: pipeline は本 controller を持つプロセス内 tokio task のため、GUI プロセスが
/// 落ちれば必ず道連れになる。子プロセスは spawn しない。
#[derive(Default)]
pub struct PipelineController {
    slot: Arc<Mutex<Slot>>,
}

impl PipelineController {
    /// 新規 controller(初期状態 Idle)。
    pub fn new() -> Self {
        Self::default()
    }

    /// 現在の実行状態。
    pub async fn state_async(&self) -> RunState {
        let slot = self.slot.lock().await;
        slot_state(&slot)
    }

    /// 現在の実行状態(同期版。ロック競合時はブロックするため UI スレッドの
    /// 毎フレーム呼出には `state_async` を推奨)。
    pub fn state(&self) -> RunState {
        // controller が drop されても lock は失敗しない(tokio Mutex は同期 lock を提供)。
        match self.slot.try_lock() {
            Ok(slot) => slot_state(&slot),
            Err(_) => RunState::Running, // ロック保持中 = 実行スロット操作中
        }
    }

    /// pipeline を開始する(二重起動防止付き)。
    ///
    /// 1. [`RunOptions::validate`] で不変量検証(invalid なら `InvalidOptions`)。
    /// 2. 実行スロットが空いていることを確認(実行中なら `AlreadyRunning`)。
    /// 3. キャンセルトークンを作成し、`run` を tokio task へ spawn。
    ///
    /// 戻り値は spawn された task の JoinHandle。完了時に内部スロットを Finished へ
    /// 遷移させるラップを被せているため、呼出側は単に `.await` すればよい。
    #[allow(clippy::result_large_err)]
    pub fn try_start(
        &self,
        opts: RunOptions,
        run: RunFn,
    ) -> Result<
        tokio::task::JoinHandle<Result<anaden_engine::LoopOutcome, anyhow::Error>>,
        StartError,
    > {
        if let Err(e) = opts.validate() {
            return Err(StartError::InvalidOptions(e));
        }
        // 排他チェック + スロット確保(同期 try_lock: 非同期コンテキスト外でも呼べる)。
        {
            let mut slot = self
                .slot
                .try_lock()
                .map_err(|_| StartError::AlreadyRunning)?;
            if slot.running {
                return Err(StartError::AlreadyRunning);
            }
            let cancel = CancellationToken::new();
            slot.cancel = Some(cancel.clone());
            slot.running = true;
            slot.finished = false;
        }
        let slot_arc = Arc::clone(&self.slot);
        let cancel = {
            match self.slot.try_lock() {
                Ok(s) => s.cancel.clone(),
                Err(_) => None,
            }
        };
        let cancel = match cancel {
            Some(c) => c,
            None => {
                // 到達不能(直上で確保済み)。安全側として run を実行しない。
                return Err(StartError::AlreadyRunning);
            }
        };
        let jh = tokio::spawn(async move {
            let result = run(opts, cancel).await;
            if let Ok(mut slot) = slot_arc.try_lock() {
                slot.running = false;
                slot.finished = true;
                slot.cancel = None;
            }
            result
        });
        Ok(jh)
    }

    /// 実行中の pipeline へキャンセルを発火する(安全停止要求)。
    ///
    /// 実行中でなければ何もしない(べき等)。
    pub async fn cancel(&self) {
        let cancel = {
            let slot = self.slot.lock().await;
            slot.cancel.clone()
        };
        if let Some(c) = cancel {
            c.cancel();
        }
    }
}

fn slot_state(slot: &Slot) -> RunState {
    if slot.running {
        RunState::Running
    } else if slot.finished {
        RunState::Finished
    } else {
        RunState::Idle
    }
}

/// `Path` が pipeline ディレクトリ(1 つ以上の `*.toml` を含む)かどうかの純粋判定。
/// GUI のディレクトリ選択バリデーション用。
pub fn is_pipeline_dir(dir: &Path) -> bool {
    dir.is_dir()
        && std::fs::read_dir(dir)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .any(|e| e.path().extension().is_some_and(|x| x == "toml"))
            })
            .unwrap_or(false)
}
