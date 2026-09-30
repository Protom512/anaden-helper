//! `anaden routine` サブコマンド (Issue #199): 複数 pipeline の連続実行。
//!
//! - 定義読込 + fail-closed 検証は [`anaden_engine::load_routine`] /
//!   [`RoutineDef::validate`] へ委譲 (engine が単一情報源)。
//! - `--dry-run` は検証 + ステップ表示のみ (デバイスに触れない)。
//! - 実行は [`anaden_engine::run_routine`] + [`Win32RoutineInvoker`]
//!   (PipelineDriver 駆動・[`PipelineInvoker`] 実装)。
//! - evidence は既存規約 (`.omc/logs/{run-id}/`) に準拠し
//!   `routine-summary.txt` + `routine-metadata.json` を永続化する。

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use tracing::{info, warn};

use anaden_engine::{
    LoopOutcome, PipelineInvoker, RoutineDef, RoutineError, RoutineStep, RoutineSummary,
    resolve_pipeline_dir, run_routine,
};

/// routine summary の evidence ファイル名。
pub const ROUTINE_SUMMARY_FILE: &str = "routine-summary.txt";
/// routine メタデータ JSON の evidence ファイル名。
pub const ROUTINE_META_FILE: &str = "routine-metadata.json";
/// NoMatch リカバリ発火の連続回数閾値 (`run` の既定と同一)。
const RECOVER_NOMATCH_THRESHOLD: u32 = 5;
/// リカバリ (ゲーム再起動) 後の起動猶予 (boot-wait) 秒数 (Issue #210)。
///
/// 再起動 → タイトル到達には分単位かかるため、この期間は NoMatch streak を
/// 数えない (再起動ストーム防止)。テンプレートがマッチすれば即座に通常動作へ復帰。
/// 実測 (issue210-verify2): grace 90s では タイトル到達 (~2分) に間に合わず
/// 起動中のゲームが再 kill され続けたため 180s へ拡大。
const RECOVERY_BOOT_GRACE_SECS: u64 = 180;
/// 再起動前の既存ゲームプロセス終了待ち上限 (Issue #210: 二重起動防止)。
const RECOVERY_EXIT_WAIT_SECS: u64 = 15;

/// `routine` サブコマンド本体。終了コードを返す (呼出元が exit する)。
///
/// 終了コード契約:
/// - 0: 全ステップが失敗なく実行完了 ([`RoutineSummary::all_ok`])
/// - 2: 失敗ステップあり / 中断 (on_failure=stop・割り込み・invoker エラー)。
///   MaxIterations 到達かつ fired=0 のステップ (no_fire) も失敗扱い (Issue #210)
/// - 1: routine 読込・検証失敗等のハードエラー (anyhow Err 経由)
///
/// # Errors
/// routine ファイル読込・検証失敗、evidence ディレクトリ作成失敗等の
/// ハードエラーを anyhow で返す (panic しない)。
pub(crate) async fn run_routine_command(
    routine_path: &Path,
    dry_run: bool,
    evidence_run_id: Option<&str>,
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

    // evidence 採取先 (無効 run-id は warn のみで継続 — `run` と同一契約)。
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

    // 起動保証は routine 冒頭で1回のみ (各ステップは起動保証を呼ばない)。
    ensure_game_open().await;

    let invoker = Win32RoutineInvoker::new(root.clone());
    // Ctrl+C 協調的キャンセル: シグナル受信で routine 全体を中止する
    // (`run` の CancellationToken 配線と同旨。future を drop して driver ループを
    // 中断するため、実行中ステップはその場で打ち切り・ゲーム側へ影響しない)。
    let summary = tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            warn!("Ctrl+C 受信: routine を中止します");
            anaden_engine::interrupted_summary(&def)
        }
        s = run_routine(&def, &invoker) => s,
    };
    print!("{}", summary.format_text());

    let command = command_line(routine_path, dry_run);
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
    Ok(exit_code)
}

/// ゲーム起動保証 (routine 冒頭 1 回)。失敗・タイムアウトは warn のみで続行
/// (`run` の soft-fail 契約と同一)。
async fn ensure_game_open() {
    match crate::ensure_open_outcome(std::time::Duration::from_secs(30)).await {
        Ok(outcome) => info!(
            "起動保証: {}",
            anaden_cli_contract::ensure_outcome_label(&outcome)
        ),
        Err(e) => warn!("起動保証に失敗したが routine を続行します: {e}"),
    }
}

/// evidence ログ冒頭に残すコマンド全文 (生コマンド出力規約)。
#[must_use]
pub(crate) fn command_line(routine_path: &Path, dry_run: bool) -> String {
    format!(
        "anaden routine {}{}",
        routine_path.display(),
        if dry_run { " --dry-run" } else { "" }
    )
}

/// サマリ → 終了コードの射影 (純関数・テスト対象)。
#[must_use]
pub(crate) fn summary_exit_code(summary: &RoutineSummary) -> i32 {
    if summary.all_ok() { 0 } else { 2 }
}

/// routine evidence を `.omc/logs/{run-id}/` へ永続化する。
///
/// - `routine-summary.txt`: コマンド全文 + routine 名 + ステップ別/合計サマリ
///   (生コマンド出力 + 機械可読な集計行)。
/// - `routine-metadata.json`: runId / runTimestamp / command / 集計 / exitCode /
///   recordedAtUnix (e2e メタデータと同じ手書き JSON 形式)。
///
/// # Errors
/// ファイル作成・書込の io エラーを返す (panic しない)。
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

/// evidence ディレクトリ名 (= run-id) を取り出す (メタデータの runId 用)。
fn run_id_of(dir: &Path) -> String {
    dir.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// 本番 invoker: 各ステップを PipelineDriver (Win32) でライブ実行する。
pub(crate) struct Win32RoutineInvoker {
    /// pipeline_dir 解決基準の workspace ルート。
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

/// 1ステップを Win32 バックエンドで実行する (`run_with_windows` の routine 版)。
///
/// `run` サブコマンドの経路 (`run_pipeline_live`) には引数フラグ (algorithm 上書き・
/// goal・verify 等) が直結しており契約保持のため再利用しない。本関数は routine
/// ステップの宣言 (pipeline_dir/start_task/interval/max_iters) のみから driver を
/// 構築する (起動保証は routine 冒頭で済み)。
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
    // device_width は初回 capture で実測 (PC 版は生サイズ 1258 想定・手動指定なし)。
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

    // NoMatch リカバリ (ゲーム再起動) — `run` の既定と同じ構成 + Issue #210 対策:
    // - エスカレーション型 recovery (Issue #210): 1 回目は起動のみ (launch_app =
    //   kill しない・起動中のゲームを殺さない)、2 回目以降は kill+再起動
    //   (restart_app = 終了待ち→spawn・ハングゲームの本気回復)。実測
    //   (issue210-verify2) で「毎回 kill」はタイトル到達前のゲームを殺し続ける
    //  ことが判明したため、まず起動を待ち、それでも NoMatch が続く時だけ
    //   kill する。カウントはステップ実行内で単調増加 (マッチで streak は
    //   リセットされるがカウントは保持 — ハングゲームの反復回復に備える)。
    // - with_recovery_grace: 再起動後の猶予は NoMatch streak を数えない
    //   (起動猶予・マッチすれば即復帰) → 再起動ストーム防止
    let launcher = anaden_device::Win32Launch::default_paths();
    let recovery: Option<anaden_engine::RecoveryHook> = {
        let launcher = launcher.clone();
        let recoveries = std::sync::atomic::AtomicU32::new(0);
        Some(Box::new(move |_streak| {
            let l = launcher.clone();
            let n = recoveries.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            Box::pin(async move {
                if n == 1 {
                    info!(
                        "NoMatch 継続(routine): ゲームを起動します (初回・kill なし・Issue #210)"
                    );
                    l.launch_app().await
                } else {
                    info!(
                        "NoMatch 継続(routine): ゲームを kill+再起動します ({n} 回目・Issue #210)"
                    );
                    l.restart_app(std::time::Duration::from_secs(RECOVERY_EXIT_WAIT_SECS))
                        .await
                }
            })
        }))
    };

    let mut driver = PipelineDriver::new(
        capture,
        input,
        PipelineState::new(&step.start_task),
        tasks,
        device_width,
        300,
    )
    // 誠実検証は `run` サブコマンドの既定 (true) と同一にする
    // (PR #201 lane2 C-1: driver 既定 false のままでは routine ステップだけ
    //  発火後のテンプレ残存検証が無効になる未文書の減衰だった)。
    .with_verify(true)
    .with_recovery_grace(std::time::Duration::from_secs(RECOVERY_BOOT_GRACE_SECS));
    Ok(driver
        .run_loop_with_recovery(
            std::time::Duration::from_secs(step.interval),
            step.max_iters,
            RECOVER_NOMATCH_THRESHOLD,
            recovery,
        )
        .await)
}

/// 非 Windows ビルド向けフォールバック (コンパイルエラー回避・実行は fail)。
#[cfg(not(windows))]
async fn run_step_live(step: &RoutineStep, _root: &Path) -> Result<LoopOutcome, RoutineError> {
    Err(RoutineError::InvocationFailed {
        step: step.name.clone(),
        reason:
            "このバイナリは Windows 向けではないため PC 版 (Win32) バックエンドを使用できません"
                .to_string(),
    })
}

/// routine 読込 + fail-closed 検証 (dry-run・実行経路共通の前段)。
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
            command_line(Path::new("templates/routines/daily.toml"), false),
            "anaden routine templates/routines/daily.toml"
        );
        assert!(command_line(Path::new("a.toml"), true).ends_with("--dry-run"));
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
    #[tokio::test]
    async fn run_routine_command_dry_run_exits_zero() {
        let path = workspace_root()
            .join("templates")
            .join("routines")
            .join("daily.toml");
        let code = run_routine_command(&path, true, None)
            .await
            .expect("dry-run must succeed");
        assert_eq!(code, 0);
    }

    /// 不正 routine の実行経路はハードエラー (Err) で抜ける。
    #[tokio::test]
    async fn run_routine_command_missing_file_is_err() {
        let result =
            run_routine_command(Path::new("C:/definitely/not/here/daily.toml"), true, None).await;
        assert!(result.is_err());
    }
}
