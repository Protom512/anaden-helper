//! Routine 定義スキーマ + loader + バリデーション (Issue #199)。
//!
//! routine = 複数 pipeline を1日のルーチンとして連続実行する宣言的定義。
//! `templates/routines/<name>.toml` に pipeline 列 (各ステップ: pipeline_dir /
//! start_task / max_iters / interval / on_failure) を記述し、
//! [`load_routine`] + [`RoutineDef::validate`] で fail-closed 検証してから
//! [`crate::routine_runner::run_routine`] へ渡す。
//!
//! スキーマは既存宣言層 (Goal の `deny_unknown_fields`・StrategyDef の
//! pipeline_dir/start_task 文字列) と同じ形式慣行に従う。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// [`RoutineStep::max_iters`] の既定値 (`run` サブコマンドの既定と同一)。
pub const DEFAULT_STEP_MAX_ITERS: u64 = 100;
/// [`RoutineStep::interval`] の既定値 (秒・`run` サブコマンドの既定と同一)。
pub const DEFAULT_STEP_INTERVAL_SECS: u64 = 1;

/// routine 定義の読込・検証エラー (fail-closed・panic しない)。
#[derive(Debug, Clone, Error)]
pub enum RoutineError {
    /// routine ファイルの読込失敗 (存在しない・権限等)。
    #[error("routine ファイル読込失敗 {path}: {reason}")]
    ReadFailed {
        /// 読込を試みた routine ファイルパス。
        path: PathBuf,
        /// io エラー文言。
        reason: String,
    },
    /// routine TOML のパース失敗 (構文・未知フィールド・型不一致)。
    #[error("routine TOML パース失敗 {path}: {reason}")]
    ParseFailed {
        /// パース対象の routine ファイルパス。
        path: PathBuf,
        /// toml エラー文言。
        reason: String,
    },
    /// steps が空 (ルーチンとして実行する pipeline が無い)。
    #[error("routine '{name}' の steps が空です: 少なくとも1ステップ必要です")]
    EmptySteps {
        /// routine 名。
        name: String,
    },
    /// ステップの pipeline_dir が実在ディレクトリに解決できない。
    #[error("ステップ '{step}' の pipeline_dir が見つかりません: {dir} (root={root})")]
    PipelineDirNotFound {
        /// ステップ名。
        step: String,
        /// 解決できなかった pipeline_dir 文字列。
        dir: String,
        /// 解決基準に使った workspace ルート。
        root: PathBuf,
    },
    /// pipeline ディレクトリの読込 (TaskDef ロード) 自体が失敗。
    #[error("ステップ '{step}' の pipeline 読込失敗 {dir}: {reason}")]
    PipelineLoadFailed {
        /// ステップ名。
        step: String,
        /// pipeline ディレクトリ。
        dir: PathBuf,
        /// load_pipeline のエラー文言。
        reason: String,
    },
    /// start_task が pipeline の TaskDef に存在しない。
    #[error(
        "ステップ '{step}' の start_task '{start_task}' が pipeline {dir} に存在しません \
         (利用可能: {available})"
    )]
    StartTaskNotFound {
        /// ステップ名。
        step: String,
        /// 指定された開始タスク名。
        start_task: String,
        /// pipeline ディレクトリ。
        dir: PathBuf,
        /// pipeline 内の実在タスク名一覧 (カンマ区切り)。
        available: String,
    },
    /// ステップパラメータの値が不正 (interval=0 等)。
    #[error("ステップ '{step}' の {field} が不正です: {reason}")]
    BadParam {
        /// ステップ名。
        step: String,
        /// 不正なフィールド名。
        field: &'static str,
        /// 理由。
        reason: String,
    },
    /// routine 名が空。
    #[error("routine 名が空です")]
    EmptyName,
    /// ステップ実行時の異常 (pipeline 解決・デバイス構築・実行開始の失敗)。
    #[error("ステップ '{step}' の実行に失敗: {reason}")]
    InvocationFailed {
        /// ステップ名。
        step: String,
        /// 異常の内容。
        reason: String,
    },
    /// ステップ名が空。
    #[error("{index} 番目のステップ名が空です")]
    EmptyStepName {
        /// ステップの位置 (0-origin)。
        index: usize,
    },
}

/// ステップ失敗時の継続ポリシー。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum OnFailure {
    /// 即座に routine 全体を中止 (残りステップは Skipped)。既定。
    #[default]
    Stop,
    /// 当該ステップを失敗扱いのままスキップし、次ステップへ継続。
    Skip,
}

/// routine の1ステップ (= pipeline 実行1回)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoutineStep {
    /// ステップの表示名 (サマリ・履歴で参照)。
    pub name: String,
    /// `anaden run` 相当の pipeline ディレクトリ (workspace ルート相対 or 絶対)。
    pub pipeline_dir: String,
    /// 開始タスク名 (pipeline の TaskDef `name` と一致必須)。
    pub start_task: String,
    /// 最大サイクル数。既定 [`DEFAULT_STEP_MAX_ITERS`]。
    #[serde(default = "default_max_iters")]
    pub max_iters: u64,
    /// ループ間隔 (秒)。既定 [`DEFAULT_STEP_INTERVAL_SECS`]。
    #[serde(default = "default_interval")]
    pub interval: u64,
    /// 失敗時ポリシー。既定 [`OnFailure::Stop`]。
    #[serde(default)]
    pub on_failure: OnFailure,
}

fn default_max_iters() -> u64 {
    DEFAULT_STEP_MAX_ITERS
}

fn default_interval() -> u64 {
    DEFAULT_STEP_INTERVAL_SECS
}

/// routine 定義 (TOML 1:1)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoutineDef {
    /// routine 名 (evidence・履歴の識別子)。
    pub name: String,
    /// 順次実行する pipeline 列。
    pub steps: Vec<RoutineStep>,
}

/// routine TOML ファイルを読み込む (検証は別途 [`RoutineDef::validate`] で)。
///
/// # Errors
/// ファイル読込失敗は [`RoutineError::ReadFailed`]、TOML パース失敗
/// (構文エラー・未知フィールド・型不一致) は [`RoutineError::ParseFailed`]。
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

/// ステップの pipeline_dir を決定的に解決する (CLI `resolve_pipeline_dir` と同一規約)。
///
/// 候補順 (最初に実在するディレクトリ):
/// 1. 与えられたパス自体 (絶対 or cwd 相対)
/// 2. `<root>/<相対パス>` (例: `templates/pipelines/login`)
/// 3. `<root>/templates/pipelines/<basename>` (bare name)
///
/// いずれも実在しなければ [`None`] (fail-closed: 偽パスを捏造しない)。
#[must_use]
pub fn resolve_step_pipeline_dir(input: &str, root: &Path) -> Option<PathBuf> {
    let as_path = Path::new(input);
    if as_path.is_dir() {
        return Some(as_path.to_path_buf());
    }
    if as_path.is_absolute() {
        // 絶対パスで非実在の場合は候補 3 の basename のみ試す。
        let base = as_path.file_name()?.to_string_lossy().into_owned();
        let pipelined = root.join("templates").join("pipelines").join(base);
        return pipelined.is_dir().then_some(pipelined);
    }
    let joined = root.join(input);
    if joined.is_dir() {
        return Some(joined);
    }
    // bare name (`login` 等) は templates/pipelines 基準で解決。
    if !input.contains('/') && !input.contains('\\') && !input.starts_with("templates") {
        let pipelined = root.join("templates").join("pipelines").join(input);
        if pipelined.is_dir() {
            return Some(pipelined);
        }
    }
    None
}

impl RoutineDef {
    /// 定義を fail-closed 検証する。
    ///
    /// 検証項目:
    /// - name / 各 step name が空でない
    /// - steps が空でない
    /// - 各ステップの pipeline_dir が解決可能で、[`anaden_vision::load_pipeline`]
    ///   で TaskDef がロード可能 (空 pipeline も不可)
    /// - start_task がロードした TaskDef の `name` に存在
    /// - interval / max_iters が 1 以上
    ///
    /// # Errors
    /// 上記いずれかの違反を [`RoutineError`] で返す (最初の違反で即時中断)。
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

/// 1ステップ分の pipeline 参照整合性を検証する (`validate` の内部実装)。
fn validate_step(step: &RoutineStep, root: &Path) -> Result<(), RoutineError> {
    let dir = resolve_step_pipeline_dir(&step.pipeline_dir, root).ok_or_else(|| {
        RoutineError::PipelineDirNotFound {
            step: step.name.clone(),
            dir: step.pipeline_dir.clone(),
            root: root.to_path_buf(),
        }
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

    /// workspace ルート (anaden-engine manifest から 2 階層上昇)。
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

    // ---- parse / defaults ----

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

    // ---- validation (fail-closed) ----

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

    /// 実在 pipeline 2つ (login → nav_to_field_pc) を参照する routine が
    /// 実 workspace ルートで検証を通る (UC-1 の headless 検証)。
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

    /// templates/routines/daily.toml (サンプル) が実ルートで load + validate を通る。
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
