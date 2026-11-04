use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const DEFAULT_STEP_MAX_ITERS: u64 = 100;
pub const DEFAULT_STEP_INTERVAL_SECS: u64 = 1;

#[derive(Debug, Clone, Error)]
pub enum RoutineError {
    #[error("routine ファイル読込失敗 {path}: {reason}")]
    ReadFailed { path: PathBuf, reason: String },
    #[error("routine TOML パース失敗 {path}: {reason}")]
    ParseFailed { path: PathBuf, reason: String },
    #[error("routine '{name}' の steps が空です: 少なくとも1ステップ必要です")]
    EmptySteps { name: String },
    #[error("ステップ '{step}' の pipeline_dir が見つかりません: {dir} (root={root})")]
    PipelineDirNotFound {
        step: String,
        dir: String,
        root: PathBuf,
    },
    #[error("ステップ '{step}' の pipeline 読込失敗 {dir}: {reason}")]
    PipelineLoadFailed {
        step: String,
        dir: PathBuf,
        reason: String,
    },
    #[error(
        "ステップ '{step}' の start_task '{start_task}' が pipeline {dir} に存在しません \
         (利用可能: {available})"
    )]
    StartTaskNotFound {
        step: String,
        start_task: String,
        dir: PathBuf,
        available: String,
    },
    #[error("ステップ '{step}' の {field} が不正です: {reason}")]
    BadParam {
        step: String,
        field: &'static str,
        reason: String,
    },
    #[error("routine 名が空です")]
    EmptyName,
    #[error("ステップ '{step}' の実行に失敗: {reason}")]
    InvocationFailed { step: String, reason: String },
    #[error("{index} 番目のステップ名が空です")]
    EmptyStepName { index: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum OnFailure {
    #[default]
    Stop,
    Skip,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoutineStep {
    pub name: String,
    pub pipeline_dir: String,
    pub start_task: String,
    #[serde(default = "default_max_iters")]
    pub max_iters: u64,
    #[serde(default = "default_interval")]
    pub interval: u64,
    #[serde(default)]
    pub on_failure: OnFailure,
}

fn default_max_iters() -> u64 {
    DEFAULT_STEP_MAX_ITERS
}

fn default_interval() -> u64 {
    DEFAULT_STEP_INTERVAL_SECS
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoutineDef {
    pub name: String,
    pub steps: Vec<RoutineStep>,
}

pub fn load_routine(path: &Path) -> Result<RoutineDef, RoutineError> {
    let content = std::fs::read_to_string(path).map_err(|e| RoutineError::ReadFailed {
        path: path.to_path_buf(),
        reason: e.to_string(),
    })?;
    toml::from_str(&content).map_err(|e| RoutineError::ParseFailed {
        path: path.to_path_buf(),
        reason: e.to_string(),
    })
}

impl RoutineDef {
    pub fn validate(&self, root: &Path) -> Result<(), RoutineError> {
        if self.name.trim().is_empty() {
            return Err(RoutineError::EmptyName);
        }
        if self.steps.is_empty() {
            return Err(RoutineError::EmptySteps {
                name: self.name.clone(),
            });
        }
        for (index, step) in self.steps.iter().enumerate() {
            if step.name.trim().is_empty() {
                return Err(RoutineError::EmptyStepName { index });
            }
            if step.interval == 0 {
                return Err(RoutineError::BadParam {
                    step: step.name.clone(),
                    field: "interval",
                    reason: "1 以上の秒数を指定してください".to_string(),
                });
            }
            if step.max_iters == 0 {
                return Err(RoutineError::BadParam {
                    step: step.name.clone(),
                    field: "max_iters",
                    reason: "1 以上を指定してください".to_string(),
                });
            }
            validate_step(step, root)?;
        }
        Ok(())
    }
}

fn validate_step(step: &RoutineStep, root: &Path) -> Result<(), RoutineError> {
    let dir = crate::pipeline_dir::resolve_pipeline_dir(Path::new(&step.pipeline_dir), root)
        .ok_or_else(|| RoutineError::PipelineDirNotFound {
            step: step.name.clone(),
            dir: step.pipeline_dir.clone(),
            root: root.to_path_buf(),
        })?;
    let tasks =
        anaden_vision::load_pipeline(&dir).map_err(|e| RoutineError::PipelineLoadFailed {
            step: step.name.clone(),
            reason: e.to_string(),
            dir: dir.clone(),
        })?;
    if tasks.is_empty() {
        return Err(RoutineError::PipelineLoadFailed {
            step: step.name.clone(),
            dir,
            reason: "pipeline ディレクトリに *.toml がありません".to_string(),
        });
    }
    if !tasks.iter().any(|t| t.name == step.start_task) {
        let mut names: Vec<&str> = tasks.iter().map(|t| t.name.as_str()).collect();
        names.sort_unstable();
        return Err(RoutineError::StartTaskNotFound {
            step: step.name.clone(),
            start_task: step.start_task.clone(),
            dir,
            available: names.join(", "),
        });
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    fn workspace_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
    }

    fn write_routine(dir: &Path, content: &str) -> PathBuf {
        let path = dir.join("routine.toml");
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn parse_valid_routine_applies_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_routine(
            tmp.path(),
            r#"
            name = "daily"
            [[steps]]
            name = "login"
            pipeline_dir = "templates/pipelines/login"
            start_task = "LoginTapTitlePc"
            "#,
        );
        let def = load_routine(&path).expect("parse");
        assert_eq!(def.name, "daily");
        assert_eq!(def.steps.len(), 1);
        let step = &def.steps[0];
        assert_eq!(step.max_iters, DEFAULT_STEP_MAX_ITERS);
        assert_eq!(step.interval, DEFAULT_STEP_INTERVAL_SECS);
        assert_eq!(step.on_failure, OnFailure::Stop);
    }

    #[test]
    fn parse_explicit_fields_override_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_routine(
            tmp.path(),
            r#"
            name = "explicit"
            [[steps]]
            name = "s1"
            pipeline_dir = "p"
            start_task = "T"
            max_iters = 42
            interval = 3
            on_failure = "skip"
            "#,
        );
        let def = load_routine(&path).expect("parse");
        assert_eq!(def.steps[0].max_iters, 42);
        assert_eq!(def.steps[0].interval, 3);
        assert_eq!(def.steps[0].on_failure, OnFailure::Skip);
    }

    #[test]
    fn parse_invalid_toml_is_parse_failed() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_routine(tmp.path(), "not = valid = toml");
        let err = load_routine(&path).expect_err("bad syntax");
        assert!(matches!(err, RoutineError::ParseFailed { .. }), "{err}");
    }

    #[test]
    fn parse_unknown_field_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_routine(
            tmp.path(),
            r#"
            name = "x"
            bogus = true
            [[steps]]
            name = "s"
            pipeline_dir = "p"
            start_task = "T"
            "#,
        );
        assert!(matches!(
            load_routine(&path),
            Err(RoutineError::ParseFailed { .. })
        ));
    }

    #[test]
    fn parse_bad_on_failure_value_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_routine(
            tmp.path(),
            r#"
            name = "x"
            [[steps]]
            name = "s"
            pipeline_dir = "p"
            start_task = "T"
            on_failure = "abort"
            "#,
        );
        assert!(matches!(
            load_routine(&path),
            Err(RoutineError::ParseFailed { .. })
        ));
    }

    #[test]
    fn load_missing_file_is_read_failed() {
        let err = load_routine(Path::new("C:/definitely/not/here/routine.toml"))
            .expect_err("missing file");
        assert!(matches!(err, RoutineError::ReadFailed { .. }), "{err}");
    }

    #[test]
    fn validate_empty_steps_is_error() {
        let def = RoutineDef {
            name: "empty".to_string(),
            steps: vec![],
        };
        let err = def.validate(&workspace_root()).expect_err("empty steps");
        assert!(matches!(err, RoutineError::EmptySteps { .. }), "{err}");
    }

    #[test]
    fn validate_empty_name_is_error() {
        let def = RoutineDef {
            name: "  ".to_string(),
            steps: vec![RoutineStep {
                name: "s".to_string(),
                pipeline_dir: "login".to_string(),
                start_task: "LoginTapTitlePc".to_string(),
                max_iters: 1,
                interval: 1,
                on_failure: OnFailure::Stop,
            }],
        };
        assert!(matches!(
            def.validate(&workspace_root()),
            Err(RoutineError::EmptyName)
        ));
    }

    #[test]
    fn validate_nonexistent_pipeline_dir_is_error() {
        let def = RoutineDef {
            name: "ghost".to_string(),
            steps: vec![RoutineStep {
                name: "s".to_string(),
                pipeline_dir: "templates/pipelines/ghost".to_string(),
                start_task: "T".to_string(),
                max_iters: 1,
                interval: 1,
                on_failure: OnFailure::Stop,
            }],
        };
        let err = def
            .validate(&workspace_root())
            .expect_err("missing pipeline dir");
        assert!(
            matches!(err, RoutineError::PipelineDirNotFound { .. }),
            "{err}"
        );
    }

    #[test]
    fn validate_unknown_start_task_lists_available_tasks() {
        let def = RoutineDef {
            name: "badtask".to_string(),
            steps: vec![RoutineStep {
                name: "s".to_string(),
                pipeline_dir: "templates/pipelines/login".to_string(),
                start_task: "NoSuchTask".to_string(),
                max_iters: 1,
                interval: 1,
                on_failure: OnFailure::Stop,
            }],
        };
        let err = def.validate(&workspace_root()).expect_err("unknown task");
        match err {
            RoutineError::StartTaskNotFound {
                available,
                start_task,
                ..
            } => {
                assert_eq!(start_task, "NoSuchTask");
                assert!(available.contains("LoginTapTitlePc"), "{available}");
            }
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn validate_zero_interval_and_max_iters_are_errors() {
        let mk = |interval: u64, max_iters: u64| RoutineDef {
            name: "p".to_string(),
            steps: vec![RoutineStep {
                name: "s".to_string(),
                pipeline_dir: "login".to_string(),
                start_task: "LoginTapTitlePc".to_string(),
                max_iters,
                interval,
                on_failure: OnFailure::Stop,
            }],
        };
        assert!(matches!(
            mk(0, 1).validate(&workspace_root()),
            Err(RoutineError::BadParam {
                field: "interval",
                ..
            })
        ));
        assert!(matches!(
            mk(1, 0).validate(&workspace_root()),
            Err(RoutineError::BadParam {
                field: "max_iters",
                ..
            })
        ));
    }

    #[test]
    fn validate_real_two_step_routine_on_real_root() {
        let def = RoutineDef {
            name: "daily".to_string(),
            steps: vec![
                RoutineStep {
                    name: "login".to_string(),
                    pipeline_dir: "templates/pipelines/login".to_string(),
                    start_task: "LoginTapTitlePc".to_string(),
                    max_iters: 150,
                    interval: 2,
                    on_failure: OnFailure::Stop,
                },
                RoutineStep {
                    name: "nav".to_string(),
                    pipeline_dir: "nav_to_field_pc".to_string(),
                    start_task: "TapToStartPc".to_string(),
                    max_iters: 60,
                    interval: 2,
                    on_failure: OnFailure::Skip,
                },
            ],
        };
        def.validate(&workspace_root())
            .expect("real routine validates");
    }

    #[test]
    fn sample_daily_toml_loads_and_validates() {
        let path = workspace_root()
            .join("templates")
            .join("routines")
            .join("daily.toml");
        let def = load_routine(&path).unwrap_or_else(|e| panic!("{e}"));
        def.validate(&workspace_root())
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(def.steps.len(), 2, "daily.toml must chain 2 pipelines");
    }
}
