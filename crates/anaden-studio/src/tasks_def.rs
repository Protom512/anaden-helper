use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

use crate::tasks_toml::{edit_task_toml_source, normalize_pipeline_dir_rel};

#[derive(Debug, Error)]
pub enum TaskError {
    #[error("task directory not found or unreadable: {0}")]
    Directory(String),
    #[error("no task definitions (*.toml) found in: {0}")]
    NoDefinitions(String),
    #[error("failed to read task file {path}: {source}")]
    Read {
        path: PathBuf,
        source: Box<std::io::Error>,
    },
    #[error("failed to parse task file {path}: {source}")]
    Parse {
        path: PathBuf,
        source: Box<toml::de::Error>,
    },
    #[error("invalid kind {kind:?} in {path}: expected \"launch_subcommand\" or \"pipeline_run\"")]
    InvalidKind { kind: String, path: PathBuf },
    #[error("pipeline_run task {id:?} is missing `pipeline_dir`")]
    MissingPipelineDir { id: String },
    #[error("task {id:?} is not implemented (implemented = false) and cannot be selected")]
    NotImplemented { id: String },
    #[error("unknown task id: {0}")]
    UnknownTask(String),
    #[error("task id mismatch in {path}: expected {expected:?}, file declares {actual:?}")]
    IdMismatch {
        expected: String,
        actual: String,
        path: PathBuf,
    },
    #[error("task {id:?} is not kind = \"pipeline_run\" and cannot be linked to a pipeline")]
    NotPipelineRun { id: String },
    #[error("pipeline not loadable: {dir}: {reason}")]
    PipelineNotLoadable { dir: String, reason: String },
    #[error("pipeline has no TaskDef TOMLs: {0}")]
    PipelineNoTaskDefs(String),
    #[error("pipeline_dir {dir:?} cannot be written as a TOML string value")]
    InvalidPipelineDir { dir: String },
    #[error("surgical edit of task file {path} failed: {reason}")]
    EditFailed { path: PathBuf, reason: String },
    #[error("failed to write task file {path}: {source}")]
    Write {
        path: PathBuf,
        source: Box<std::io::Error>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskKind {
    LaunchSubcommand,
    PipelineRun,
}

impl TaskKind {
    pub fn parse(s: &str, path: &Path) -> Result<Self, TaskError> {
        match s {
            "launch_subcommand" => Ok(Self::LaunchSubcommand),
            "pipeline_run" => Ok(Self::PipelineRun),
            _ => Err(TaskError::InvalidKind {
                kind: s.to_string(),
                path: path.to_path_buf(),
            }),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::LaunchSubcommand => "launch_subcommand",
            Self::PipelineRun => "pipeline_run",
        }
    }
}

#[derive(Debug, Deserialize)]
struct RawTaskDefinition {
    id: String,
    title: String,
    kind: String,
    #[serde(default)]
    implemented: bool,
    #[serde(default)]
    pipeline_dir: Option<String>,
    #[serde(default)]
    start_task: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskDefinition {
    pub id: String,
    pub title: String,
    pub kind: TaskKind,
    pub implemented: bool,
    pub pipeline_dir: Option<PathBuf>,
    pub start_task: Option<String>,
}

impl TaskDefinition {
    pub fn is_selectable(&self) -> bool {
        self.implemented
    }

    pub fn parse_toml(source: &str, path: &Path) -> Result<Self, TaskError> {
        let raw: RawTaskDefinition = toml::from_str(source).map_err(|source| TaskError::Parse {
            path: path.to_path_buf(),
            source: Box::new(source),
        })?;
        let kind = TaskKind::parse(&raw.kind, path)?;
        let pipeline_dir = match kind {
            TaskKind::PipelineRun => Some(
                raw.pipeline_dir
                    .ok_or_else(|| TaskError::MissingPipelineDir { id: raw.id.clone() })?
                    .into(),
            ),
            TaskKind::LaunchSubcommand => raw.pipeline_dir.map(Into::into),
        };
        Ok(Self {
            id: raw.id,
            title: raw.title,
            kind,
            implemented: raw.implemented,
            pipeline_dir,
            start_task: raw.start_task,
        })
    }
}

pub fn load_task_definitions(dir: &Path) -> Result<Vec<TaskDefinition>, TaskError> {
    let entries =
        std::fs::read_dir(dir).map_err(|_| TaskError::Directory(dir.display().to_string()))?;
    let mut defs = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }
        let source = std::fs::read_to_string(&path).map_err(|source| TaskError::Read {
            path: path.clone(),
            source: Box::new(source),
        })?;
        defs.push(TaskDefinition::parse_toml(&source, &path)?);
    }
    if defs.is_empty() {
        return Err(TaskError::NoDefinitions(dir.display().to_string()));
    }
    defs.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(defs)
}

pub fn enable_task(
    task_path: &Path,
    expected_id: &str,
    pipeline_dir_rel: &str,
    root: &Path,
) -> Result<TaskDefinition, TaskError> {
    let dir_value = normalize_pipeline_dir_rel(pipeline_dir_rel)?;
    let source = std::fs::read_to_string(task_path).map_err(|source| TaskError::Read {
        path: task_path.to_path_buf(),
        source: Box::new(source),
    })?;
    let raw: RawTaskDefinition = toml::from_str(&source).map_err(|source| TaskError::Parse {
        path: task_path.to_path_buf(),
        source: Box::new(source),
    })?;
    if raw.id != expected_id {
        return Err(TaskError::IdMismatch {
            expected: expected_id.to_string(),
            actual: raw.id,
            path: task_path.to_path_buf(),
        });
    }
    if TaskKind::parse(&raw.kind, task_path)? != TaskKind::PipelineRun {
        return Err(TaskError::NotPipelineRun {
            id: expected_id.to_string(),
        });
    }
    let abs_dir = root.join(&dir_value);
    anaden_vision::load_pipeline_manifest(&abs_dir).map_err(|e| {
        TaskError::PipelineNotLoadable {
            dir: dir_value.clone(),
            reason: e.to_string(),
        }
    })?;
    if anaden_vision::load_pipeline(&abs_dir)
        .map_err(|e| TaskError::PipelineNotLoadable {
            dir: dir_value.clone(),
            reason: e.to_string(),
        })?
        .is_empty()
    {
        return Err(TaskError::PipelineNoTaskDefs(dir_value));
    }
    let edited = edit_task_toml_source(&source, &dir_value, task_path)?;
    let def = TaskDefinition::parse_toml(&edited, task_path)?;
    if !def.implemented || def.pipeline_dir.as_deref() != Some(Path::new(&dir_value)) {
        return Err(TaskError::EditFailed {
            path: task_path.to_path_buf(),
            reason: format!(
                "edited source must declare implemented = true and pipeline_dir = {dir_value:?}"
            ),
        });
    }
    std::fs::write(task_path, &edited).map_err(|source| TaskError::Write {
        path: task_path.to_path_buf(),
        source: Box::new(source),
    })?;
    Ok(def)
}

#[cfg(test)]
pub(crate) mod test_util {
    pub(crate) const LAUNCH_TOML: &str = r#"
id = "launch"
title = "ゲーム起動"
kind = "launch_subcommand"
implemented = true
"#;

    pub(crate) const FIELD_LOOP_TOML: &str = r#"
id = "field_loop_pc"
title = "フィールド周回"
kind = "pipeline_run"
implemented = true
pipeline_dir = "templates/pipelines/field_loop_pc"
start_task = "start"
"#;

    pub(crate) const LOGIN_TOML: &str = r#"
id = "login"
title = "ログイン"
kind = "pipeline_run"
implemented = false
pipeline_dir = "templates/pipelines/login"
"#;
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use test_util::{FIELD_LOOP_TOML, LAUNCH_TOML};

    #[test]
    fn test_parses_launch_subcommand_task() {
        let def = TaskDefinition::parse_toml(LAUNCH_TOML, Path::new("launch.toml")).unwrap();
        assert_eq!(def.id, "launch");
        assert_eq!(def.title, "ゲーム起動");
        assert_eq!(def.kind, TaskKind::LaunchSubcommand);
        assert!(def.implemented);
        assert!(def.pipeline_dir.is_none());
        assert!(def.is_selectable());
    }

    #[test]
    fn test_parses_pipeline_run_task_with_dir() {
        let def =
            TaskDefinition::parse_toml(FIELD_LOOP_TOML, Path::new("field_loop_pc.toml")).unwrap();
        assert_eq!(def.kind, TaskKind::PipelineRun);
        assert_eq!(
            def.pipeline_dir.as_ref().unwrap(),
            Path::new("templates/pipelines/field_loop_pc")
        );
        assert_eq!(def.start_task.as_deref(), Some("start"));
    }

    #[test]
    fn test_load_definitions_from_directory_sorted_by_id() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("zz_launch.toml"), LAUNCH_TOML).unwrap();
        std::fs::write(tmp.path().join("aa_field.toml"), FIELD_LOOP_TOML).unwrap();
        std::fs::write(tmp.path().join("readme.txt"), "not a task").unwrap();
        let defs = load_task_definitions(tmp.path()).unwrap();
        let ids: Vec<&str> = defs.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(ids, vec!["field_loop_pc", "launch"]);
    }

    #[test]
    fn test_invalid_kind_is_fail_closed_error() {
        let bad = r#"
id = "x"
title = "X"
kind = "magic"
implemented = true
"#;
        let err = TaskDefinition::parse_toml(bad, Path::new("x.toml")).unwrap_err();
        assert!(matches!(err, TaskError::InvalidKind { .. }), "got {err:?}");
    }

    #[test]
    fn test_missing_toml_directory_or_file_fails_closed() {
        let err = load_task_definitions(Path::new("nonexistent-dir-xyz")).unwrap_err();
        assert!(matches!(err, TaskError::Directory(_)), "got {err:?}");

        let tmp = tempfile::tempdir().unwrap();
        let err = load_task_definitions(tmp.path()).unwrap_err();
        assert!(matches!(err, TaskError::NoDefinitions(_)), "got {err:?}");
    }

    #[test]
    fn test_pipeline_run_without_pipeline_dir_is_error() {
        let bad = r#"
id = "p"
title = "P"
kind = "pipeline_run"
implemented = true
"#;
        let err = TaskDefinition::parse_toml(bad, Path::new("p.toml")).unwrap_err();
        assert!(matches!(err, TaskError::MissingPipelineDir { .. }));
    }

    fn raw_implemented_flag(source: &str) -> Option<bool> {
        source.lines().find_map(|l| {
            let t = l.trim_start();
            if t.starts_with('#') {
                return None;
            }
            let rest = t.strip_prefix("implemented")?;
            let value = rest.trim_start().strip_prefix('=')?.trim();
            match value {
                "true" => Some(true),
                "false" => Some(false),
                _ => None,
            }
        })
    }

    #[test]
    fn test_repo_task_definitions_all_parse() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
        let tasks_dir = root.join("templates/tasks");
        let defs = load_task_definitions(&tasks_dir).unwrap();
        let all_ids = [
            "field_loop_pc",
            "fishing",
            "launch",
            "login",
            "nav_to_field_pc",
            "neko_nikki",
            "roguelike",
            "ticket_digest",
        ];
        let ids: Vec<&str> = defs.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(ids, all_ids);

        let selectable: Vec<&str> = defs
            .iter()
            .filter(|d| d.is_selectable())
            .map(|d| d.id.as_str())
            .collect();
        for id in all_ids {
            let source = std::fs::read_to_string(tasks_dir.join(format!("{id}.toml"))).unwrap();
            let file_implemented = raw_implemented_flag(&source)
                .unwrap_or_else(|| panic!("{id}.toml has no parsable `implemented` line"));
            let def = defs
                .iter()
                .find(|d| d.id == id)
                .unwrap_or_else(|| panic!("{id} missing from loaded defs"));
            assert_eq!(
                def.is_selectable(),
                file_implemented,
                "{id}: is_selectable must reflect the file's implemented flag"
            );
        }
        for known in ["field_loop_pc", "launch", "login", "nav_to_field_pc"] {
            assert!(selectable.contains(&known), "{known} must stay selectable");
        }
    }

    fn write_min_pipeline(dir: &Path) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("pipeline.toml"), "start_task = \"stub_start\"\n").unwrap();
        std::fs::write(
            dir.join("stub_start.toml"),
            "name = \"stub_start\"\nstate = \"Field\"\nalgorithm = \"ccoeff\"\ntemplate = \"stub.png\"\n",
        )
        .unwrap();
    }

    fn write_unimplemented_task(tasks_dir: &Path) -> PathBuf {
        std::fs::create_dir_all(tasks_dir).unwrap();
        let src = "# ねこにっきタスクのスケルトン (Issue #154)。\r\n\
                   # implemented = false: 未実装。グレー表示・チェック不可。\r\n\
                   id = \"neko_nikki\"\r\n\
                   title = \"ねこにっき\"\r\n\
                   kind = \"pipeline_run\"\r\n\
                   implemented = false\r\n\
                   pipeline_dir = \"templates/pipelines/neko_nikki\"\r\n";
        let path = tasks_dir.join("neko_nikki.toml");
        std::fs::write(&path, src).unwrap();
        path
    }

    #[test]
    fn test_enable_task_flips_implemented_and_keeps_comments_crlf() {
        let tmp = tempfile::tempdir().unwrap();
        let tasks_dir = tmp.path().join("templates").join("tasks");
        let task_path = write_unimplemented_task(&tasks_dir);
        write_min_pipeline(
            &tmp.path()
                .join("templates")
                .join("pipelines")
                .join("neko_nikki"),
        );

        let def = enable_task(
            &task_path,
            "neko_nikki",
            "templates/pipelines/neko_nikki",
            tmp.path(),
        )
        .unwrap();
        assert!(def.implemented);
        assert_eq!(
            def.pipeline_dir.as_deref(),
            Some(Path::new("templates/pipelines/neko_nikki"))
        );
        assert_eq!(def.kind, TaskKind::PipelineRun);

        let after = std::fs::read_to_string(&task_path).unwrap();
        assert!(
            after.starts_with("# ねこにっきタスクのスケルトン (Issue #154)。\r\n"),
            "comments dropped:\n{after}"
        );
        assert!(after.contains("# implemented = false: 未実装。グレー表示・チェック不可。\r\n"));
        assert!(after.contains("kind = \"pipeline_run\"\r\nimplemented = true\r\n"));
        assert!(
            after.contains("pipeline_dir = \"templates/pipelines/neko_nikki\"\r\n"),
            "after:\n{after}"
        );
        let defs = load_task_definitions(&tasks_dir).unwrap();
        let neko = defs
            .iter()
            .find(|d| d.id == "neko_nikki")
            .unwrap_or_else(|| panic!("neko_nikki missing after rewrite"));
        assert!(neko.is_selectable());
    }

    #[test]
    fn test_enable_task_rejects_id_mismatch_and_keeps_file() {
        let tmp = tempfile::tempdir().unwrap();
        let tasks_dir = tmp.path().join("tasks");
        let task_path = write_unimplemented_task(&tasks_dir);
        write_min_pipeline(&tmp.path().join("pipe"));
        let before = std::fs::read_to_string(&task_path).unwrap();
        let err = enable_task(&task_path, "roguelike", "pipe", tmp.path()).unwrap_err();
        assert!(matches!(err, TaskError::IdMismatch { .. }), "got {err:?}");
        assert_eq!(std::fs::read_to_string(&task_path).unwrap(), before);
    }

    #[test]
    fn test_enable_task_rejects_launch_subcommand() {
        let tmp = tempfile::tempdir().unwrap();
        let tasks_dir = tmp.path().join("tasks");
        std::fs::create_dir_all(&tasks_dir).unwrap();
        let task_path = tasks_dir.join("launch.toml");
        std::fs::write(
            &task_path,
            "# ゲーム起動\r\nid = \"launch\"\r\ntitle = \"起動\"\r\nkind = \"launch_subcommand\"\r\nimplemented = true\r\n",
        )
        .unwrap();
        write_min_pipeline(&tmp.path().join("pipe"));
        let before = std::fs::read_to_string(&task_path).unwrap();
        let err = enable_task(&task_path, "launch", "pipe", tmp.path()).unwrap_err();
        assert!(
            matches!(err, TaskError::NotPipelineRun { .. }),
            "got {err:?}"
        );
        assert_eq!(std::fs::read_to_string(&task_path).unwrap(), before);
    }

    #[test]
    fn test_enable_task_rejects_pipeline_without_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let tasks_dir = tmp.path().join("tasks");
        let task_path = write_unimplemented_task(&tasks_dir);

        let err = enable_task(&task_path, "neko_nikki", "pipelines/ghost", tmp.path()).unwrap_err();
        assert!(
            matches!(err, TaskError::PipelineNotLoadable { .. }),
            "got {err:?}"
        );

        let pipe = tmp.path().join("pipelines").join("neko_nikki");
        std::fs::create_dir_all(&pipe).unwrap();
        std::fs::write(
            pipe.join("stub_start.toml"),
            "name = \"stub_start\"\nstate = \"Field\"\nalgorithm = \"ccoeff\"\ntemplate = \"stub.png\"\n",
        )
        .unwrap();
        let err =
            enable_task(&task_path, "neko_nikki", "pipelines/neko_nikki", tmp.path()).unwrap_err();
        assert!(
            matches!(err, TaskError::PipelineNotLoadable { .. }),
            "got {err:?}"
        );

        let after = std::fs::read_to_string(&task_path).unwrap();
        assert!(after.contains("implemented = false\r\n"), "after:\n{after}");
    }

    #[test]
    fn test_enable_task_rejects_pipeline_with_zero_taskdefs() {
        let tmp = tempfile::tempdir().unwrap();
        let tasks_dir = tmp.path().join("tasks");
        let task_path = write_unimplemented_task(&tasks_dir);
        let pipe = tmp.path().join("pipe");
        std::fs::create_dir_all(&pipe).unwrap();
        std::fs::write(pipe.join("pipeline.toml"), "start_task = \"stub_start\"\n").unwrap();
        let err = enable_task(&task_path, "neko_nikki", "pipe", tmp.path()).unwrap_err();
        assert!(
            matches!(err, TaskError::PipelineNoTaskDefs(_)),
            "got {err:?}"
        );
        assert!(
            std::fs::read_to_string(&task_path)
                .unwrap()
                .contains("implemented = false")
        );
    }

    #[test]
    fn test_enable_task_inserts_missing_implemented_and_pipeline_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let tasks_dir = tmp.path().join("tasks");
        std::fs::create_dir_all(&tasks_dir).unwrap();
        let task_path = tasks_dir.join("bare.toml");
        std::fs::write(
            &task_path,
            "id = \"bare\"\ntitle = \"B\"\nkind = \"pipeline_run\"\n",
        )
        .unwrap();
        write_min_pipeline(&tmp.path().join("pipe"));
        let def = enable_task(&task_path, "bare", "pipe", tmp.path()).unwrap();
        assert!(def.implemented);
        assert_eq!(def.pipeline_dir.as_deref(), Some(Path::new("pipe")));
        let after = std::fs::read_to_string(&task_path).unwrap();
        assert!(
            after
                .contains("kind = \"pipeline_run\"\nimplemented = true\npipeline_dir = \"pipe\"\n"),
            "after:\n{after}"
        );
    }

    #[test]
    fn test_enable_task_preserves_inline_comment_on_implemented_line() {
        let tmp = tempfile::tempdir().unwrap();
        let tasks_dir = tmp.path().join("tasks");
        std::fs::create_dir_all(&tasks_dir).unwrap();
        let task_path = tasks_dir.join("c.toml");
        std::fs::write(
            &task_path,
            "id = \"c\"\ntitle = \"C\"\nkind = \"pipeline_run\"\nimplemented = false  # 有効化待ち\npipeline_dir = \"old\"\n",
        )
        .unwrap();
        write_min_pipeline(&tmp.path().join("pipe"));
        let def = enable_task(&task_path, "c", "pipe", tmp.path()).unwrap();
        assert!(def.implemented);
        assert_eq!(def.pipeline_dir.as_deref(), Some(Path::new("pipe")));
        let after = std::fs::read_to_string(&task_path).unwrap();
        assert!(
            after.contains("implemented = true  # 有効化待ち\n"),
            "after:\n{after}"
        );
        assert!(after.contains("pipeline_dir = \"pipe\"\n"));
    }

    #[test]
    fn test_enable_task_normalizes_backslash_path_to_forward_slash() {
        let tmp = tempfile::tempdir().unwrap();
        let tasks_dir = tmp.path().join("tasks");
        let task_path = write_unimplemented_task(&tasks_dir);
        write_min_pipeline(&tmp.path().join("pipe").join("sub"));
        let def = enable_task(&task_path, "neko_nikki", "pipe\\sub", tmp.path()).unwrap();
        assert_eq!(def.pipeline_dir.as_deref(), Some(Path::new("pipe/sub")));
        let after = std::fs::read_to_string(&task_path).unwrap();
        assert!(
            after.contains("pipeline_dir = \"pipe/sub\"\r\n"),
            "after:\n{after}"
        );
    }

    #[test]
    fn test_enable_task_rejects_unwritable_pipeline_dir_value() {
        let tmp = tempfile::tempdir().unwrap();
        let tasks_dir = tmp.path().join("tasks");
        let task_path = write_unimplemented_task(&tasks_dir);
        write_min_pipeline(&tmp.path().join("pipe"));
        let err = enable_task(&task_path, "neko_nikki", "pipe\"quote", tmp.path()).unwrap_err();
        assert!(
            matches!(err, TaskError::InvalidPipelineDir { .. }),
            "got {err:?}"
        );
        assert!(
            std::fs::read_to_string(&task_path)
                .unwrap()
                .contains("implemented = false")
        );
    }

    #[test]
    fn test_enable_task_repo_tomls_stay_parseable_with_comments() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
        let repo_tasks = root.join("templates").join("tasks");
        let pipeline_run_ids = [
            "field_loop_pc",
            "fishing",
            "login",
            "nav_to_field_pc",
            "neko_nikki",
            "roguelike",
            "ticket_digest",
        ];
        for id in pipeline_run_ids {
            let tmp = tempfile::tempdir().unwrap();
            let tasks_dir = tmp.path().join("tasks");
            std::fs::create_dir_all(&tasks_dir).unwrap();
            write_min_pipeline(&tmp.path().join("pipe"));
            let task_path = tasks_dir.join(format!("{id}.toml"));
            std::fs::copy(repo_tasks.join(format!("{id}.toml")), &task_path).unwrap();
            let original = std::fs::read_to_string(&task_path).unwrap();

            let def = enable_task(&task_path, id, "pipe", tmp.path())
                .unwrap_or_else(|e| panic!("enable failed for {id}: {e}"));
            assert!(def.implemented, "{id}");
            assert_eq!(def.pipeline_dir.as_deref(), Some(Path::new("pipe")), "{id}");

            let after = std::fs::read_to_string(&task_path).unwrap();
            for line in original.lines().filter(|l| l.trim_start().starts_with('#')) {
                assert!(after.contains(line), "{id}: comment line lost: {line}");
            }
            let defs = load_task_definitions(&tasks_dir).unwrap();
            let enabled = defs
                .iter()
                .find(|d| d.id == id)
                .unwrap_or_else(|| panic!("{id} missing after rewrite"));
            assert!(enabled.is_selectable(), "{id}");
        }
    }
}
