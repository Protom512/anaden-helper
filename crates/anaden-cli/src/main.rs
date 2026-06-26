//! Another Eden 自動操作ツールの CLI エントリポイント。
//!
//! 4つのサブコマンドを持つ:
//! - `run`: 宣言的パイプラインを ADB 実機でライブ実行する(PipelineDriver 駆動)。
//! - `legacy`: 旧来の Orchestrator(命令型 Strategy ループ) を動かす後方互換エントリ。
//! - `ensure-open`: 起動状態を確認し未起動なら起動する独立 CI gate(Issue #21)。
//! - `launch`: 無条件起動(AlreadyOpen チェックなし、リカバリ用途)。

use std::path::PathBuf;
use std::time::Duration;

use anaden_cli_contract::{ensure_outcome_label, standalone_exit_code};
use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing::{info, warn};

use anaden_engine::{AutomationConfig, Orchestrator};

#[derive(Parser, Debug)]
#[command(name = "anaden", about = "Another Eden automation helper")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// 宣言的パイプラインを ADB 実機でライブ実行する
    Run {
        /// 実行ターゲット: `android`(ADB、既定) または `windows`(PC版 Win32)。
        /// `windows` 指定時は capture/input を Win32 バックエンドへ自動切替え(serial 不要)。
        #[arg(long, default_value = "android")]
        target: String,
        /// `*.toml` を格納したパイプラインディレクトリ
        pipeline_dir: PathBuf,
        /// 開始タスク名(PipelineState の初期 current)
        start_task: String,
        /// ADB デバイスシリアル(例: localhost:5555, R3CN...)。省略可(位置引数 `[SERIAL]`)。
        /// `--target windows` 時は未指定(None)可(PC版は ADB 不要)。`--target android` 時は必須。
        serial: Option<String>,
        /// algorithm 上書き。未指定時は TOML の algorithm を尊重
        #[arg(long)]
        algorithm: Option<String>,
        /// ループ間隔(秒)
        #[arg(long, default_value_t = 1)]
        interval: u64,
        /// 最大サイクル数
        #[arg(long, default_value_t = 100)]
        max_iters: u64,
        /// デバイス実横解像度(rescale 用)。未指定時は初回 capture の width で実測
        #[arg(long)]
        width: Option<u32>,
        /// 接続時にゲームが未起動/非前景なら自動起動して前景化を待つ(デフォルト true)。
        /// 無効化する場合は `--ensure-open false` を指定。
        #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
        ensure_open: bool,
        /// ゲーム前景化待ちのタイムアウト(秒)。
        #[arg(long, default_value_t = 30)]
        ensure_open_wait_secs: u64,
        /// NoMatch 連続時のゲーム再起動リカバリ(デフォルト true)。
        /// 無効化する場合は `--recover-launch false` を指定。
        #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
        recover_launch: bool,
        /// NoMatch が連続してこの回数に達したらゲームを再起動する。
        #[arg(long, default_value_t = 5)]
        recover_nomatch_threshold: u32,
        /// 画面キャプチャ方式: `screencap`(adb exec-out、既定) または `scrcpy`(常駐 H.264 受信)。
        /// `scrcpy` は `capture-scrcpy` feature 有効時のみ使用可。
        #[arg(long, default_value = "screencap")]
        capture: String,
        /// scrcpy サーバ jar のローカルパス(`--capture scrcpy` 時)。
        #[arg(
            long,
            default_value = r"C:\Users\black\scoop\apps\scrcpy\current\scrcpy-server"
        )]
        scrcpy_jar: String,
        /// 入力方式: `adb`(adb input tap、既定) または `scrcpy`(scrcpy control ソケット経由
        /// TYPE_INJECT_TOUCH_EVENT)。`scrcpy` はゲーム(Another Eden)が adb input を無視する
        /// 問題を回避する。`--input scrcpy` 指定時は capture も scrcpy セッション(video+control
        /// 2ソケット)へ一本化される(`--capture` は無視される)。`capture-scrcpy` feature 必須。
        #[arg(long, default_value = "adb")]
        input: String,
        /// 発火後検証(誠実検証)を有効化する(デフォルト true)。
        ///
        /// 有効時、発火成功後にもう1回 capture して同タスクのテンプレがまだマッチするか検証し、
        /// 残存(アクション無効)なら FiredUnverified(実質 NoMatch 相当)を返す。これにより
        /// 「テンプレがマッチして発火した→成功」という偽成功(close_btn 誤キャプチャ等)を弾く。
        /// TASKS.md 誠実検証基準: E2E 効果は単発 MD5 ではなく画面内容のシーン変化(発火前後の
        /// テンプレ消失/領域マッチ変化)で判定する。明示的に無効化する場合のみ `false` を指定。
        #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
        verify_after_fire: bool,
    },
    /// 旧来の Orchestrator(命令型 Strategy ループ) を実行する
    Legacy {
        /// ADB デバイスのシリアル番号または接続先
        #[arg(short, long, default_value = "localhost:5555")]
        device: String,
        /// テンプレート画像のディレクトリパス
        #[arg(short, long, default_value = "./templates/scenes")]
        templates: PathBuf,
        /// メインループの間隔(ミリ秒)
        #[arg(short, long, default_value_t = 500)]
        interval: u64,
        /// テンプレートマッチの信頼度閾値 (0.0〜1.0)
        #[arg(short, long, default_value_t = 0.85)]
        threshold: f32,
        /// 最大実行時間(秒)。0 で無制限。
        #[arg(long, default_value_t = 0)]
        timeout: u64,
        /// 設定ファイルのパス (TOML)
        #[arg(short, long)]
        config: Option<PathBuf>,
    },
    /// ゲームの起動状態を確認し、未起動なら起動して生存/前景化を確認する(Issue #21)。
    ///
    /// パイプライン実行なしで「起動確認＋起動」だけを行う独立 CI gate サブコマンド。
    ///
    /// **終了コード契約(run とは異なる・純加算)**:
    /// - AlreadyOpen / Launched => exit 0(CI gate success)
    /// - Timeout                => exit 2(CI gate **失敗**。ソフト失敗)
    /// - ハードエラー(AdbError/spawn/OpenProcess 失敗) => exit 1
    ///
    /// 注意: `anaden run` は Timeout を soft warn として **継続** するが、本サブコマンドは
    /// 独立 gate として Timeout を非ゼロ終了する。CI スクリプトはこの違いを前提に分岐すること。
    EnsureOpen {
        /// 実行ターゲット: `android`(ADB) または `windows`(PC版 Win32)。
        /// `windows` 指定時は `Win32Launch`、`android` 指定時は `AppController` を使用。
        #[arg(long, default_value = "android")]
        target: String,
        /// ADB デバイスシリアル。`--target android` 時は必須、`--target windows` 時は不要。
        serial: Option<String>,
        /// 起動/前景化待ちのタイムアウト(秒)。Timeout 到達で exit 2。
        /// `run` の `--ensure-open-wait-secs`(既定 30) と同等。
        #[arg(long, default_value_t = 30)]
        wait_secs: u64,
    },
    /// ゲームを無条件で起動する(AlreadyOpen チェックをスキップ、Issue #21)。
    ///
    /// `ensure-open` との違い: 起動済みかに関わらず常に launch を1回実行し、生存/前景化を
    /// 確認する。リカバリ用途(プロセス異常時の強制再起動)向け。終了コード契約は
    /// `ensure-open` に同じ(Launched=0 / Timeout=2 / ハードエラー=1)。
    Launch {
        /// 実行ターゲット: `android`(ADB) または `windows`(PC版 Win32)。
        #[arg(long, default_value = "android")]
        target: String,
        /// ADB デバイスシリアル。`--target android` 時は必須、`--target windows` 時は不要。
        serial: Option<String>,
        /// 起動/前景化待ちのタイムアウト(秒)。Timeout 到達で exit 2。
        /// `run` の `--ensure-open-wait-secs`(既定 30) と同等。
        #[arg(long, default_value_t = 30)]
        wait_secs: u64,
    },
}

/// ゲーム起動保証の共有実行部(`run` パスと standalone サブコマンドの唯一の真実の源)。
///
/// `target` に応じてバックエンドを解決し、対応する ensure 呼出(`AppController::ensure_app_open`
/// / `Win32Launch::ensure_open`)を行って生の [`anaden_device::EnsureOutcome`] を返す。
/// 戻り値のラベル化/ログ/終了コードは呼出側の責務:
/// - `run` パスは soft warn でパイプラインを継続(Timeout でも止まらない)。
/// - standalone(`ensure-open`/`launch`)は [`ensure_open_exit_code`] で非ゼロ終了。
///
/// これを抽出することで、`run_pipeline_live` の android/windows 両経路と standalone 経路が
/// 同一の起動保証本体を共有し、ドリフトを防ぐ(architecture-coupling-balance: high-cohesion,
/// single source of truth for ensure behavior)。`run_pipeline_live` 本体のそれ以外の
/// リファクタリングは行わない(最小変更)。
///
/// - `target = "android"`: `serial` 必須。`AppController::ensure_app_open` を呼ぶ。
/// - `target = "windows"`: `Win32Launch::ensure_open` を呼ぶ(`#[cfg(windows)]`)。
///   非 Windows ビルドでは対となるフォールバック関数が bail する(Linux CI 回避)。
/// - それ以外の `target`: anyhow エラー。
#[cfg(windows)]
async fn ensure_open_outcome(
    target: &str,
    serial: Option<&str>,
    wait: Duration,
) -> Result<anaden_device::EnsureOutcome> {
    match target {
        "android" => {
            let serial = serial.ok_or_else(|| {
                anyhow::anyhow!("--target android 時は ADB シリアル(serial)が必須です")
            })?;
            let controller =
                anaden_device::AppController::new(anaden_device::AdbClient::new(serial));
            controller
                .ensure_app_open(wait)
                .await
                .map_err(|e| anyhow::anyhow!("ゲーム起動保証に失敗({serial}): {e}"))
        }
        "windows" => {
            let launcher = anaden_device::Win32Launch::default_paths();
            launcher
                .ensure_open(wait)
                .await
                .map_err(|e| anyhow::anyhow!("ゲーム起動保証(Win32)に失敗: {e}"))
        }
        other => anyhow::bail!("--target は `android` または `windows` です(指定値: {other})"),
    }
}

/// `ensure_open_outcome` の非 Windows ビルド向けフォールバック(コンパイルエラー回避)。
///
/// `windows` ターゲットは PC版 Win32 バックエンド(`wfsdrv` 依存)を要するため、非 Windows
/// ビルドでは `Win32Launch` が存在しない。`windows` ターゲットのみ bail する。
///
/// 注意: android(ADB) はプラットフォーム非依存(`AdbClient` は cfg-gate されない)なので、
/// 非 Windows ビルドでも android パスは機能する。bail するのは `windows` ターゲットのみ
/// (Win32 バックエンド依存)。これにより Linux CI runner 上でも `--target android` の
/// コンパイル/呼出が壊れない。
#[cfg(not(windows))]
async fn ensure_open_outcome(
    target: &str,
    serial: Option<&str>,
    wait: Duration,
) -> Result<anaden_device::EnsureOutcome> {
    match target {
        "android" => {
            let serial = serial.ok_or_else(|| {
                anyhow::anyhow!("--target android 時は ADB シリアル(serial)が必須です")
            })?;
            let controller =
                anaden_device::AppController::new(anaden_device::AdbClient::new(serial));
            controller
                .ensure_app_open(wait)
                .await
                .map_err(|e| anyhow::anyhow!("ゲーム起動保証に失敗({serial}): {e}"))
        }
        "windows" => anyhow::bail!(
            "`--target windows` は Windows ビルドでのみ利用可能です。このバイナリは Windows 向けではないため PC版バックエンドを使用できません"
        ),
        other => anyhow::bail!("--target は `android` または `windows` です(指定値: {other})"),
    }
}

/// `--algorithm` 文字列を Algorithm へ解決する。
/// `sse` / `ccoeff` 以外は即座に anyhow エラーを返す(panic しない)。
fn resolve_algorithm(value: &str) -> Result<anaden_vision::Algorithm> {
    match value {
        "sse" => Ok(anaden_vision::Algorithm::Sse),
        "ccoeff" => Ok(anaden_vision::Algorithm::Ccoeff),
        other => anyhow::bail!("--algorithm は `sse` または `ccoeff` です(指定値: {other})"),
    }
}

/// `run` サブコマンド: 宣言的パイプラインを ADB 実機でライブ実行する。
///
/// フロー:
///   (1) パイプライン読込 + algorithm 上書き
///   (2) デバイス接続確認(check_connection) + device_width 取得
///   (3) PipelineDriver 構築 → run_loop → LoopOutcome を人間可読出力
///
/// いかなる異常系でも panic せず anyhow エラーを返してプロセス非ゼロ終了する。
#[allow(clippy::too_many_arguments)]
async fn run_pipeline_live(
    serial: Option<&str>,
    pipeline_dir: &PathBuf,
    start_task: &str,
    algorithm: Option<&str>,
    interval: u64,
    max_iters: u64,
    width: Option<u32>,
    ensure_open: bool,
    ensure_open_wait_secs: u64,
    recover_launch: bool,
    recover_nomatch_threshold: u32,
    capture_mode: &str,
    scrcpy_jar: &str,
    input_mode: &str,
    target: &str,
    verify_after_fire: bool,
) -> Result<()> {
    // ---- (0) 実行ターゲット解決 ----
    // `--target windows` なら ADB 経由を一切使わず Win32 バックエンドへ切替え。
    // `--target android` なら従来通り(serial 必須)。
    match target {
        "windows" => {
            return run_with_windows(
                start_task,
                pipeline_dir,
                algorithm,
                interval,
                max_iters,
                width,
                ensure_open,
                ensure_open_wait_secs,
                recover_launch,
                recover_nomatch_threshold,
                verify_after_fire,
            )
            .await;
        }
        "android" => {}
        other => anyhow::bail!("--target は `android` または `windows` です(指定値: {other})"),
    }

    // android パスは serial 必須。
    let serial = serial
        .ok_or_else(|| anyhow::anyhow!("--target android 時は ADB シリアル(serial)が必須です"))?;

    // ---- (1) パイプライン読込 + algorithm 上書き ----
    let mut tasks = anaden_vision::load_pipeline(pipeline_dir)
        .map_err(|e| anyhow::anyhow!("パイプライン読込失敗 {pipeline_dir:?}: {e}"))?;
    if tasks.is_empty() {
        anyhow::bail!("パイプラインが空です: {pipeline_dir:?}");
    }

    let override_algo = match algorithm {
        Some(a) => Some(resolve_algorithm(a)?),
        None => None,
    };
    if let Some(algo) = override_algo {
        // start_task の TaskDef.algorithm を差し替え
        for t in tasks.iter_mut() {
            if t.name == start_task {
                t.algorithm = algo;
            }
        }
    }

    info!(
        "パイプライン読込: {} タスク {:?} (開始: {})",
        tasks.len(),
        pipeline_dir,
        start_task,
    );

    // ---- (2) デバイス接続確認 + device_width 取得 ----
    let client = anaden_device::AdbClient::new(serial);
    client
        .check_connection()
        .await
        .map_err(|e| anyhow::anyhow!("デバイス未接続({serial}): {e}"))?;

    // ---- (2b) ゲーム起動保証 ----
    // 接続時にゲームが開いている保証はないため、非前景なら起動して前景化を待つ。
    // 起動保証の呼出本体は `ensure_open_outcome`(run/standalone 共有)へ一本化。
    // run パスは Timeout を soft warn として扱いパイプラインを継続する(standalone とは異なる)。
    if ensure_open {
        let outcome = ensure_open_outcome(
            "android",
            Some(serial),
            Duration::from_secs(ensure_open_wait_secs),
        )
        .await?;
        match outcome {
            anaden_device::EnsureOutcome::AlreadyOpen => {
                info!("ゲームは既に前景(起動不要)");
            }
            anaden_device::EnsureOutcome::Launched => {
                info!("ゲームを起動し前景化を確認");
            }
            anaden_device::EnsureOutcome::Timeout => {
                warn!(
                    "ゲーム前景化がタイムアウト({}s)。そのまま続行するが初回 NoMatch が増える可能性あり",
                    ensure_open_wait_secs
                );
            }
        }
    }

    let input = anaden_device::InputExecutor::new(anaden_device::AdbClient::new(serial));

    // ---- (2c) 画面OFF(Doze)対策 ----
    // 画面がタイムアウトで消灯すると `screencap` が純黒フレームを返し、tick が NoMatch 連鎖 →
    // リカバリでゲーム再起動、という遅延パスに入る。ループ開始前に1回だけ `screen_off_timeout`
    // を最大値へ延長する(ループ内で毎回 keyevent は送らない: adb 呼び出し増で性能劣化するため)。
    let display = anaden_device::DisplayController::new(anaden_device::AdbClient::new(serial));
    let _original_screen_off_timeout = display.ensure_stay_on().await;
    if _original_screen_off_timeout.is_some() {
        info!("screen_off_timeout を最大値へ延長(ループ中の黒フレーム抑制)");
    }

    // ---- NoMatch 連続時のゲーム再起動リカバリ ----
    // recover_launch が有効なら、NoMatch が閾値に達したときゲームを再 launch する。
    let recovery: Option<anaden_engine::RecoveryHook> = if recover_launch {
        let controller = anaden_device::AppController::new(anaden_device::AdbClient::new(serial));
        Some(Box::new(move |_streak| {
            let ctrl = controller.clone();
            Box::pin(async move {
                info!("NoMatch 継続: ゲームを再起動します");
                ctrl.launch_app().await
            })
        }))
    } else {
        None
    };

    let interval_dur = Duration::from_secs(interval);

    // 入力経路の解決。`--input scrcpy` は scrcpy control ソケット経由のタッチ注入を使い、
    // capture も同一セッション(video+control 2ソケット)へ一本化される。
    match input_mode {
        "scrcpy" => {
            run_with_scrcpy_session(
                serial,
                start_task,
                tasks,
                width,
                scrcpy_jar,
                interval_dur,
                max_iters,
                recover_nomatch_threshold,
                recovery,
                verify_after_fire,
            )
            .await
        }
        "adb" => match capture_mode {
            "scrcpy" => {
                run_with_capture_scrcpy(
                    serial,
                    start_task,
                    tasks,
                    width,
                    input,
                    scrcpy_jar,
                    interval_dur,
                    max_iters,
                    recover_nomatch_threshold,
                    recovery,
                    verify_after_fire,
                )
                .await
            }
            "screencap" => {
                let capture =
                    anaden_device::ScreenshotCapture::new(anaden_device::AdbClient::new(serial));

                // device_width: --width 指定 > 初回 capture の width 実測
                let device_width = match width {
                    Some(w) => {
                        info!("device_width 指定値: {w}");
                        w
                    }
                    None => {
                        let probe = capture.capture().await.map_err(|e| {
                            anyhow::anyhow!("初回キャプチャ失敗(device_width 実測不可): {e}")
                        })?;
                        let w = probe.width();
                        info!("device_width 実測: {w}(height={})", probe.height());
                        w
                    }
                };

                run_driver(
                    anaden_engine::PipelineDriver::new(
                        capture,
                        input,
                        anaden_engine::PipelineState::new(start_task),
                        tasks,
                        device_width,
                        300,
                    )
                    .with_verify(verify_after_fire),
                    interval_dur,
                    max_iters,
                    recover_nomatch_threshold,
                    recovery,
                )
                .await
            }
            other => {
                anyhow::bail!("--capture は `screencap` または `scrcpy` です(指定値: {other})")
            }
        },
        other => anyhow::bail!("--input は `adb` または `scrcpy` です(指定値: {other})"),
    }
}

/// `--target windows` 時の構築パス。PC版(Windows) Win32 バックエンドへ一本化する。
///
/// ADB/シリアルに依存せず、`Win32Capture` / `Win32InputExecutor` を PipelineDriver へ渡す。
/// 起動保証と NoMatch リカバリは ADB の `AppController` ではなく `Win32Launch` を使う。
/// `windows` 専用なので `#[cfg(windows)]` で gating する(非 Windows ビルドでは対となる
/// フォールバック関数が bail する)。
#[cfg(windows)]
#[allow(clippy::too_many_arguments)]
async fn run_with_windows(
    start_task: &str,
    pipeline_dir: &PathBuf,
    algorithm: Option<&str>,
    interval: u64,
    max_iters: u64,
    width: Option<u32>,
    ensure_open: bool,
    ensure_open_wait_secs: u64,
    recover_launch: bool,
    recover_nomatch_threshold: u32,
    verify_after_fire: bool,
) -> Result<()> {
    // ---- (1) パイプライン読込 + algorithm 上書き ----
    let mut tasks = anaden_vision::load_pipeline(pipeline_dir)
        .map_err(|e| anyhow::anyhow!("パイプライン読込失敗 {pipeline_dir:?}: {e}"))?;
    if tasks.is_empty() {
        anyhow::bail!("パイプラインが空です: {pipeline_dir:?}");
    }

    if let Some(a) = algorithm {
        let algo = resolve_algorithm(a)?;
        for t in tasks.iter_mut() {
            if t.name == start_task {
                t.algorithm = algo;
            }
        }
    }

    info!(
        "パイプライン読込(PC版): {} タスク {:?} (開始: {})",
        tasks.len(),
        pipeline_dir,
        start_task,
    );

    // ---- (2) 起動保証(Win32Launch) ----
    // `launcher` は (5) の NoMatch リカバリフックでも再利用するためここで構築。
    // 起動保証の呼出本体は `ensure_open_outcome`(run/standalone 共有)へ一本化。
    let launcher = anaden_device::Win32Launch::default_paths();
    if ensure_open {
        let outcome =
            ensure_open_outcome("windows", None, Duration::from_secs(ensure_open_wait_secs))
                .await?;
        match outcome {
            anaden_device::EnsureOutcome::AlreadyOpen => info!("ゲームは既に起動中(起動不要)"),
            anaden_device::EnsureOutcome::Launched => info!("ゲームを起動し生存を確認"),
            anaden_device::EnsureOutcome::Timeout => warn!(
                "ゲーム起動がタイムアウト({}s)。そのまま続行するが初回 NoMatch が増える可能性あり",
                ensure_open_wait_secs
            ),
        }
    }

    // ---- (3) capture/input 構築(Win32) ----
    let capture = anaden_device::Win32Capture::default_process();
    let input = anaden_device::Win32InputExecutor::new(anaden_device::DEFAULT_PROCESS_NAME);

    // ---- (4) device_width: --width > 初回 capture の width 実測 ----
    // PC版クライアント生サイズ(1258x708 想定)をそのまま device_width へ採用する。
    // 手動 --width 指定は非推奨(座標ズレの元)。未指定で実測させるのが正解。
    let device_width = match width {
        Some(w) => {
            warn!("device_width 手動指定: {w} (PC版では実測推奨。座標ズレに注意)");
            w
        }
        None => {
            let probe = capture.capture().await.map_err(|e| {
                anyhow::anyhow!("初回キャプチャ失敗(Win32, device_width 実測不可): {e}")
            })?;
            let w = probe.width();
            info!("device_width 実測(PC版): {w}(height={})", probe.height());
            w
        }
    };

    // ---- (5) NoMatch リカバリフック(Win32Launch::launch_app) ----
    let recovery: Option<anaden_engine::RecoveryHook> = if recover_launch {
        let launcher = launcher.clone();
        Some(Box::new(move |_streak| {
            let l = launcher.clone();
            Box::pin(async move {
                info!("NoMatch 継続(PC版): ゲームを再起動します");
                l.launch_app().await
            })
        }))
    } else {
        None
    };

    let interval_dur = Duration::from_secs(interval);

    run_driver(
        anaden_engine::PipelineDriver::new(
            capture,
            input,
            anaden_engine::PipelineState::new(start_task),
            tasks,
            device_width,
            300,
        )
        .with_verify(verify_after_fire),
        interval_dur,
        max_iters,
        recover_nomatch_threshold,
        recovery,
    )
    .await
}

/// `--target windows` 指定だが非 Windows ビルド時のフォールバック(コンパイルエラー回避)。
#[cfg(not(windows))]
#[allow(clippy::too_many_arguments)]
async fn run_with_windows(
    _start_task: &str,
    _pipeline_dir: &PathBuf,
    _algorithm: Option<&str>,
    _interval: u64,
    _max_iters: u64,
    _width: Option<u32>,
    _ensure_open: bool,
    _ensure_open_wait_secs: u64,
    _recover_launch: bool,
    _recover_nomatch_threshold: u32,
    _verify_after_fire: bool,
) -> Result<()> {
    anyhow::bail!(
        "`--target windows` は Windows ビルドでのみ利用可能です。このバイナリは Windows 向けではないため PC版バックエンドを使用できません"
    )
}

/// `run_loop_with_recovery` を呼び出し、結果を人間可読出力する共通末尾。
async fn run_driver<C, I>(
    mut driver: anaden_engine::PipelineDriver<C, I>,
    interval: Duration,
    max_iters: u64,
    recover_nomatch_threshold: u32,
    recovery: Option<anaden_engine::RecoveryHook>,
) -> Result<()>
where
    C: anaden_engine::Capture,
    I: anaden_engine::Input,
{
    info!(
        "run_loop 開始: interval={:?} max_iters={}",
        interval, max_iters
    );
    let outcome = driver
        .run_loop_with_recovery(interval, max_iters, recover_nomatch_threshold, recovery)
        .await;

    println!("\n=== 実行結果 ===");
    println!("サイクル数: {}", outcome.iterations);
    println!("発火回数:   {}", outcome.fired_commands.len());
    println!("終端タスク: {}", outcome.terminal);
    println!(
        "停止理由:   {}",
        match outcome.reason {
            anaden_engine::LoopStopReason::Stop => "Stop アクション到達(正常)",
            anaden_engine::LoopStopReason::TerminalTask => "終端タスク到達(正常)",
            anaden_engine::LoopStopReason::MaxIterations => "最大サイクル到達",
            anaden_engine::LoopStopReason::CaptureError => "キャプチャエラーで停止",
            anaden_engine::LoopStopReason::ExecuteError => "発火エラーで停止",
        }
    );
    for (i, c) in outcome.fired_commands.iter().enumerate() {
        println!("  [{i}] {c:?}");
    }
    Ok(())
}

/// `--capture scrcpy` 時の構築パス。`capture-scrcpy` feature 有効時のみコンパイルされる。
#[cfg(feature = "capture-scrcpy")]
async fn run_with_capture_scrcpy<I>(
    serial: &str,
    start_task: &str,
    tasks: Vec<anaden_vision::TaskDef>,
    width: Option<u32>,
    input: I,
    scrcpy_jar: &str,
    interval: Duration,
    max_iters: u64,
    recover_nomatch_threshold: u32,
    recovery: Option<anaden_engine::RecoveryHook>,
    verify_after_fire: bool,
) -> Result<()>
where
    I: anaden_engine::Input,
{
    let mut config = anaden_device::ScrcpyConfig::default();
    config.local_jar_path = scrcpy_jar.to_string();
    let capture =
        anaden_device::ScrcpyCapture::start(anaden_device::AdbClient::new(serial), config)
            .await
            .map_err(|e| anyhow::anyhow!("scrcpy capture 起動失敗: {e}"))?;

    // device_width: --width 指定 > 初回 capture の width 実測
    let device_width = match width {
        Some(w) => {
            info!("device_width 指定値: {w}");
            w
        }
        None => {
            let probe = capture.capture().await.map_err(|e| {
                anyhow::anyhow!("初回 scrcpy キャプチャ失敗(device_width 実測不可): {e}")
            })?;
            let w = probe.width();
            info!("device_width 実測: {w}(height={})", probe.height());
            w
        }
    };

    run_driver(
        anaden_engine::PipelineDriver::new(
            capture,
            input,
            anaden_engine::PipelineState::new(start_task),
            tasks,
            device_width,
            300,
        )
        .with_verify(verify_after_fire),
        interval,
        max_iters,
        recover_nomatch_threshold,
        recovery,
    )
    .await
}

/// `--input scrcpy` 時の構築パス。`ScrcpySession`(video+control 2ソケット) を1本立て、
/// capture も入力も同一セッションを共有する。`capture-scrcpy` feature 有効時のみ。
///
/// `Arc<ScrcpySession>` は `Capture` と `Input` 両 trait を impl するので、
/// PipelineDriver へ capture にも input にも同じ Arc を渡せる。
#[cfg(feature = "capture-scrcpy")]
async fn run_with_scrcpy_session(
    serial: &str,
    start_task: &str,
    tasks: Vec<anaden_vision::TaskDef>,
    width: Option<u32>,
    scrcpy_jar: &str,
    interval: Duration,
    max_iters: u64,
    recover_nomatch_threshold: u32,
    recovery: Option<anaden_engine::RecoveryHook>,
    verify_after_fire: bool,
) -> Result<()> {
    let mut config = anaden_device::ScrcpySessionConfig::default();
    // jar パスは scoop 既定と同じだが、CLI 引数で上書き可能にする。
    config.local_jar_path = scrcpy_jar.to_string();
    let session =
        anaden_device::ScrcpySession::start(anaden_device::AdbClient::new(serial), config)
            .await
            .map_err(|e| anyhow::anyhow!("scrcpy session 起動失敗: {e}"))?;

    if !session.control_ready() {
        anyhow::bail!("scrcpy session の control ソケットが未確立(タッチ注入不可)");
    }
    info!("scrcpy session 起動完了(control_ready=true): 入力経路を scrcpy-touch へ切替え");

    // device_width: --width 指定 > 初回 capture の width 実測
    let device_width = match width {
        Some(w) => {
            info!("device_width 指定値: {w}");
            w
        }
        None => {
            let probe = session.capture().await.map_err(|e| {
                anyhow::anyhow!("初回 scrcpy キャプチャ失敗(device_width 実測不可): {e}")
            })?;
            let w = probe.width();
            info!("device_width 実測: {w}(height={})", probe.height());
            w
        }
    };

    let session = std::sync::Arc::new(session);
    run_driver(
        anaden_engine::PipelineDriver::new(
            session.clone(),
            session.clone(),
            anaden_engine::PipelineState::new(start_task),
            tasks,
            device_width,
            300,
        )
        .with_verify(verify_after_fire),
        interval,
        max_iters,
        recover_nomatch_threshold,
        recovery,
    )
    .await
}

/// `--input scrcpy` 指定だが feature 無効時のフォールバック(コンパイルエラー回避)。
#[cfg(not(feature = "capture-scrcpy"))]
#[allow(clippy::too_many_arguments)]
async fn run_with_scrcpy_session(
    _serial: &str,
    _start_task: &str,
    _tasks: Vec<anaden_vision::TaskDef>,
    _width: Option<u32>,
    _scrcpy_jar: &str,
    _interval: Duration,
    _max_iters: u64,
    _recover_nomatch_threshold: u32,
    _recovery: Option<anaden_engine::RecoveryHook>,
    _verify_after_fire: bool,
) -> Result<()> {
    anyhow::bail!(
        "`--input scrcpy` は `capture-scrcpy` feature 無効では使用できません。`--features anaden-cli/capture-scrcpy` でビルドしてください"
    )
}

/// `--capture scrcpy` 指定だが feature 無効時のフォールバック(コンパイルエラー回避)。
#[cfg(not(feature = "capture-scrcpy"))]
#[allow(clippy::too_many_arguments)]
async fn run_with_capture_scrcpy<I>(
    _serial: &str,
    _start_task: &str,
    _tasks: Vec<anaden_vision::TaskDef>,
    _width: Option<u32>,
    _input: I,
    _scrcpy_jar: &str,
    _interval: Duration,
    _max_iters: u64,
    _recover_nomatch_threshold: u32,
    _recovery: Option<anaden_engine::RecoveryHook>,
    _verify_after_fire: bool,
) -> Result<()>
where
    I: anaden_engine::Input,
{
    anyhow::bail!(
        "`--capture scrcpy` は `capture-scrcpy` feature 無効では使用できません。`--features anaden-engine/capture-scrcpy` でビルドしてください"
    )
}

/// `legacy` サブコマンド: Orchestrator(命令型 Strategy ループ) を実行する。
async fn run_legacy(
    device: String,
    templates: PathBuf,
    interval: u64,
    threshold: f32,
    timeout: u64,
    config: Option<PathBuf>,
) -> Result<()> {
    let config = if let Some(config_path) = config {
        let content = std::fs::read_to_string(&config_path)?;
        let mut cfg: AutomationConfig = toml::from_str(&content)?;
        if device != "localhost:5555" {
            cfg.device_serial = device;
        }
        cfg
    } else {
        AutomationConfig {
            device_serial: device,
            template_dir: templates.to_string_lossy().to_string(),
            loop_interval_ms: interval,
            confidence_threshold: threshold,
            max_runtime_secs: timeout,
            ..Default::default()
        }
    };

    info!("Starting anaden-helper with config: {:?}", config);

    let mut orchestrator = Orchestrator::new(config);
    let summary = orchestrator.run().await?;

    info!("Automation completed: {:?}", summary);
    println!("\n=== 実行結果 ===");
    println!("総ループ回数: {}", summary.total_loops);
    println!("実行時間: {:.1}秒", summary.elapsed_secs);
    println!("終了理由: {}", summary.termination_reason);
    println!("\n状態別滞在回数:");
    for (state, count) in &summary.state_counts {
        println!("  {}: {}", state, count);
    }

    Ok(())
}

/// `ensure-open` / `launch` サブコマンドの実行部(Issue #21)。
///
/// `target` でバックエンドを解決し、`force_launch` で挙動を切り替える:
/// - `force_launch == false`(`ensure-open`): ゲームが非前景/未起動なら起動し、前景化/生存を
///   `wait` の間ポーリング確認する。起動保証の呼出本体は [`ensure_open_outcome`] へ一本化
///   (`run` パスと同一の真実の源・ドリフト防止)。
/// - `force_launch == true`(`launch`): 常に launch を1回実行した上で、前景化/生存を
///   [`ensure_open_outcome`] で確認する。
///
/// 戻り値は `Result<EnsureOutcome, anyhow::Error>`:
/// - `Ok(outcome)`: ドメイン成果物。`main()` が [`exit_standalone`] へ渡し、
///   [`standalone_exit_code`] で exit code を決定し、[`ensure_outcome_label`] の1行を
///   印字してから `std::process::exit` する。
/// - `Err(_)`: ハードエラー(AdbError / spawn / OpenProcess 失敗)。同じく [`exit_standalone`]
///   が [`standalone_exit_code`] で `EXIT_HARDCERROR`(1) へ射影し exit 1 する
///   (AC4 の真経路。本関数では exit しない=テスト可能性と panic 禁止のため)。
async fn run_ensure_open_or_launch(
    target: &str,
    serial: Option<&str>,
    wait_secs: u64,
    force_launch: bool,
) -> Result<anaden_device::EnsureOutcome> {
    let wait = Duration::from_secs(wait_secs);
    // force_launch 時の先行 launch は起動済みかに関わらず常に 1 回行う(launch サブコマンド専用)。
    // その後の生存/前景化確認は `ensure_open_outcome` で `run` パスと共有する。
    if force_launch {
        force_launch_app(target, serial).await?;
    }
    ensure_open_outcome(target, serial, wait).await
}

/// スタンドアロン(`ensure-open`/`launch`)サブコマンドの成果物→終了コード適用(唯一の exit 点)。
///
/// [`standalone_exit_code`] で Ok/Err 両方の終了コードを決定し(AC1-AC4 の契約適用)、
/// 人間可読1行を印字してから `std::process::exit` する。`-> !` で常に diverge するため、
/// main の match arm はこれだけで完結する(`?` bubble による暗黙 exit 1 を廃止し、
/// AC4「hard error ⇒ exit 1」を明示的な真経路へ置換)。
fn exit_standalone(result: Result<anaden_device::EnsureOutcome, anyhow::Error>) -> ! {
    let code = standalone_exit_code(result.as_ref());
    match &result {
        Ok(outcome) => println!("{}", ensure_outcome_label(outcome)),
        Err(e) => eprintln!("ensure-open/launch 失敗: {e}"),
    }
    std::process::exit(code);
}

/// `launch` サブコマンドの先行起動(`force_launch == true` 時)。起動済みかに関わらず
/// 常に 1 回 `launch_app` を呼ぶ。実機呼出のみで戻り値なし(成果物は呼出元の ensure が担う)。
#[cfg(windows)]
async fn force_launch_app(target: &str, serial: Option<&str>) -> Result<()> {
    match target {
        "android" => {
            let serial = serial.ok_or_else(|| {
                anyhow::anyhow!("--target android 時は ADB シリアル(serial)が必須です")
            })?;
            let controller =
                anaden_device::AppController::new(anaden_device::AdbClient::new(serial));
            controller
                .launch_app()
                .await
                .map_err(|e| anyhow::anyhow!("ゲーム起動に失敗({serial}): {e}"))
        }
        "windows" => {
            let launcher = anaden_device::Win32Launch::default_paths();
            launcher
                .launch_app()
                .await
                .map_err(|e| anyhow::anyhow!("ゲーム起動(Win32)に失敗: {e}"))
        }
        other => anyhow::bail!("--target は `android` または `windows` です(指定値: {other})"),
    }
}

/// `force_launch_app` の非 Windows ビルド向けフォールバック(コンパイルエラー回避)。
///
/// android(ADB) はプラットフォーム非依存なので機能する。bail するのは `windows` のみ。
#[cfg(not(windows))]
async fn force_launch_app(target: &str, serial: Option<&str>) -> Result<()> {
    match target {
        "android" => {
            let serial = serial.ok_or_else(|| {
                anyhow::anyhow!("--target android 時は ADB シリアル(serial)が必須です")
            })?;
            let controller =
                anaden_device::AppController::new(anaden_device::AdbClient::new(serial));
            controller
                .launch_app()
                .await
                .map_err(|e| anyhow::anyhow!("ゲーム起動に失敗({serial}): {e}"))
        }
        "windows" => anyhow::bail!(
            "`--target windows` は Windows ビルドでのみ利用可能です。このバイナリは Windows 向けではないため PC版バックエンドを使用できません"
        ),
        other => anyhow::bail!("--target は `android` または `windows` です(指定値: {other})"),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // ロギングの初期化
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "anaden=info".into()),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Run {
            serial,
            target,
            pipeline_dir,
            start_task,
            algorithm,
            interval,
            max_iters,
            width,
            ensure_open,
            ensure_open_wait_secs,
            recover_launch,
            recover_nomatch_threshold,
            capture,
            scrcpy_jar,
            input,
            verify_after_fire,
        } => {
            run_pipeline_live(
                serial.as_deref(),
                &pipeline_dir,
                &start_task,
                algorithm.as_deref(),
                interval,
                max_iters,
                width,
                ensure_open,
                ensure_open_wait_secs,
                recover_launch,
                recover_nomatch_threshold,
                &capture,
                &scrcpy_jar,
                &input,
                &target,
                verify_after_fire,
            )
            .await
        }
        Commands::Legacy {
            device,
            templates,
            interval,
            threshold,
            timeout,
            config,
        } => {
            if let Err(e) =
                run_legacy(device, templates, interval, threshold, timeout, config).await
            {
                warn!("legacy 実行エラー: {e}");
                return Err(e);
            }
            Ok(())
        }
        Commands::EnsureOpen {
            target,
            serial,
            wait_secs,
        } => {
            // ensure-open: 起動状態確認＋必要なら起動。Timeout は CI gate 失敗(exit 2)。
            // Ok/Err 双方の終了コードを `exit_standalone`(唯一の exit 点)で適用する。
            exit_standalone(
                run_ensure_open_or_launch(&target, serial.as_deref(), wait_secs, false).await,
            );
        }
        Commands::Launch {
            target,
            serial,
            wait_secs,
        } => {
            // launch: 無条件起動(AlreadyOpen チェックなし)。終了コード契約は ensure-open に同じ。
            exit_standalone(
                run_ensure_open_or_launch(&target, serial.as_deref(), wait_secs, true).await,
            );
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use anaden_cli_contract::{EXIT_TIMEOUT, ensure_open_exit_code};

    // ---- ensure_outcome_label (pure contract, device-free) ----
    // この純粋関数が `run` パス(android/windows)と standalone サブコマンドの
    // 「EnsureOutcome → 人間可読メッセージ」唯一の真実の源(single source of truth)。
    // 両経路のドリフトを防ぐため、ラベル化は必ずこの関数へ集中させる。

    #[test]
    fn label_already_open() {
        assert_eq!(
            ensure_outcome_label(&anaden_device::EnsureOutcome::AlreadyOpen),
            "起動不要(既に起動中)"
        );
    }

    #[test]
    fn label_launched() {
        assert_eq!(
            ensure_outcome_label(&anaden_device::EnsureOutcome::Launched),
            "起動し生存を確認"
        );
    }

    #[test]
    fn label_timeout() {
        assert_eq!(
            ensure_outcome_label(&anaden_device::EnsureOutcome::Timeout),
            "起動タイムアウト"
        );
    }

    #[test]
    fn label_covers_all_variants() {
        // EnsureOutcome へ新バリアントが追加された際、このテストがラベル未対応を検出する。
        let variants = [
            anaden_device::EnsureOutcome::AlreadyOpen,
            anaden_device::EnsureOutcome::Launched,
            anaden_device::EnsureOutcome::Timeout,
        ];
        for v in &variants {
            // 各バリアントが空でないラベルへ解決されること(フォールバック漏れ検出)。
            let label = ensure_outcome_label(v);
            assert!(!label.is_empty(), "variant {:?} produced empty label", v);
        }
    }

    // ---- ensure_open_exit_code (Issue #21 exit-code contract, device-free) ----
    // 契約: AlreadyOpen=0, Launched=0, Timeout=2(非0・ハードエラー1とは区別)。
    // ハードエラー(AdbError/spawn/OpenProcess 失敗)は standalone_exit_code が Err を
    // EXIT_HARDCERROR(1) へ射影する(main exit_standalone の真経路・AC4)。本テストは
    // 「ソフト成果物の exit code」のみ検証し、Err 側は ensure_open_cli.rs の真正テストが担う。

    #[test]
    fn exit_code_already_open_is_zero() {
        // CI gate は AlreadyOpen を success とみなす(UC-1: 起動スキップ)。
        assert_eq!(
            ensure_open_exit_code(&anaden_device::EnsureOutcome::AlreadyOpen),
            0
        );
    }

    #[test]
    fn exit_code_launched_is_zero() {
        // CI gate は Launched を success とみなす(UC-2: 起動+生存確認)。
        assert_eq!(
            ensure_open_exit_code(&anaden_device::EnsureOutcome::Launched),
            0
        );
    }

    #[test]
    fn exit_code_timeout_is_non_zero_and_distinct_from_hard_error() {
        // UC-3: Timeout は CI gate 失敗(非0)。推奨契約値 2 はハードエラー(exit 1 via
        // standalone_exit_code)と区別される「ソフト失敗」。この分離が run-vs-standalone の意味の分裂の核。
        let code = ensure_open_exit_code(&anaden_device::EnsureOutcome::Timeout);
        assert_ne!(code, 0, "Timeout must be non-zero for CI gate");
        assert_eq!(code, EXIT_TIMEOUT, "Timeout must map to EXIT_TIMEOUT (2)");
        assert_ne!(
            code, 1,
            "Timeout must be distinct from hard-error exit code 1"
        );
    }

    #[test]
    fn exit_code_success_variants_share_zero() {
        // AlreadyOpen と Launched は CI gate 上同一个(success)。両者が異なる exit code だと
        // スクリプト分岐が無用に複雑になる。同一の 0 に揃っていることを検証。
        assert_eq!(
            ensure_open_exit_code(&anaden_device::EnsureOutcome::AlreadyOpen),
            ensure_open_exit_code(&anaden_device::EnsureOutcome::Launched)
        );
    }

    // ---- ensure_open_outcome: ターゲット解離・引数バリデーション(デバイス非依存分岐) ----
    // 実機呼出(android: ADB / windows: Win32)は各バックエンドのユニットテストで検証済み。
    // ここではバックエンドへ到達する前に bail する分岐(不正ターゲット・serial 必須)のみ検証する。
    // これが `run` パスと standalone サブコマンドの共有する ensure 本体の契約入口。

    #[tokio::test]
    async fn ensure_open_outcome_rejects_unknown_target() {
        // 不正ターゲットはバックエンド解決前にエラー(panic しない)。
        let result =
            ensure_open_outcome("ios", Some("localhost:5555"), Duration::from_secs(1)).await;
        assert!(result.is_err(), "unknown target must error, not panic");
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.contains("--target"),
            "error should mention --target validation, got: {msg}"
        );
    }

    #[tokio::test]
    async fn ensure_open_outcome_android_requires_serial() {
        // android ターゲットで serial 未指定は引数エラー(panic しない)。
        // シリアル解決で即 bail するため実機 ADB 呼出へ到達しない。
        let result = ensure_open_outcome("android", None, Duration::from_secs(1)).await;
        assert!(result.is_err(), "android without serial must error");
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.contains("serial"),
            "error should mention missing serial, got: {msg}"
        );
    }

    #[tokio::test]
    async fn force_launch_app_rejects_unknown_target() {
        // launch サブコマンドの先行起動も同じターゲットバリデーションを共有する。
        let result = force_launch_app("ios", Some("localhost:5555")).await;
        assert!(result.is_err(), "unknown target must error, not panic");
        assert!(
            format!("{}", result.unwrap_err()).contains("--target"),
            "error should mention --target validation"
        );
    }
}
