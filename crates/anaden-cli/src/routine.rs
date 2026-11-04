use std::path::{Path, PathBuf};

use async_trait::async_trait;
use tracing::{info, warn};

use anaden_engine::{
    LoopOutcome, PipelineInvoker, RoutineDef, RoutineError, RoutineStep, RoutineSummary,
    resolve_pipeline_dir, run_routine,
};

pub const ROUTINE_SUMMARY_FILE: &str = "routine-summary.txt";
pub const ROUTINE_META_FILE: &str = "routine-metadata.json";
const RECOVER_NOMATCH_THRESHOLD: u32 = 5;
const RECOVERY_BOOT_GRACE_SECS: u64 = 180;

pub(crate) async fn run_routine_command(
    routine_path: &Path,
    dry_run: bool,
    evidence_run_id: Option<&str>,
    report_file: Option<&Path>,
    artifacts: crate::run_report::RunArtifacts,
) -> anyhow::Result<i32> {
    let root = crate::cli_workspace_root();
    let def = load_and_validate(routine_path)?;
    info!(
        "routine '{}' 読込+検証 OK: {} ステップ",
        def.name,
        def.steps.len()
    );

    if dry_run {
        print!("{}", anaden_engine::format_dry_run(&def, &root));
        println!("dry-run 完了 (検証のみ・実行なし)");
        return Ok(0);
    }

    let evidence_dir = evidence_run_id.and_then(|rid| {
        match crate::e2e::e2e_run_dir(&root.join(".omc").join("logs"), rid) {
            Some(dir) => {
                info!("routine evidence 採取有効: {}", dir.display());
                Some(dir)
            }
            None => {
                warn!("evidence-run-id が無効のため証跡を採取しない: {rid:?}");
                None
            }
        }
    });

    ensure_game_open().await;

    let invoker = Win32RoutineInvoker::new(root.clone());
    let summary = tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            warn!("Ctrl+C 受信: routine を中止します");
            anaden_engine::interrupted_summary(&def)
        }
        s = run_routine(&def, &invoker) => s,
    };
    print!("{}", summary.format_text());

    let command = command_line(routine_path, dry_run, report_file);
    let exit_code = summary_exit_code(&summary);
    if let Some(dir) = &evidence_dir {
        match write_routine_evidence(dir, &def.name, &command, &summary, exit_code) {
            Ok((summary_path, meta_path)) => info!(
                "routine evidence 永続化: {} / {}",
                summary_path.display(),
                meta_path.display()
            ),
            Err(e) => warn!("routine evidence 書込失敗: {e}"),
        }
    }
    if let Some(path) = artifacts.report_path.as_ref() {
        write_routine_report(path, &summary)?;
        info!("routine レポート永続化: {}", path.display());
    }
    Ok(exit_code)
}

async fn ensure_game_open() {
    match crate::ensure_open_outcome(std::time::Duration::from_secs(30)).await {
        Ok(outcome) => info!(
            "起動保証: {}",
            anaden_cli_contract::ensure_outcome_label(&outcome)
        ),
        Err(e) => warn!("起動保証に失敗したが routine を続行します: {e}"),
    }
}

#[must_use]
pub(crate) fn command_line(
    routine_path: &Path,
    dry_run: bool,
    report_file: Option<&Path>,
) -> String {
    let mut out = format!(
        "anaden routine {}{}",
        routine_path.display(),
        if dry_run { " --dry-run" } else { "" }
    );
    if let Some(p) = report_file {
        out.push_str(&format!(" --report-file {}", p.display()));
    }
    out
}

#[must_use]
pub(crate) fn summary_exit_code(summary: &RoutineSummary) -> i32 {
    if summary.all_ok() { 0 } else { 2 }
}

pub(crate) fn write_routine_evidence(
    dir: &Path,
    routine_name: &str,
    command: &str,
    summary: &RoutineSummary,
    exit_code: i32,
) -> std::io::Result<(PathBuf, PathBuf)> {
    std::fs::create_dir_all(dir)?;
    let summary_text = format!(
        "{command}\nroutine={routine_name}\n{}",
        summary.format_text()
    );
    let summary_path = dir.join(ROUTINE_SUMMARY_FILE);
    std::fs::write(&summary_path, &summary_text)?;

    let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    let meta = format!(
        "{{\n  \"runId\": \"{}\",\n  \"runTimestamp\": \"{}\",\n  \"command\": \"{}\",\n  \"routine\": \"{}\",\n  \"steps\": {},\n  \"completed\": {},\n  \"failed\": {},\n  \"skipped\": {},\n  \"aborted\": {},\n  \"totalIterations\": {},\n  \"totalFired\": {},\n  \"exitCode\": {},\n  \"recordedAtUnix\": {}\n}}\n",
        esc(&run_id_of(dir)),
        crate::e2e::iso8601_utc(crate::e2e::unix_now()),
        esc(command),
        esc(routine_name),
        summary.results.len(),
        summary.completed,
        summary.failed,
        summary.skipped,
        summary.aborted,
        summary.total_iterations,
        summary.total_fired,
        exit_code,
        crate::e2e::unix_now(),
    );
    let meta_path = dir.join(ROUTINE_META_FILE);
    std::fs::write(&meta_path, &meta)?;
    Ok((summary_path, meta_path))
}

fn run_id_of(dir: &Path) -> String {
    dir.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

pub(crate) fn write_routine_report(path: &Path, summary: &RoutineSummary) -> std::io::Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(summary)
        .map_err(|e| std::io::Error::other(format!("RoutineSummary の JSON 化に失敗: {e}")))?;
    std::fs::write(path, json)
}

pub(crate) struct Win32RoutineInvoker {
    root: PathBuf,
}

impl Win32RoutineInvoker {
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }
}

#[async_trait]
impl PipelineInvoker for Win32RoutineInvoker {
    async fn invoke(&self, step: &RoutineStep) -> Result<LoopOutcome, RoutineError> {
        println!(
            "=== routine step '{}' 開始 ({}) ===",
            step.name, step.pipeline_dir
        );
        let outcome = run_step_live(step, &self.root).await;
        match &outcome {
            Ok(o) => println!(
                "=== routine step '{}' 終了: iterations={} fired={} reason={:?} ===",
                step.name,
                o.iterations,
                o.fired_commands.len(),
                o.reason
            ),
            Err(e) => warn!("routine step '{}' 失敗: {e}", step.name),
        }
        outcome
    }
}

#[cfg(windows)]
async fn run_step_live(step: &RoutineStep, root: &Path) -> Result<LoopOutcome, RoutineError> {
    use anaden_engine::{PipelineDriver, PipelineState};

    let dir = resolve_pipeline_dir(Path::new(&step.pipeline_dir), root).ok_or_else(|| {
        RoutineError::InvocationFailed {
            step: step.name.clone(),
            reason: format!("pipeline_dir が解決できません: {}", step.pipeline_dir),
        }
    })?;
    let tasks = anaden_vision::load_pipeline(&dir).map_err(|e| RoutineError::InvocationFailed {
        step: step.name.clone(),
        reason: format!("pipeline 読込失敗 {}: {e}", dir.display()),
    })?;
    if tasks.is_empty() {
        return Err(RoutineError::InvocationFailed {
            step: step.name.clone(),
            reason: format!("pipeline が空です: {}", dir.display()),
        });
    }

    let capture = anaden_device::Win32Capture::default_process();
    let input = anaden_device::Win32InputExecutor::new(anaden_device::DEFAULT_PROCESS_NAME);
    let probe = capture
        .capture()
        .await
        .map_err(|e| RoutineError::InvocationFailed {
            step: step.name.clone(),
            reason: format!("device_width 実測のための初回キャプチャ失敗: {e}"),
        })?;
    let device_width = probe.width();
    info!(
        "routine step '{}': device_width 実測 {device_width}",
        step.name
    );

    let launcher = anaden_device::Win32Launch::default_paths();
    let recovery: Option<anaden_engine::RecoveryHook> =
        Some(crate::escalation_recovery_hook(launcher, "routine"));

    let mut driver = PipelineDriver::new(
        capture,
        input,
        PipelineState::new(&step.start_task),
        tasks,
        device_width,
        300,
    )
    .with_verify(true)
    .with_recovery_grace(std::time::Duration::from_secs(RECOVERY_BOOT_GRACE_SECS))
    .with_max_recovery_attempts(crate::DEFAULT_MAX_RECOVERY_ATTEMPTS);
    Ok(driver
        .run_loop_with_recovery(
            std::time::Duration::from_secs(step.interval),
            step.max_iters,
            RECOVER_NOMATCH_THRESHOLD,
            recovery,
        )
        .await)
}

#[cfg(not(windows))]
async fn run_step_live(step: &RoutineStep, _root: &Path) -> Result<LoopOutcome, RoutineError> {
    Err(RoutineError::InvocationFailed {
        step: step.name.clone(),
        reason:
            "このバイナリは Windows 向けではないため PC 版 (Win32) バックエンドを使用できません"
                .to_string(),
    })
}

pub(crate) fn load_and_validate(routine_path: &Path) -> anyhow::Result<RoutineDef> {
    let root = crate::cli_workspace_root();
    let def = anaden_engine::load_routine(routine_path)?;
    def.validate(&root)?;
    Ok(def)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use anaden_engine::{LoopStopReason, RoutineStepResult, StepStatus};

    fn summary_of(statuses: Vec<StepStatus>) -> RoutineSummary {
        let results: Vec<RoutineStepResult> = statuses
            .into_iter()
            .enumerate()
            .map(|(i, status)| RoutineStepResult {
                step: format!("s{i}"),
                pipeline_dir: format!("p{i}"),
                status,
                iterations: 1,
                fired_count: 0,
                terminal: "t".to_string(),
            })
            .collect();
        RoutineSummary {
            routine: "rt".to_string(),
            completed: results
                .iter()
                .filter(|r| matches!(r.status, StepStatus::Completed(_)))
                .count(),
            failed: results.iter().filter(|r| r.status.is_failure()).count(),
            skipped: results
                .iter()
                .filter(|r| r.status == StepStatus::Skipped)
                .count(),
            total_iterations: results.len() as u64,
            total_fired: 0,
            results,
            aborted: false,
        }
    }

    // ---- summary_exit_code (終了コード契約) ----

    #[test]
    fn exit_code_zero_when_all_completed() {
        let s = summary_of(vec![
            StepStatus::Completed(LoopStopReason::Stop),
            StepStatus::Completed(LoopStopReason::TerminalTask),
        ]);
        assert_eq!(summary_exit_code(&s), 0);
    }

    #[test]
    fn exit_code_two_when_any_failed() {
        let s = summary_of(vec![
            StepStatus::Completed(LoopStopReason::Stop),
            StepStatus::Failed(LoopStopReason::CaptureError),
        ]);
        assert_eq!(summary_exit_code(&s), 2);
    }

    #[test]
    fn exit_code_two_when_interrupted_or_skipped() {
        let mut interrupted = summary_of(vec![StepStatus::Interrupted]);
        interrupted.aborted = true;
        assert_eq!(summary_exit_code(&interrupted), 2);
        let skipped = summary_of(vec![
            StepStatus::Completed(LoopStopReason::Stop),
            StepStatus::Skipped,
        ]);
        assert_eq!(summary_exit_code(&skipped), 2);
    }

    // ---- command_line ----

    #[test]
    fn command_line_includes_path_and_dry_run_flag() {
        assert_eq!(
            command_line(Path::new("templates/routines/daily.toml"), false, None),
            "anaden routine templates/routines/daily.toml"
        );
        assert!(command_line(Path::new("a.toml"), true, None).ends_with("--dry-run"));
    }

    /// `--report-file` 指定時はコマンド全文にも残る (生コマンド出力規約)。
    #[test]
    fn command_line_includes_report_file_flag() {
        assert_eq!(
            command_line(
                Path::new("daily.toml"),
                false,
                Some(Path::new("reports/1.json"))
            ),
            "anaden routine daily.toml --report-file reports/1.json"
        );
    }

    // ---- write_routine_report (report.json 書込の機械可読契約) ----

    /// 書込んだ report.json は `serde_json::from_reader::<RoutineSummary>` で
    /// 読める (studio 消費契約)。親ディレクトリは自動作成。
    #[test]
    fn write_routine_report_roundtrips_routine_summary() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("report.json");
        let s = summary_of(vec![
            StepStatus::Completed(LoopStopReason::GoalReached),
            StepStatus::NoFire(LoopStopReason::MaxIterations),
        ]);
        write_routine_report(&path, &s).expect("write report");
        let file = std::fs::File::open(&path).unwrap();
        let back: RoutineSummary = serde_json::from_reader(file).unwrap();
        assert_eq!(back, s);
        assert_eq!(back.failed, 1, "no_fire step must survive roundtrip");
    }

    // ---- write_routine_evidence (永続化形式) ----

    #[test]
    fn evidence_files_contain_command_summary_and_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let s = summary_of(vec![
            StepStatus::Completed(LoopStopReason::Stop),
            StepStatus::Failed(LoopStopReason::ExecuteError),
        ]);
        let (summary_path, meta_path) = write_routine_evidence(
            dir.path().join("run-x").as_path(),
            "daily",
            "anaden routine daily.toml",
            &s,
            2,
        )
        .expect("evidence write");
        assert!(summary_path.is_file() && meta_path.is_file());

        let summary_txt = std::fs::read_to_string(&summary_path).unwrap();
        assert!(
            summary_txt.contains("anaden routine daily.toml"),
            "{summary_txt}"
        );
        assert!(summary_txt.contains("routine=daily"), "{summary_txt}");
        assert!(summary_txt.contains("[0] s0"), "{summary_txt}");
        assert!(summary_txt.contains("failed=1"), "{summary_txt}");

        let meta = std::fs::read_to_string(&meta_path).unwrap();
        for required in [
            "\"runId\": \"run-x\"",
            "\"command\": \"anaden routine daily.toml\"",
            "\"routine\": \"daily\"",
            "\"steps\": 2",
            "\"failed\": 1",
            "\"exitCode\": 2",
        ] {
            assert!(meta.contains(required), "missing {required} in:\n{meta}");
        }
    }

    // ---- dry-run 経路 (実機非依存・実 routine ファイル検証) ----

    fn workspace_root() -> PathBuf {
        crate::cli_workspace_root()
    }

    #[test]
    fn dry_run_validates_sample_daily_routine_on_real_root() {
        let path = workspace_root()
            .join("templates")
            .join("routines")
            .join("daily.toml");
        let def = load_and_validate(&path).expect("daily.toml must load+validate");
        assert_eq!(def.steps.len(), 2);
        assert_eq!(def.steps[0].start_task, "LoginTapTitlePc");
        assert_eq!(def.steps[1].start_task, "TapToStartPc");
    }

    #[test]
    fn dry_run_text_renders_resolved_real_pipelines() {
        let path = workspace_root()
            .join("templates")
            .join("routines")
            .join("daily.toml");
        let def = load_and_validate(&path).expect("load");
        let text = anaden_engine::format_dry_run(&def, &workspace_root());
        assert!(text.contains("routine 'daily'"), "{text}");
        assert!(
            text.contains("templates\\pipelines\\login")
                || text.contains("templates/pipelines/login"),
            "resolved login dir missing: {text}"
        );
        assert!(text.contains("start=LoginTapTitlePc"), "{text}");
        assert!(text.contains("start=TapToStartPc"), "{text}");
    }

    /// 不正 routine (不明 start_task) は dry-run でも fail-closed に Err。
    #[test]
    fn dry_run_rejects_unknown_start_task_fail_closed() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("bad.toml");
        std::fs::write(
            &path,
            r#"
            name = "bad"
            [[steps]]
            name = "s"
            pipeline_dir = "templates/pipelines/login"
            start_task = "GhostTask"
            "#,
        )
        .unwrap();
        let err = load_and_validate(&path).expect_err("must fail");
        assert!(format!("{err}").contains("GhostTask"), "{err}");
    }

    /// `run_routine_command` の dry-run 経路が exit 0 で完了する (実機に触れない)。
    /// レポート書込先が解決されていても dry-run は実行しないため書かない。
    #[tokio::test]
    async fn run_routine_command_dry_run_exits_zero() {
        let path = workspace_root()
            .join("templates")
            .join("routines")
            .join("daily.toml");
        let dir = tempfile::tempdir().unwrap();
        let report = dir.path().join("report.json");
        let artifacts = crate::run_report::RunArtifacts {
            report_path: Some(report.clone()),
            log_path: None,
        };
        let code = run_routine_command(&path, true, None, Some(&report), artifacts)
            .await
            .expect("dry-run must succeed");
        assert_eq!(code, 0);
        assert!(!report.exists(), "dry-run must not write report.json");
    }

    /// 不正 routine の実行経路はハードエラー (Err) で抜ける。
    #[tokio::test]
    async fn run_routine_command_missing_file_is_err() {
        let result = run_routine_command(
            Path::new("C:/definitely/not/here/daily.toml"),
            true,
            None,
            None,
            crate::run_report::RunArtifacts::default(),
        )
        .await;
        assert!(result.is_err());
    }
}
