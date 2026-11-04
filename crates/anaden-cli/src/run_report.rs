use std::io;
use std::path::{Path, PathBuf};

use anaden_engine::LoopOutcome;

const DEFAULT_ARTIFACTS_DIR: &str = ".anaden-run";
const REPORT_FILE_NAME: &str = "report.json";
const RUN_LOG_FILE_NAME: &str = "run.log";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RunArtifacts {
    pub report_path: Option<PathBuf>,
    pub log_path: Option<PathBuf>,
}

impl RunArtifacts {
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

const DEFAULT_ROUTINE_ARTIFACTS_DIR: &str = ".anaden-routine";
const ROUTINE_REPORT_FILE_NAME: &str = "routine-report.json";
const ROUTINE_LOG_FILE_NAME: &str = "routine.log";

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
                jitter_seed: None,
                verify: Default::default(),
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

    #[test]
    fn resolve_explicit_report_file_wins() {
        let dir = tempfile::tempdir().unwrap();
        let explicit = dir.path().join("custom-report.json");
        let got = resolve_run_artifacts(dir.path(), Some(&explicit), false, "20260101-000000");
        assert_eq!(got.report_path, Some(explicit));
        assert_eq!(got.log_path, None);
    }

    #[test]
    fn resolve_explicit_report_file_survives_no_persist() {
        let dir = tempfile::tempdir().unwrap();
        let explicit = dir.path().join("custom-report.json");
        let got = resolve_run_artifacts(dir.path(), Some(&explicit), true, "ts");
        assert_eq!(got.report_path, Some(explicit));
    }

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

    #[test]
    fn resolve_no_persist_disables_all() {
        let dir = tempfile::tempdir().unwrap();
        let got = resolve_run_artifacts(dir.path(), None, true, "20261008-093000");
        assert_eq!(got.report_path, None);
        assert_eq!(got.log_path, None);
    }

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

    #[test]
    fn resolve_routine_explicit_report_file_wins() {
        let dir = tempfile::tempdir().unwrap();
        let explicit = dir.path().join("custom-routine-report.json");
        let got = resolve_routine_artifacts(dir.path(), Some(&explicit), false, "ts");
        assert_eq!(got.report_path, Some(explicit));
        assert_eq!(got.log_path, None);
    }

    #[test]
    fn resolve_routine_explicit_survives_no_persist() {
        let dir = tempfile::tempdir().unwrap();
        let explicit = dir.path().join("custom-routine-report.json");
        let got = resolve_routine_artifacts(dir.path(), Some(&explicit), true, "ts");
        assert_eq!(got.report_path, Some(explicit));
    }

    #[test]
    fn resolve_routine_no_persist_disables_all() {
        let dir = tempfile::tempdir().unwrap();
        let got = resolve_routine_artifacts(dir.path(), None, true, "20261010-120000");
        assert_eq!(got.report_path, None);
        assert_eq!(got.log_path, None);
    }

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

    #[test]
    fn prepare_noop_when_disabled() {
        let got = resolve_run_artifacts(Path::new("pl"), None, true, "ts");
        got.prepare().expect("prepare noop");
    }

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

    #[test]
    fn run_artifact_timestamp_known_values() {
        assert_eq!(run_artifact_timestamp(0), "19700101-000000");
        assert_eq!(run_artifact_timestamp(1_788_220_800), "20260901-000000");
        assert!(!run_artifact_timestamp(3_600).contains(':'));
    }
}
