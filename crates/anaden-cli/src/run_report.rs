//! run サブコマンドの成果物永続化 (03-report-channel)。
//!
//! F7 解消: `ProgressReport` は Serialize 付きなのに run の成果サマリが
//! どこにも永続化されていなかった。本モジュールは機械可読レポート
//! (report.json) の書込先解決・事前作成・書込の単一実装を担う。
//! studio はこのレポートを読むことで CLI の人間可読 stdout 行の文字列解析
//! (F11) から脱却する。
//!
//! loop 10: routine サブコマンドにも run と対称な既定永続化
//! (`.anaden-routine/<timestamp>/`) を提供する。resolver のみを本モジュールが
//! 担い、書込本体は `routine::write_routine_report` (RoutineSummary の JSON)。

use std::io;
use std::path::{Path, PathBuf};

use anaden_engine::LoopOutcome;

/// 既定の成果物ディレクトリ名 (pipeline_dir 直下)。
const DEFAULT_ARTIFACTS_DIR: &str = ".anaden-run";
/// レポートファイル名。
const REPORT_FILE_NAME: &str = "report.json";
/// tracing 二重出力 (run.log) のファイル名。
const RUN_LOG_FILE_NAME: &str = "run.log";

/// 1 回の run の成果物書込先。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RunArtifacts {
    /// report.json の書込先 (`None` = 永続化しない)。
    pub report_path: Option<PathBuf>,
    /// run.log (tracing ファイル出力) のパス (`None` = stdout のみ)。
    pub log_path: Option<PathBuf>,
}

impl RunArtifacts {
    /// 成果物ディレクトリと run.log を事前作成する (report は run 終了時に書込)。
    ///
    /// run.log を起動時に作成しておくことで、run 中に異常終了しても
    /// 「どのディレクトリで実行したか」が残る。永続化なし (`None` のみ) の
    /// 場合は何もしない。
    pub(crate) fn prepare(&self) -> io::Result<()> {
        if let Some(p) = &self.log_path {
            if let Some(parent) = p.parent()
                && !parent.as_os_str().is_empty()
            {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::File::create(p)?;
        }
        Ok(())
    }
}

/// routine 既定の成果物ディレクトリ名 (routine ファイルの置かれたディレクトリ直下)。
const DEFAULT_ROUTINE_ARTIFACTS_DIR: &str = ".anaden-routine";
/// routine レポートファイル名 (run の report.json と区別)。
const ROUTINE_REPORT_FILE_NAME: &str = "routine-report.json";
/// tracing 二重出力 (routine.log) のファイル名。
const ROUTINE_LOG_FILE_NAME: &str = "routine.log";

/// フラグ → 成果物パス解決の共通コア (run / routine で優先順位契約は同一)。
///
/// - `report_file = Some(p)`: 明示指定を最優先。`--no-persist` は既定永続化の
///   無効化フラグであり、明示指定には効かない (ユーザが明示的に要求した
///   ファイルは黙って捨てない)。
/// - 既定 (`report_file = None`, `no_persist = false`):
///   `<base_dir>/<default_dir>/<timestamp>/` へ report + log。
/// - `no_persist = true` (明示指定なし): 何も永続化しない。
fn resolve_artifacts_at(
    base_dir: &Path,
    report_file: Option<&Path>,
    no_persist: bool,
    timestamp: &str,
    default_dir: &str,
    report_name: &str,
    log_name: &str,
) -> RunArtifacts {
    match report_file {
        Some(p) => RunArtifacts {
            report_path: Some(p.to_path_buf()),
            log_path: None,
        },
        None if !no_persist => {
            let dir = base_dir.join(default_dir).join(timestamp);
            RunArtifacts {
                report_path: Some(dir.join(report_name)),
                log_path: Some(dir.join(log_name)),
            }
        }
        None => RunArtifacts::default(),
    }
}

/// フラグ → パスの解決 (run 用・純関数・テスト可能)。
///
/// 既定 (`report_file = None`, `no_persist = false`) は
/// `<pipeline_dir>/.anaden-run/<timestamp>/` へ report.json + run.log。
pub(crate) fn resolve_run_artifacts(
    pipeline_dir: &Path,
    report_file: Option<&Path>,
    no_persist: bool,
    timestamp: &str,
) -> RunArtifacts {
    resolve_artifacts_at(
        pipeline_dir,
        report_file,
        no_persist,
        timestamp,
        DEFAULT_ARTIFACTS_DIR,
        REPORT_FILE_NAME,
        RUN_LOG_FILE_NAME,
    )
}

/// フラグ → パスの解決 (routine 用・純関数・テスト可能 — loop 10)。
///
/// run と対称な既定永続化: 未指定時は
/// `<routine_dir>/.anaden-routine/<timestamp>/` へ routine-report.json +
/// routine.log。優先順位契約 (明示最優先 / `--no-persist`) は run と同一。
pub(crate) fn resolve_routine_artifacts(
    routine_dir: &Path,
    report_file: Option<&Path>,
    no_persist: bool,
    timestamp: &str,
) -> RunArtifacts {
    resolve_artifacts_at(
        routine_dir,
        report_file,
        no_persist,
        timestamp,
        DEFAULT_ROUTINE_ARTIFACTS_DIR,
        ROUTINE_REPORT_FILE_NAME,
        ROUTINE_LOG_FILE_NAME,
    )
}

/// run 終了時のレポート書込 (fail-closed: 失敗は呼出元へ伝播し exit 非ゼロ)。
///
/// 中身は `LoopOutcome::progress_report` (`ProgressReport`) の JSON。
/// `LoopOutcome` 全体ではなく report 単体とするのは、studio 消費の契約を
/// 最小にするため (停止理由は `stop_reason` フィールドで読める)。
pub(crate) fn write_report(path: &Path, outcome: &LoopOutcome) -> io::Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(&outcome.progress_report)
        .map_err(|e| io::Error::other(format!("ProgressReport の JSON 化に失敗: {e}")))?;
    std::fs::write(path, json)
}

/// Unix 秒 → ファイル名安全なタイムスタンプ (`YYYYMMDD-HHMMSS`, UTC)。
///
/// `e2e::date_parts_unix` (civil-from-days) と同一の変換源。ISO 8601 と違い
/// コロンを含まないため Windows パス部品として安全。
pub(crate) fn run_artifact_timestamp(unix_secs: u64) -> String {
    let (y, m, d, hh, mm, ss) = crate::e2e::date_parts_unix(unix_secs);
    format!("{y:04}{m:02}{d:02}-{hh:02}{mm:02}{ss:02}")
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use anaden_engine::{LoopOutcome, LoopStopReason, ProgressReport};

    fn outcome_fixture() -> LoopOutcome {
        LoopOutcome {
            iterations: 7,
            fired_commands: vec![],
            terminal: "max_iterations".to_string(),
            reason: LoopStopReason::MaxIterations,
            progress_report: ProgressReport {
                iterations: 7,
                fired_count: 3,
                per_task_matches: vec![],
                elapsed_ms: 1200,
                terminal_task: Some("max_iterations".to_string()),
                reached_goal: None,
                stop_reason: Some(LoopStopReason::MaxIterations),
                recovery_attempts: 0,
                max_consecutive_recovery: 0,
            },
        }
    }

    // ---- resolve_run_artifacts (フラグ → パス解決の純契約) ----

    /// 明示 `--report-file` は最優先。run.log (tracing 二重出力) は既定
    /// 永続化専用のため作らない。
    #[test]
    fn resolve_explicit_report_file_wins() {
        let dir = tempfile::tempdir().unwrap();
        let explicit = dir.path().join("custom-report.json");
        let got = resolve_run_artifacts(dir.path(), Some(&explicit), false, "20260101-000000");
        assert_eq!(got.report_path, Some(explicit));
        assert_eq!(got.log_path, None);
    }

    /// `--no-persist` は既定永続化の無効化フラグであり、明示指定には効かない。
    #[test]
    fn resolve_explicit_report_file_survives_no_persist() {
        let dir = tempfile::tempdir().unwrap();
        let explicit = dir.path().join("custom-report.json");
        let got = resolve_run_artifacts(dir.path(), Some(&explicit), true, "ts");
        assert_eq!(got.report_path, Some(explicit));
    }

    /// 未指定時の既定: `<pipeline_dir>/.anaden-run/<timestamp>/` へ
    /// report.json + run.log。
    #[test]
    fn resolve_default_artifacts_under_pipeline_dir() {
        let dir = tempfile::tempdir().unwrap();
        let pipeline = dir.path().join("pl");
        std::fs::create_dir_all(&pipeline).unwrap();
        let got = resolve_run_artifacts(&pipeline, None, false, "20261008-093000");
        let base = pipeline.join(".anaden-run").join("20261008-093000");
        assert_eq!(got.report_path, Some(base.join("report.json")));
        assert_eq!(got.log_path, Some(base.join("run.log")));
    }

    /// `--no-persist` (明示指定なし) は何も永続化しない。
    #[test]
    fn resolve_no_persist_disables_all() {
        let dir = tempfile::tempdir().unwrap();
        let got = resolve_run_artifacts(dir.path(), None, true, "20261008-093000");
        assert_eq!(got.report_path, None);
        assert_eq!(got.log_path, None);
    }

    // ---- resolve_routine_artifacts (routine 既定永続化 — loop 10) ----

    /// 未指定時の既定: `<routine_dir>/.anaden-routine/<timestamp>/` へ
    /// routine-report.json + routine.log (run の .anaden-run と対称)。
    #[test]
    fn resolve_routine_default_artifacts_under_routine_dir() {
        let dir = tempfile::tempdir().unwrap();
        let routines = dir.path().join("templates").join("routines");
        std::fs::create_dir_all(&routines).unwrap();
        let got = resolve_routine_artifacts(&routines, None, false, "20261010-120000");
        let base = routines.join(".anaden-routine").join("20261010-120000");
        assert_eq!(got.report_path, Some(base.join("routine-report.json")));
        assert_eq!(got.log_path, Some(base.join("routine.log")));
    }

    /// 明示 `--report-file` は最優先。routine.log (tracing 二重出力) は
    /// 既定永続化専用のため作らない (run と同一契約)。
    #[test]
    fn resolve_routine_explicit_report_file_wins() {
        let dir = tempfile::tempdir().unwrap();
        let explicit = dir.path().join("custom-routine-report.json");
        let got = resolve_routine_artifacts(dir.path(), Some(&explicit), false, "ts");
        assert_eq!(got.report_path, Some(explicit));
        assert_eq!(got.log_path, None);
    }

    /// `--no-persist` は明示指定には効かない (run と同一契約)。
    #[test]
    fn resolve_routine_explicit_survives_no_persist() {
        let dir = tempfile::tempdir().unwrap();
        let explicit = dir.path().join("custom-routine-report.json");
        let got = resolve_routine_artifacts(dir.path(), Some(&explicit), true, "ts");
        assert_eq!(got.report_path, Some(explicit));
    }

    /// `--no-persist` (明示指定なし) は何も永続化しない。
    #[test]
    fn resolve_routine_no_persist_disables_all() {
        let dir = tempfile::tempdir().unwrap();
        let got = resolve_routine_artifacts(dir.path(), None, true, "20261010-120000");
        assert_eq!(got.report_path, None);
        assert_eq!(got.log_path, None);
    }

    /// prepare は routine 成果物ディレクトリと routine.log を事前作成する
    /// (report は routine 終了時)。異常終了時も「どこで実行したか」が残る。
    #[test]
    fn prepare_creates_routine_dir_and_log() {
        let dir = tempfile::tempdir().unwrap();
        let routines = dir.path().join("routines");
        std::fs::create_dir_all(&routines).unwrap();
        let got = resolve_routine_artifacts(&routines, None, false, "20261010-120000");
        got.prepare().expect("prepare");
        let log = got.log_path.clone().unwrap();
        assert!(log.is_file(), "routine.log should exist: {}", log.display());
        assert!(!log.parent().unwrap().join("routine-report.json").is_file());
    }

    // ---- prepare (run.log の事前作成) ----

    /// prepare はディレクトリと run.log を作成する (report は run 終了時)。
    /// run 中に異常終了しても「どのディレクトリで実行したか」が残る。
    #[test]
    fn prepare_creates_dirs_and_run_log() {
        let dir = tempfile::tempdir().unwrap();
        let pipeline = dir.path().join("pl");
        std::fs::create_dir_all(&pipeline).unwrap();
        let got = resolve_run_artifacts(&pipeline, None, false, "20261008-093000");
        got.prepare().expect("prepare");
        let log = got.log_path.clone().unwrap();
        assert!(log.is_file(), "run.log should exist: {}", log.display());
        assert!(!log.parent().unwrap().join("report.json").is_file());
    }

    /// 永続化なし (None) の prepare は何もしない (Ok)。
    #[test]
    fn prepare_noop_when_disabled() {
        let got = resolve_run_artifacts(Path::new("pl"), None, true, "ts");
        got.prepare().expect("prepare noop");
    }

    // ---- write_report (report.json 書込の機械可読契約) ----

    /// 書込んだ report.json は `serde_json::from_reader::<ProgressReport>` で
    /// 読め、停止理由・サイクル数・発火数が入っている (studio 消費契約)。
    #[test]
    fn write_report_roundtrips_progress_report() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("report.json");
        let outcome = outcome_fixture();
        write_report(&path, &outcome).expect("write report");
        let file = std::fs::File::open(&path).unwrap();
        let report: ProgressReport = serde_json::from_reader(file).unwrap();
        assert_eq!(report.stop_reason, Some(LoopStopReason::MaxIterations));
        assert_eq!(report.iterations, 7);
        assert_eq!(report.fired_count, 3);
    }

    // ---- run_artifact_timestamp ----

    /// 既知値: unix 秒 → ファイル名安全な YYYYMMDD-HHMMSS (UTC)。
    #[test]
    fn run_artifact_timestamp_known_values() {
        assert_eq!(run_artifact_timestamp(0), "19700101-000000");
        assert_eq!(run_artifact_timestamp(1_788_220_800), "20260901-000000");
        // コロンを含まない (Windows パス部品として安全)。
        assert!(!run_artifact_timestamp(3_600).contains(':'));
    }
}
