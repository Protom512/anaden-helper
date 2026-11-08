use std::path::{Path, PathBuf};
use std::time::Duration;

pub(crate) mod e2e;
use e2e::tree_hash_of_pipeline;

mod routine;

mod run_report;
use run_report::RunArtifacts;

use anaden_cli_contract::{ensure_outcome_label, standalone_exit_code};
use anyhow::Result;
use clap::{Parser, Subcommand};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use anaden_core::Goal;

const RECOVERY_EXIT_WAIT_SECS: u64 = 15;

const DEFAULT_MAX_RECOVERY_ATTEMPTS: u32 = 5;

#[derive(Parser, Debug)]
#[command(
    name = "anaden",
    version,
    about = "Another Eden automation helper (PC/Windows)"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
#[allow(clippy::large_enum_variant)]
enum Commands {
    Run {
        pipeline_dir: PathBuf,
        start_task: String,
        #[arg(long)]
        algorithm: Option<String>,
        #[arg(long, default_value_t = 1)]
        interval: u64,
        #[arg(long, default_value_t = 100)]
        max_iters: u64,
        #[arg(long)]
        width: Option<u32>,
        #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
        ensure_open: bool,
        #[arg(long, default_value_t = 30)]
        ensure_open_wait_secs: u64,
        #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
        recover_launch: bool,
        #[arg(long, default_value_t = 5)]
        recover_nomatch_threshold: u32,
        #[arg(long, default_value_t = 180)]
        recover_grace_secs: u64,
        #[arg(long, default_value_t = DEFAULT_MAX_RECOVERY_ATTEMPTS)]
        max_recovery_attempts: u32,
        #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
        verify_after_fire: bool,
        #[arg(long)]
        jitter: bool,
        #[arg(long)]
        jitter_seed: Option<u64>,
        #[arg(long)]
        goal: Option<String>,
        #[arg(long)]
        goal_file: Option<PathBuf>,
        #[arg(long)]
        evidence_run_id: Option<String>,
        #[arg(long)]
        report_file: Option<PathBuf>,
        #[arg(long)]
        no_persist: bool,
    },
    Routine {
        routine_path: PathBuf,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        evidence_run_id: Option<String>,
        #[arg(long)]
        report_file: Option<PathBuf>,
        #[arg(long)]
        no_persist: bool,
    },
    EnsureOpen {
        #[arg(long, default_value_t = 30)]
        wait_secs: u64,
        #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
        readiness: bool,
    },
    Launch {
        #[arg(long, default_value_t = 30)]
        wait_secs: u64,
        #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
        readiness: bool,
    },
}

#[cfg(windows)]
pub(crate) async fn ensure_open_outcome(wait: Duration) -> Result<anaden_device::EnsureOutcome> {
    let launcher = anaden_device::Win32Launch::default_paths();
    launcher
        .ensure_open(wait)
        .await
        .map_err(|e| anyhow::anyhow!("ゲーム起動保証(Win32)に失敗: {e}"))
}

#[cfg(not(windows))]
pub(crate) async fn ensure_open_outcome(_wait: Duration) -> Result<anaden_device::EnsureOutcome> {
    anyhow::bail!(
        "このバイナリは Windows 向けではないため PC 版 (Win32) バックエンドを使用できません"
    )
}

pub(crate) fn cli_workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

fn resolve_pipeline_dir(input: &Path, root: &Path) -> PathBuf {
    anaden_engine::resolve_pipeline_dir(input, root).unwrap_or_else(|| input.to_path_buf())
}

fn routine_artifacts_base_dir(routine_path: &Path) -> &Path {
    routine_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}

fn resolve_algorithm(value: &str) -> Result<anaden_vision::Algorithm> {
    match value {
        "sse" => Ok(anaden_vision::Algorithm::Sse),
        "ccoeff" => Ok(anaden_vision::Algorithm::Ccoeff),
        other => anyhow::bail!("--algorithm は `sse` または `ccoeff` です(指定値: {other})"),
    }
}

fn parse_goal_flag(
    goal: Option<&str>,
    goal_file: Option<&std::path::Path>,
) -> Result<Option<Goal>> {
    match (goal, goal_file) {
        (None, None) => Ok(None),
        (Some(_), Some(_)) => anyhow::bail!(
            "--goal と --goal-file は exclusive(排他)です。いずれか一方のみ指定してください"
        ),
        (Some(inline), None) => {
            let parsed: Goal = toml::from_str(inline)
                .map_err(|e| anyhow::anyhow!("--goal の TOML パース失敗: {e}"))?;
            parsed
                .validate()
                .map_err(|e| anyhow::anyhow!("--goal のバリデーション失敗: {e}"))?;
            Ok(Some(parsed))
        }
        (None, Some(path)) => {
            let content = std::fs::read_to_string(path)
                .map_err(|e| anyhow::anyhow!("--goal-file の読込失敗 {}: {e}", path.display()))?;
            let parsed: Goal = toml::from_str(&content)
                .map_err(|e| anyhow::anyhow!("--goal-file の TOML パース失敗: {e}"))?;
            parsed
                .validate()
                .map_err(|e| anyhow::anyhow!("--goal-file のバリデーション失敗: {e}"))?;
            Ok(Some(parsed))
        }
    }
}

fn resolve_goal(
    goal: Option<&str>,
    goal_file: Option<&std::path::Path>,
    pipeline_dir: &std::path::Path,
) -> Result<Option<Goal>> {
    if goal.is_some() || goal_file.is_some() {
        return parse_goal_flag(goal, goal_file);
    }
    manifest_default_goal(pipeline_dir)
}

fn manifest_default_goal(pipeline_dir: &std::path::Path) -> Result<Option<Goal>> {
    let manifest_path = pipeline_dir.join(anaden_vision::PIPELINE_MANIFEST_FILENAME);
    if !manifest_path.is_file() {
        return Ok(None);
    }
    let manifest = anaden_vision::load_pipeline_manifest(pipeline_dir).map_err(|e| {
        anyhow::anyhow!(
            "pipeline.toml manifest の読込失敗 {}: {e}",
            manifest_path.display()
        )
    })?;
    let goal = match manifest.goals.split_first() {
        None => return Ok(None),
        Some((only, [])) => only.clone(),
        Some(_) => {
            let names = manifest
                .goals
                .iter()
                .map(|g| g.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            Goal {
                name: format!("any({names})"),
                stop: anaden_core::StopCondition::Any {
                    conditions: manifest.goals.iter().map(|g| g.stop.clone()).collect(),
                },
            }
        }
    };
    goal.validate()
        .map_err(|e| anyhow::anyhow!("manifest goal のバリデーション失敗: {e}"))?;
    Ok(Some(goal))
}

#[cfg(windows)]
pub(crate) fn escalation_recovery_hook(
    launcher: anaden_device::Win32Launch,
    label: &'static str,
) -> anaden_engine::RecoveryHook {
    Box::new(move |ctx: anaden_engine::RecoveryContext| {
        let l = launcher.clone();
        Box::pin(async move {
            if ctx.attempt == 1 {
                info!(
                    "NoMatch 継続({label}): ゲームを起動します \
                     (attempt 1・kill なし・Issue #210)"
                );
                l.launch_app().await
            } else {
                info!(
                    "NoMatch 継続({label}): ゲームを kill+再起動します \
                     (attempt {}・Issue #210)",
                    ctx.attempt
                );
                l.restart_app(Duration::from_secs(RECOVERY_EXIT_WAIT_SECS))
                    .await
            }
        })
    })
}

#[allow(clippy::too_many_arguments)]
async fn run_pipeline_live(
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
    recover_grace_secs: u64,
    max_recovery_attempts: u32,
    verify_after_fire: bool,
    jitter_seed: Option<u64>,
    goal: Option<Goal>,
    cancel_token: CancellationToken,
    evidence_dir: Option<&Path>,
    artifacts: &RunArtifacts,
) -> Result<anaden_engine::LoopStopReason> {
    spawn_ctrl_c_listener(cancel_token.clone());

    if let Some(g) = &goal {
        info!("宣言的ゴール活性: {} ({:?})", g.name, g.stop);
    } else {
        info!("宣言的ゴール無し(従来モード: MaxIterations/Stop/TerminalTask で停止)");
    }

    run_with_windows(
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
        recover_grace_secs,
        max_recovery_attempts,
        verify_after_fire,
        jitter_seed,
        goal,
        cancel_token,
        evidence_dir,
        artifacts,
    )
    .await
}

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
    recover_grace_secs: u64,
    max_recovery_attempts: u32,
    verify_after_fire: bool,
    jitter_seed: Option<u64>,
    goal: Option<Goal>,
    cancel_token: CancellationToken,
    evidence_dir: Option<&Path>,
    artifacts: &RunArtifacts,
) -> Result<anaden_engine::LoopStopReason> {
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

    let watch_names = resolve_watch_tasks(pipeline_dir, &tasks)?;
    if !watch_names.is_empty() {
        info!("watch (常時監視) タスク: {watch_names:?} — current タスクより優先評価");
    }

    let launcher = anaden_device::Win32Launch::default_paths();
    if ensure_open {
        let outcome = ensure_open_outcome(Duration::from_secs(ensure_open_wait_secs)).await?;
        match outcome {
            anaden_device::EnsureOutcome::AlreadyOpen => info!("ゲームは既に起動中(起動不要)"),
            anaden_device::EnsureOutcome::Launched => info!("ゲームを起動し生存を確認"),
            anaden_device::EnsureOutcome::Timeout => warn!(
                "ゲーム起動がタイムアウト({}s)。そのまま続行するが初回 NoMatch が増える可能性あり",
                ensure_open_wait_secs
            ),
            anaden_device::EnsureOutcome::NotReady => warn!(
                "ゲームは生存しているが readiness probe が splash-hang を検出。そのまま続行するが NoMatch リカバリが発火する可能性あり"
            ),
        }
    }

    let capture = anaden_device::Win32Capture::default_process();
    let input = anaden_device::Win32InputExecutor::new(anaden_device::DEFAULT_PROCESS_NAME);
    if let Some(dir) = evidence_dir {
        let _ = e2e::append_evidence_line(
            dir,
            &format!(
                "ANADEN_E2E backend=win32 process={} pipeline={:?} start={start_task}",
                anaden_device::DEFAULT_PROCESS_NAME,
                pipeline_dir.display().to_string()
            ),
        );
    }

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

    let recovery: Option<anaden_engine::RecoveryHook> = if recover_launch {
        Some(escalation_recovery_hook(launcher.clone(), "PC版"))
    } else {
        None
    };
    let recovery_grace = Duration::from_secs(recover_grace_secs);

    let interval_dur = Duration::from_secs(interval);

    let jitter = jitter_seed.map(|seed| {
        info!("jitter 有効: seed={seed} (tap ±3px / interval ±15%)");
        anaden_engine::Jitter::new(seed)
    });

    if let Some(dir) = evidence_dir {
        run_driver(
            anaden_engine::PipelineDriver::new(
                e2e::EvidenceCapture::new(capture, dir.to_path_buf()),
                input,
                anaden_engine::PipelineState::new(start_task),
                tasks,
                device_width,
                300,
            )
            .with_verify(verify_after_fire)
            .with_jitter(jitter.clone())
            .with_watch(watch_names.clone())
            .with_cancel(cancel_token)
            .with_recovery_grace(recovery_grace)
            .with_max_recovery_attempts(max_recovery_attempts),
            interval_dur,
            max_iters,
            recover_nomatch_threshold,
            recovery,
            goal,
            artifacts,
        )
        .await
    } else {
        run_driver(
            anaden_engine::PipelineDriver::new(
                capture,
                input,
                anaden_engine::PipelineState::new(start_task),
                tasks,
                device_width,
                300,
            )
            .with_verify(verify_after_fire)
            .with_jitter(jitter)
            .with_watch(watch_names)
            .with_cancel(cancel_token)
            .with_recovery_grace(recovery_grace)
            .with_max_recovery_attempts(max_recovery_attempts),
            interval_dur,
            max_iters,
            recover_nomatch_threshold,
            recovery,
            goal,
            artifacts,
        )
        .await
    }
}

fn resolve_watch_tasks(
    pipeline_dir: &std::path::Path,
    tasks: &[anaden_vision::TaskDef],
) -> Result<Vec<String>> {
    let manifest_path = pipeline_dir.join(anaden_vision::PIPELINE_MANIFEST_FILENAME);
    if !manifest_path.is_file() {
        return Ok(Vec::new());
    }
    let manifest = anaden_vision::load_pipeline_manifest(pipeline_dir)
        .map_err(|e| anyhow::anyhow!("pipeline.toml manifest の読込失敗: {e}"))?;
    let missing: Vec<&String> = manifest
        .watch
        .iter()
        .filter(|w| !tasks.iter().any(|t| &t.name == *w))
        .collect();
    if missing.is_empty() {
        Ok(manifest.watch)
    } else {
        anyhow::bail!(
            "manifest の watch タスクが pipeline 内に存在しない: {missing:?} \
             (TaskDef 名と一致させてください)"
        );
    }
}

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
    _recover_grace_secs: u64,
    _max_recovery_attempts: u32,
    _verify_after_fire: bool,
    _jitter_seed: Option<u64>,
    _goal: Option<Goal>,
    _cancel_token: CancellationToken,
    _evidence_dir: Option<&Path>,
    _artifacts: &RunArtifacts,
) -> Result<anaden_engine::LoopStopReason> {
    anyhow::bail!(
        "このバイナリは Windows 向けではないため PC 版 (Win32) バックエンドを使用できません"
    )
}

async fn run_driver<C, I>(
    mut driver: anaden_engine::PipelineDriver<C, I>,
    interval: Duration,
    max_iters: u64,
    recover_nomatch_threshold: u32,
    recovery: Option<anaden_engine::RecoveryHook>,
    goal: Option<Goal>,
    artifacts: &RunArtifacts,
) -> Result<anaden_engine::LoopStopReason>
where
    C: anaden_engine::Capture,
    I: anaden_engine::Input,
{
    anaden_cli_contract::validate_goal(&goal)?;

    info!(
        "run_loop 開始: interval={:?} max_iters={} goal={}",
        interval,
        max_iters,
        goal.as_ref().map(|g| g.name.as_str()).unwrap_or("(none)")
    );
    let outcome = if goal.is_some() {
        driver
            .run_loop_with_goal(
                interval,
                max_iters,
                recover_nomatch_threshold,
                recovery,
                goal.as_ref(),
                anaden_engine::SystemClock::new(),
            )
            .await
    } else {
        driver
            .run_loop_with_recovery(interval, max_iters, recover_nomatch_threshold, recovery)
            .await
    };

    println!("\n=== 実行結果 ===");
    println!("サイクル数: {}", outcome.iterations);
    println!("発火回数:   {}", outcome.fired_commands.len());
    println!("終端タスク: {}", outcome.terminal);
    println!(
        "停止理由:   {}",
        anaden_engine::stop_reason_label(outcome.reason.clone())
    );
    for (i, c) in outcome.fired_commands.iter().enumerate() {
        println!("  [{i}] {c:?}");
    }
    if let Some(path) = artifacts.report_path.as_ref() {
        run_report::write_report(path, &outcome)
            .map_err(|e| anyhow::anyhow!("run レポート書込失敗 {}: {e}", path.display()))?;
        info!("run レポート永続化: {}", path.display());
    }
    Ok(outcome.reason)
}

async fn run_ensure_open_or_launch(
    wait_secs: u64,
    force_launch: bool,
    readiness: bool,
) -> Result<anaden_device::EnsureOutcome> {
    let wait = Duration::from_secs(wait_secs);
    if force_launch {
        force_launch_app().await?;
    }
    let outcome = ensure_open_outcome(wait).await?;
    if readiness {
        return apply_readiness_probe(outcome).await;
    }
    Ok(outcome)
}

#[cfg(windows)]
async fn apply_readiness_probe(
    outcome: anaden_device::EnsureOutcome,
) -> Result<anaden_device::EnsureOutcome> {
    use anaden_device::ReadinessVerdict;

    if !matches!(
        outcome,
        anaden_device::EnsureOutcome::AlreadyOpen | anaden_device::EnsureOutcome::Launched
    ) {
        return Ok(outcome);
    }
    let samples = anaden_device::probe_readiness(
        anaden_device::DEFAULT_PROCESS_NAME,
        anaden_device::ReadinessProbeConfig::default(),
    )
    .await
    .map_err(|e| anyhow::anyhow!("readiness probe に失敗: {e}"))?;
    if anaden_device::classify_readiness(&samples) == ReadinessVerdict::SplashHang {
        warn!(
            "readiness probe: フレーム不変かつ TCP ソケット 0 = splash-hang (not-ready)。exit 3 で fail-closed"
        );
        return Ok(anaden_device::EnsureOutcome::NotReady);
    }
    Ok(outcome)
}

#[cfg(not(windows))]
async fn apply_readiness_probe(
    outcome: anaden_device::EnsureOutcome,
) -> Result<anaden_device::EnsureOutcome> {
    anyhow::bail!(
        "このバイナリは Windows 向けではないため readiness probe (Win32) を使用できません"
    )
}

fn exit_standalone(result: Result<anaden_device::EnsureOutcome, anyhow::Error>) -> ! {
    let code = standalone_exit_code(result.as_ref());
    match &result {
        Ok(outcome) => println!("{}", ensure_outcome_label(outcome)),
        Err(e) => eprintln!("ensure-open/launch 失敗: {e}"),
    }
    std::process::exit(code);
}

#[cfg(windows)]
async fn force_launch_app() -> Result<()> {
    let launcher = anaden_device::Win32Launch::default_paths();
    launcher
        .launch_app()
        .await
        .map_err(|e| anyhow::anyhow!("ゲーム起動(Win32)に失敗: {e}"))
}

#[cfg(not(windows))]
async fn force_launch_app() -> Result<()> {
    anyhow::bail!(
        "このバイナリは Windows 向けではないため PC 版 (Win32) バックエンドを使用できません"
    )
}

fn spawn_ctrl_c_listener(token: CancellationToken) {
    tokio::spawn(async move {
        if let Err(e) = tokio::signal::ctrl_c().await {
            warn!("ctrl_c シグナルリスナの設定に失敗しました(Ctrl+C で安全停止できません): {e}");
            return;
        }
        info!("Ctrl+C / SIGINT を受信: キャンセルトークンを発火します(安全停止へ遷移)");
        token.cancel();
    });
}

struct DualWriter {
    file: std::sync::Arc<std::fs::File>,
}

impl std::io::Write for DualWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let mut stdout = std::io::stdout();
        let _ = stdout.write(buf);
        let _ = stdout.flush();
        self.file.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        let _ = std::io::stdout().flush();
        self.file.flush()
    }
}

fn init_tracing(log_file: Option<&Path>) {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "anaden=info".into());
    let file = log_file.and_then(|p| {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(p)
            .ok()
    });
    match file {
        Some(file) => {
            let file = std::sync::Arc::new(file);
            tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_ansi(false)
                .with_writer(move || DualWriter { file: file.clone() })
                .init();
        }
        None => {
            tracing_subscriber::fmt().with_env_filter(filter).init();
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let command_artifacts = match &cli.command {
        Commands::Run {
            pipeline_dir,
            report_file,
            no_persist,
            ..
        } => {
            let resolved_dir = resolve_pipeline_dir(pipeline_dir, &cli_workspace_root());
            let artifacts = run_report::resolve_run_artifacts(
                &resolved_dir,
                report_file.as_deref(),
                *no_persist,
                &run_report::run_artifact_timestamp(e2e::unix_now()),
            );
            if let Err(e) = artifacts.prepare() {
                eprintln!("run 成果物ディレクトリの事前作成に失敗 (永続化せず継続): {e}");
                RunArtifacts::default()
            } else {
                artifacts
            }
        }
        Commands::Routine {
            routine_path,
            dry_run: false,
            report_file,
            no_persist,
            ..
        } => {
            let artifacts = run_report::resolve_routine_artifacts(
                routine_artifacts_base_dir(routine_path),
                report_file.as_deref(),
                *no_persist,
                &run_report::run_artifact_timestamp(e2e::unix_now()),
            );
            if let Err(e) = artifacts.prepare() {
                eprintln!("routine 成果物ディレクトリの事前作成に失敗 (永続化せず継続): {e}");
                RunArtifacts::default()
            } else {
                artifacts
            }
        }
        _ => RunArtifacts::default(),
    };

    init_tracing(command_artifacts.log_path.as_deref());

    match cli.command {
        Commands::Run {
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
            recover_grace_secs,
            max_recovery_attempts,
            verify_after_fire,
            jitter,
            jitter_seed,
            goal,
            goal_file,
            evidence_run_id,
            ..
        } => {
            let evidence_ctx = evidence_run_id.as_deref().and_then(|rid| {
                match e2e::e2e_run_dir(&cli_workspace_root().join(".omc").join("logs"), rid) {
                    Some(dir) => {
                        info!("UC-2 E2E 証跡採取有効: {}", dir.display());
                        Some((rid.to_string(), dir))
                    }
                    None => {
                        warn!(
                            "evidence-run-id が無効(空/パス区切り/.. 含む)のため証跡を採取しない: {rid:?}"
                        );
                        None
                    }
                }
            });
            if let Some((rid, dir)) = &evidence_ctx {
                let command_line =
                    format!("anaden run {pipeline_dir:?} {start_task} (max_iters={max_iters})");
                let _ = e2e::append_evidence_line(
                    dir,
                    &format!("ANADEN_E2E runId={rid} command={command_line}"),
                );
            }
            let pipeline_dir = resolve_pipeline_dir(&pipeline_dir, &cli_workspace_root());
            let parsed_goal = resolve_goal(goal.as_deref(), goal_file.as_deref(), &pipeline_dir)
                .map_err(|e| anyhow::anyhow!("ゴール指定の解決失敗: {e}"))?;
            let cancel_token = CancellationToken::new();
            let jitter_on = jitter || jitter_seed.is_some();
            let resolved_jitter_seed = jitter_on.then(|| {
                jitter_seed.unwrap_or_else(|| {
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_nanos() as u64)
                        .unwrap_or(0)
                })
            });
            let reason = run_pipeline_live(
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
                recover_grace_secs,
                max_recovery_attempts,
                verify_after_fire,
                resolved_jitter_seed,
                parsed_goal,
                cancel_token,
                evidence_ctx.as_ref().map(|(_, d)| d.as_path()),
                &command_artifacts,
            )
            .await?;
            if let Some((rid, dir)) = &evidence_ctx {
                let shots = e2e::count_screenshots(dir);
                let tree = tree_hash_of_pipeline(&pipeline_dir);
                let meta = e2e::write_metadata(
                    dir,
                    rid,
                    &e2e::iso8601_utc(e2e::unix_now()),
                    &format!("anaden run {:?}", pipeline_dir.display()),
                    &tree,
                    shots,
                );
                match meta {
                    Ok(p) => info!(
                        "UC-2 E2E メタデータ永続化: {} (screenshots={shots})",
                        p.display()
                    ),
                    Err(e) => warn!("UC-2 E2E メタデータ書込失敗: {e}"),
                }
            }
            let code = anaden_cli_contract::run_exit_code(&reason);
            info!(
                "run 終了コード: {code} (停止理由: {})",
                anaden_engine::stop_reason_label(reason.clone())
            );
            std::process::exit(code);
        }
        Commands::EnsureOpen {
            wait_secs,
            readiness,
        } => {
            exit_standalone(run_ensure_open_or_launch(wait_secs, false, readiness).await);
        }
        Commands::Launch {
            wait_secs,
            readiness,
        } => {
            exit_standalone(run_ensure_open_or_launch(wait_secs, true, readiness).await);
        }
        Commands::Routine {
            routine_path,
            dry_run,
            evidence_run_id,
            report_file,
            ..
        } => {
            let code = routine::run_routine_command(
                &routine_path,
                dry_run,
                evidence_run_id.as_deref(),
                report_file.as_deref(),
                command_artifacts,
            )
            .await?;
            std::process::exit(code);
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn routine_artifacts_base_dir_parent_or_dot() {
        assert_eq!(
            routine_artifacts_base_dir(Path::new("templates/routines/daily.toml")),
            Path::new("templates/routines")
        );
        assert_eq!(
            routine_artifacts_base_dir(Path::new("daily.toml")),
            Path::new(".")
        );
    }

    #[test]
    fn resolve_pipeline_dir_prefers_existing_cwd_path() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("templates").join("pipelines").join("x");
        std::fs::create_dir_all(&input).unwrap();
        assert_eq!(resolve_pipeline_dir(&input, dir.path()), input);
    }

    #[test]
    fn resolve_pipeline_dir_falls_back_to_workspace_root_relative() {
        let root = tempfile::tempdir().unwrap();
        let dir = root
            .path()
            .join("templates")
            .join("pipelines")
            .join("field_loop_pc");
        std::fs::create_dir_all(&dir).unwrap();
        let input = PathBuf::from("templates/pipelines/field_loop_pc");
        assert_eq!(resolve_pipeline_dir(&input, root.path()), dir);
    }

    #[test]
    fn resolve_pipeline_dir_resolves_bare_pipeline_name() {
        let root = tempfile::tempdir().unwrap();
        let dir = root
            .path()
            .join("templates")
            .join("pipelines")
            .join("field_loop_pc");
        std::fs::create_dir_all(&dir).unwrap();
        let input = PathBuf::from("field_loop_pc");
        assert_eq!(resolve_pipeline_dir(&input, root.path()), dir);
    }

    #[test]
    fn resolve_pipeline_dir_nonexistent_returns_input_as_is() {
        let root = tempfile::tempdir().unwrap();
        let input = PathBuf::from("templates/pipelines/ghost");
        assert_eq!(resolve_pipeline_dir(&input, root.path()), input);
    }

    #[test]
    fn resolve_pipeline_dir_supports_all_real_pipelines_on_real_root() {
        let root = cli_workspace_root();
        for id in ["field_loop_pc", "nav_to_field_pc", "fishing", "login"] {
            let rel = PathBuf::from(format!("templates/pipelines/{id}"));
            let got = resolve_pipeline_dir(&rel, &root);
            assert!(
                got.is_dir(),
                "{id}: not resolved to existing dir: {}",
                got.display()
            );
            let bare = resolve_pipeline_dir(&PathBuf::from(id), &root);
            assert!(
                bare.is_dir(),
                "{id}: bare name not resolved: {}",
                bare.display()
            );
        }
    }

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
        let variants = [
            anaden_device::EnsureOutcome::AlreadyOpen,
            anaden_device::EnsureOutcome::Launched,
            anaden_device::EnsureOutcome::Timeout,
        ];
        for v in &variants {
            let label = ensure_outcome_label(v);
            assert!(!label.is_empty(), "variant {:?} produced empty label", v);
        }
    }

    #[test]
    fn exit_code_already_open_is_zero() {
        assert_eq!(
            anaden_cli_contract::ensure_open_exit_code(&anaden_device::EnsureOutcome::AlreadyOpen),
            0
        );
    }

    #[test]
    fn exit_code_launched_is_zero() {
        assert_eq!(
            anaden_cli_contract::ensure_open_exit_code(&anaden_device::EnsureOutcome::Launched),
            0
        );
    }

    #[test]
    fn exit_code_timeout_is_non_zero_and_distinct_from_hard_error() {
        let code =
            anaden_cli_contract::ensure_open_exit_code(&anaden_device::EnsureOutcome::Timeout);
        assert_ne!(code, 0, "Timeout must be non-zero for CI gate");
        assert_eq!(
            code,
            anaden_cli_contract::EXIT_TIMEOUT,
            "Timeout must map to EXIT_TIMEOUT (2)"
        );
        assert_ne!(
            code, 1,
            "Timeout must be distinct from hard-error exit code 1"
        );
    }

    #[test]
    fn exit_code_success_variants_share_zero() {
        assert_eq!(
            anaden_cli_contract::ensure_open_exit_code(&anaden_device::EnsureOutcome::AlreadyOpen),
            anaden_cli_contract::ensure_open_exit_code(&anaden_device::EnsureOutcome::Launched)
        );
    }

    #[test]
    fn version_flag_renders_crate_version() {
        use clap::CommandFactory;
        let rendered = Cli::command().render_version();
        assert!(
            rendered.contains(concat!("anaden ", env!("CARGO_PKG_VERSION"))),
            "--version must render `anaden <crate version>`, got: {rendered}"
        );
    }

    #[test]
    fn version_flag_matches_cargo_pkg_version() {
        use clap::CommandFactory;
        let cmd = Cli::command();
        let v = cmd.get_version().expect("version must be set on Cli");
        assert_eq!(v, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn parse_goal_flag_both_none_returns_none() {
        let parsed = parse_goal_flag(None, None).expect("both-none must be Ok(None)");
        assert!(parsed.is_none(), "no flag → no goal (backcompat)");
    }

    #[test]
    fn parse_goal_flag_inline_goal_parses() {
        let inline = r#"
        name = "farm50"
        [stop]
        LoopCount = { target = 50 }
        "#;
        let parsed = parse_goal_flag(Some(inline), None).expect("inline must parse");
        let goal = parsed.expect("Some(goal) when --goal provided");
        assert_eq!(goal.name, "farm50");
        assert_eq!(
            goal.stop,
            anaden_core::StopCondition::LoopCount { target: 50 }
        );
    }

    #[test]
    fn parse_goal_flag_inline_goal_validates() {
        let inline = r#"
        name = "bad"
        [stop]
        LoopCount = { target = 0 }
        "#;
        let err = parse_goal_flag(Some(inline), None).expect_err("target=0 must fail validation");
        assert!(
            format!("{err}").contains("target"),
            "error should reference the invalid field"
        );
    }

    #[test]
    fn parse_goal_flag_inline_invalid_toml_is_err() {
        let err = parse_goal_flag(Some("not = valid = toml"), None).expect_err("bad toml");
        assert!(!format!("{err}").is_empty());
    }

    #[test]
    fn parse_goal_flag_inline_unknown_field_rejected() {
        let inline = r#"
        name = "g"
        bogus = true
        [stop]
        LoopCount = { target = 1 }
        "#;
        assert!(parse_goal_flag(Some(inline), None).is_err());
    }

    #[test]
    fn parse_goal_flag_file_reads_and_parses() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("goal.toml");
        std::fs::write(
            &path,
            r#"
            name = "find_clear"
            [stop.TemplateMatch]
            task = "clear"
            confidence = 0.85
            "#,
        )
        .expect("write goal file");

        let parsed = parse_goal_flag(None, Some(path.as_path())).expect("file must read+parse");
        let goal = parsed.expect("Some(goal) when --goal-file provided");
        assert_eq!(
            goal.stop,
            anaden_core::StopCondition::TemplateMatch {
                task: "clear".to_string(),
                confidence: 0.85,
            }
        );
    }

    #[test]
    fn parse_goal_flag_file_missing_is_err() {
        let err = parse_goal_flag(
            None,
            Some(std::path::Path::new("/nonexistent/does/not/exist.toml")),
        )
        .expect_err("missing file must error");
        assert!(
            format!("{err}").contains("--goal-file"),
            "error should mention --goal-file, got: {err}"
        );
    }

    #[test]
    fn parse_goal_flag_both_set_is_err() {
        let inline = r#"
        name = "g"
        [stop]
        LoopCount = { target = 1 }
        "#;
        let err = parse_goal_flag(Some(inline), Some(std::path::Path::new("whatever.toml")))
            .expect_err("both flags set must error");
        assert!(
            format!("{err}").contains("exclusive"),
            "error should explain mutual exclusivity, got: {err}"
        );
    }

    fn manifest_goal_dir(goals_toml: &str) -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("pipeline.toml"),
            format!("start_task = \"SomeTaskPc\"\n{goals_toml}"),
        )
        .unwrap();
        tmp
    }

    #[test]
    fn resolve_goal_manifest_single_goal_used_when_flags_absent() {
        let dir = manifest_goal_dir(
            "[[goal]]\nname = \"loop3\"\n[goal.stop]\nLoopCount = { target = 3 }\n",
        );
        let g = resolve_goal(None, None, dir.path())
            .expect("resolve")
            .unwrap();
        assert_eq!(g.name, "loop3");
        assert!(matches!(
            g.stop,
            anaden_core::StopCondition::LoopCount { target: 3 }
        ));
    }

    #[test]
    fn resolve_goal_manifest_empty_goals_returns_none() {
        let dir = manifest_goal_dir("");
        assert!(
            resolve_goal(None, None, dir.path())
                .expect("resolve")
                .is_none()
        );
    }

    #[test]
    fn resolve_goal_manifest_missing_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(
            resolve_goal(None, None, tmp.path())
                .expect("resolve")
                .is_none()
        );
    }

    #[test]
    fn resolve_goal_explicit_flag_overrides_manifest() {
        let dir = manifest_goal_dir(
            "[[goal]]\nname = \"manifest_goal\"\n[goal.stop]\nLoopCount = { target = 99 }\n",
        );
        let g = resolve_goal(
            Some("name = \"flag_goal\"\n[stop]\nLoopCount = { target = 1 }\n"),
            None,
            dir.path(),
        )
        .expect("resolve")
        .unwrap();
        assert_eq!(g.name, "flag_goal", "明示 --goal が manifest に勝つ");
    }

    #[test]
    fn resolve_goal_broken_manifest_fails_closed() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("pipeline.toml"), "not valid toml =").unwrap();
        let err = resolve_goal(None, None, tmp.path()).expect_err("broken manifest must error");
        assert!(
            format!("{err}").contains("pipeline.toml"),
            "error should mention pipeline.toml, got: {err}"
        );
    }

    #[test]
    fn resolve_goal_multiple_goals_composed_as_any() {
        let dir = manifest_goal_dir(
            "[[goal]]\nname = \"find\"\n[goal.stop]\nLoopCount = { target = 5 }\n\
             [[goal]]\nname = \"cap\"\n[goal.stop]\nTimeout = { secs = 600 }\n",
        );
        let g = resolve_goal(None, None, dir.path())
            .expect("resolve")
            .unwrap();
        assert_eq!(g.name, "any(find, cap)");
        match g.stop {
            anaden_core::StopCondition::Any { conditions } => {
                assert_eq!(conditions.len(), 2);
                assert!(matches!(
                    conditions[0],
                    anaden_core::StopCondition::LoopCount { target: 5 }
                ));
                assert!(matches!(
                    conditions[1],
                    anaden_core::StopCondition::Timeout { secs: 600 }
                ));
            }
            other => panic!("Any 合成が期待されるが {other:?}"),
        }
    }

    #[test]
    fn resolve_goal_repo_login_manifest_resolves() {
        let root = cli_workspace_root();
        let dir = root.join("templates").join("pipelines").join("login");
        let g = resolve_goal(None, None, &dir).expect("resolve").unwrap();
        assert_eq!(g.name, "login_field_reached");
        assert!(matches!(g.stop, anaden_core::StopCondition::Any { .. }));
    }

    mod fake_backend {
        use anaden_core::InputAction;
        use anaden_device::DeviceError;
        use anaden_engine::{Capture, Input};
        use async_trait::async_trait;

        pub(super) struct FailCapture;

        #[async_trait]
        impl Capture for FailCapture {
            async fn capture(&self) -> Result<image::DynamicImage, DeviceError> {
                Err(DeviceError::CommandFailed {
                    message: "fake capture failure (test)".into(),
                })
            }
        }

        pub(super) struct NoopInput;

        #[async_trait]
        impl Input for NoopInput {
            async fn execute(&self, _action: &InputAction) -> Result<(), DeviceError> {
                Ok(())
            }
        }
    }

    #[tokio::test]
    async fn run_driver_writes_report_file_with_stop_reason() {
        let dir = tempfile::tempdir().unwrap();
        let pipeline_dir = dir.path().join("pl");
        std::fs::create_dir_all(&pipeline_dir).unwrap();
        let artifacts =
            run_report::resolve_run_artifacts(&pipeline_dir, None, false, "20260101-000000");
        artifacts.prepare().unwrap();

        let driver = anaden_engine::PipelineDriver::new(
            fake_backend::FailCapture,
            fake_backend::NoopInput,
            anaden_engine::PipelineState::new("t"),
            vec![],
            1280,
            300,
        );
        run_driver(driver, Duration::ZERO, 3, 1, None, None, &artifacts)
            .await
            .unwrap();

        let base = pipeline_dir.join(".anaden-run").join("20260101-000000");
        assert!(base.join("run.log").is_file(), "run.log missing");
        let file = std::fs::File::open(base.join("report.json")).unwrap();
        let report: anaden_engine::ProgressReport = serde_json::from_reader(file).unwrap();
        assert_eq!(
            report.stop_reason,
            Some(anaden_engine::LoopStopReason::CaptureError)
        );
        assert_eq!(report.iterations, 0);
        assert_eq!(report.fired_count, 0);
    }

    #[tokio::test]
    async fn run_driver_no_persist_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let pipeline_dir = dir.path().join("pl");
        std::fs::create_dir_all(&pipeline_dir).unwrap();
        let artifacts = run_report::resolve_run_artifacts(&pipeline_dir, None, true, "ts");

        let driver = anaden_engine::PipelineDriver::new(
            fake_backend::FailCapture,
            fake_backend::NoopInput,
            anaden_engine::PipelineState::new("t"),
            vec![],
            1280,
            300,
        );
        run_driver(driver, Duration::ZERO, 3, 1, None, None, &artifacts)
            .await
            .unwrap();

        assert!(
            !pipeline_dir.join(".anaden-run").exists(),
            "no-persist must not create artifacts dir"
        );
    }

    #[test]
    fn resolve_watch_tasks_without_manifest_returns_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let got = resolve_watch_tasks(tmp.path(), &[]).expect("resolve");
        assert!(got.is_empty());
    }

    #[test]
    fn resolve_watch_tasks_without_watch_key_returns_empty() {
        let dir = manifest_goal_dir("");
        let got = resolve_watch_tasks(dir.path(), &[]).expect("resolve");
        assert!(got.is_empty());
    }

    #[test]
    fn resolve_watch_tasks_unknown_name_errors() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("pipeline.toml"),
            "start_task = \"SomeTaskPc\"\nwatch = [\"ghost_popup\"]\n",
        )
        .unwrap();
        let err = resolve_watch_tasks(tmp.path(), &[]).expect_err("unknown watch must fail");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("ghost_popup"),
            "error must name the missing task: {msg}"
        );
    }

    #[test]
    fn resolve_watch_tasks_broken_manifest_errors() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("pipeline.toml"),
            "totally_not_valid_toml = [",
        )
        .unwrap();
        assert!(resolve_watch_tasks(tmp.path(), &[]).is_err());
    }
}
