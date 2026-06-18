//! Another Eden 自動操作ツールの CLI エントリポイント。
//!
//! 2つのサブコマンドを持つ:
//! - `run`: 宣言的パイプラインを ADB 実機でライブ実行する(PipelineDriver 駆動)。
//! - `legacy`: 旧来の Orchestrator(命令型 Strategy ループ) を動かす後方互換エントリ。

use std::path::PathBuf;
use std::time::Duration;

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
    if ensure_open {
        let controller = anaden_device::AppController::new(anaden_device::AdbClient::new(serial));
        let outcome = controller
            .ensure_app_open(Duration::from_secs(ensure_open_wait_secs))
            .await
            .map_err(|e| anyhow::anyhow!("ゲーム起動保証に失敗({serial}): {e}"))?;
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
                    ),
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
    let launcher = anaden_device::Win32Launch::default_paths();
    if ensure_open {
        let outcome = launcher
            .ensure_open(Duration::from_secs(ensure_open_wait_secs))
            .await
            .map_err(|e| anyhow::anyhow!("ゲーム起動保証(Win32)に失敗: {e}"))?;
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
        ),
        interval_dur,
        max_iters,
        recover_nomatch_threshold,
        recovery,
    )
    .await
}

/// `--target windows` 指定だが非 Windows ビルド時のフォールバック(コンパイルエラー回避)。
#[cfg(not(windows))]
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
        ),
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
        ),
        interval,
        max_iters,
        recover_nomatch_threshold,
        recovery,
    )
    .await
}

/// `--input scrcpy` 指定だが feature 無効時のフォールバック(コンパイルエラー回避)。
#[cfg(not(feature = "capture-scrcpy"))]
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
) -> Result<()> {
    anyhow::bail!(
        "`--input scrcpy` は `capture-scrcpy` feature 無効では使用できません。`--features anaden-cli/capture-scrcpy` でビルドしてください"
    )
}

/// `--capture scrcpy` 指定だが feature 無効時のフォールバック(コンパイルエラー回避)。
#[cfg(not(feature = "capture-scrcpy"))]
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
    }
}
