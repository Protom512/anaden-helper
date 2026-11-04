use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::tasks::{self, TaskDefinition, TaskError, TaskKind};

#[derive(Debug, Error)]
pub enum ScenarioTaskError {
    #[error("task id `{id}` is not a safe task file name")]
    UnsafeTaskId { id: String },
    #[error("task already exists (overwrite refused): {0}")]
    TaskAlreadyExists(PathBuf),
    #[error("pipeline dir {dir} is not a subdirectory of the workspace root")]
    NotRootRelative { dir: String },
    #[error("generated task TOML failed pre-write validation: {0}")]
    InvalidGenerated(#[source] TaskError),
    #[error("failed to write task file {path}")]
    Write {
        path: PathBuf,
        source: Box<std::io::Error>,
    },
    #[error("task not found: {0}")]
    TaskNotFound(PathBuf),
    #[error(transparent)]
    Enable(#[from] TaskError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StubTaskOption {
    pub id: String,
    pub title: String,
}

#[must_use]
pub fn stub_options(defs: &[TaskDefinition]) -> Vec<StubTaskOption> {
    defs.iter()
        .filter(|d| d.kind == TaskKind::PipelineRun && !d.implemented)
        .map(|d| StubTaskOption {
            id: d.id.clone(),
            title: d.title.clone(),
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScenarioPanelEvent {
    TaskEnabled { message: String },
}

pub struct TaskLinkContext<'a> {
    pub root: &'a Path,
    pub tasks_dir: &'a Path,
    pub stubs: &'a [StubTaskOption],
}

fn is_safe_task_id(id: &str) -> bool {
    !id.is_empty()
        && Path::new(id).file_name() == Some(OsStr::new(id))
        && !id
            .chars()
            .any(|c| matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*' | '\\') || c.is_control())
}

fn pipeline_dir_rel(pipeline_dir: &Path, root: &Path) -> Result<String, ScenarioTaskError> {
    let rel = crate::scenario_ui::resolve_template_reference(pipeline_dir, root);
    let unsafe_rel =
        rel.is_empty() || Path::new(&rel).is_absolute() || rel.split('/').any(|seg| seg == "..");
    if unsafe_rel {
        return Err(ScenarioTaskError::NotRootRelative {
            dir: pipeline_dir.display().to_string(),
        });
    }
    Ok(rel)
}

#[derive(serde::Serialize)]
struct GeneratedTaskToml<'a> {
    id: &'a str,
    title: &'a str,
    kind: &'a str,
    implemented: bool,
    pipeline_dir: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    start_task: Option<&'a str>,
}

fn generate_task_toml(
    id: &str,
    title: &str,
    pipeline_dir_rel: &str,
    start_task: &str,
) -> Result<String, ScenarioTaskError> {
    let comment_title: String = title
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let start = if start_task.trim().is_empty() {
        None
    } else {
        Some(start_task.trim())
    };
    let body = toml::to_string(&GeneratedTaskToml {
        id,
        title,
        kind: "pipeline_run",
        implemented: false,
        pipeline_dir: pipeline_dir_rel,
        start_task: start,
    })
    .map_err(|e| {
        ScenarioTaskError::InvalidGenerated(TaskError::EditFailed {
            path: PathBuf::from(format!("{id}.toml")),
            reason: format!("serialization failed: {e}"),
        })
    })?;
    Ok(format!(
        "# {comment_title} タスク (Issue #160 シナリオ作成 GUI から自動登録)。\n\
         # 実行経路: `anaden run {pipeline_dir_rel}`。\n{body}"
    ))
}

pub fn register_and_enable_task(
    tasks_dir: &Path,
    root: &Path,
    pipeline_dir: &Path,
    task_id: &str,
    title: &str,
    start_task: &str,
) -> Result<TaskDefinition, ScenarioTaskError> {
    let id = task_id.trim();
    if !is_safe_task_id(id) {
        return Err(ScenarioTaskError::UnsafeTaskId {
            id: task_id.to_string(),
        });
    }
    let title = if title.trim().is_empty() {
        id
    } else {
        title.trim()
    };
    let dir_rel = pipeline_dir_rel(pipeline_dir, root)?;
    let task_path = tasks_dir.join(format!("{id}.toml"));
    if task_path.exists() {
        return Err(ScenarioTaskError::TaskAlreadyExists(task_path));
    }
    let source = generate_task_toml(id, title, &dir_rel, start_task)?;
    TaskDefinition::parse_toml(&source, &task_path).map_err(ScenarioTaskError::InvalidGenerated)?;
    std::fs::create_dir_all(tasks_dir).map_err(|source| ScenarioTaskError::Write {
        path: tasks_dir.to_path_buf(),
        source: Box::new(source),
    })?;
    if let Err(source) = std::fs::write(&task_path, &source) {
        let _ = std::fs::remove_file(&task_path);
        return Err(ScenarioTaskError::Write {
            path: task_path,
            source: Box::new(source),
        });
    }
    match tasks::enable_task(&task_path, id, &dir_rel, root) {
        Ok(def) => Ok(def),
        Err(e) => {
            let _ = std::fs::remove_file(&task_path);
            Err(ScenarioTaskError::Enable(e))
        }
    }
}

pub fn bind_and_enable_task(
    tasks_dir: &Path,
    root: &Path,
    task_id: &str,
    pipeline_dir: &Path,
) -> Result<TaskDefinition, ScenarioTaskError> {
    let id = task_id.trim();
    if !is_safe_task_id(id) {
        return Err(ScenarioTaskError::UnsafeTaskId {
            id: task_id.to_string(),
        });
    }
    let dir_rel = pipeline_dir_rel(pipeline_dir, root)?;
    let task_path = tasks_dir.join(format!("{id}.toml"));
    if !task_path.exists() {
        return Err(ScenarioTaskError::TaskNotFound(task_path));
    }
    tasks::enable_task(&task_path, id, &dir_rel, root).map_err(ScenarioTaskError::Enable)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::scenario_ui::ScenarioEditorState;
    use anaden_core::{Goal, StopCondition};
    use anaden_vision::{Action, Algorithm, TaskDef};
    use std::fs;

    fn task_def(name: &str) -> TaskDef {
        TaskDef {
            name: name.to_string(),
            state: "Field".to_string(),
            algorithm: Algorithm::Ccoeff,
            template: std::path::PathBuf::from(format!("{name}.png")),
            roi: Some([10, 20, 100, 50]),
            threshold: 0.8,
            base: None,
            action: Some(Action::ClickSelf),
            next: Some(vec![]),
        }
    }

    fn saved_pipeline(root: &Path, name: &str) -> PathBuf {
        let pipelines_root = root.join("templates/pipelines");
        let mut st = ScenarioEditorState::new(name);
        st.add_task(task_def("Start"));
        st.add_task(task_def("Loop"));
        st.task_mut("Start").unwrap().next = Some(vec!["Loop".to_string()]);
        st.add_goal(Goal {
            name: "loop3".to_string(),
            stop: StopCondition::LoopCount { target: 3 },
        });
        crate::scenario_ui::save_scenario(&st, &[], &pipelines_root).expect("save scenario")
    }

    #[test]
    fn register_generates_enabled_selectable_task_toml() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        let tasks_dir = root.join("templates/tasks");
        let pipeline = saved_pipeline(root, "fishing2");

        let def = register_and_enable_task(
            &tasks_dir,
            root,
            &pipeline,
            "fishing2",
            "つり (新規)",
            "Start",
        )
        .expect("register");

        assert!(def.implemented && def.is_selectable());
        assert_eq!(def.kind, TaskKind::PipelineRun);
        assert_eq!(
            def.pipeline_dir.as_deref(),
            Some(Path::new("templates/pipelines/fishing2"))
        );
        assert_eq!(def.start_task.as_deref(), Some("Start"));
        let source = fs::read_to_string(tasks_dir.join("fishing2.toml")).expect("read");
        assert!(
            source.starts_with('#'),
            "人間可読ヘッダコメント必須:\n{source}"
        );
        let reparsed =
            TaskDefinition::parse_toml(&source, Path::new("fishing2.toml")).expect("reparse");
        assert!(reparsed.implemented && reparsed.is_selectable());
        assert_eq!(reparsed.id, "fishing2");
        assert_eq!(reparsed.title, "つり (新規)");
    }

    #[test]
    fn bind_enables_existing_stub_writing_back_pipeline_dir() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        let tasks_dir = root.join("templates/tasks");
        fs::create_dir_all(&tasks_dir).expect("mkdir");
        let pipeline = saved_pipeline(root, "fishing2");
        fs::write(
            tasks_dir.join("neko_nikki.toml"),
            "# ねこにっきタスクのスケルトン (Issue #154)。\n\
             # implemented = false: 未実装。グレー表示・チェック不可。\n\
             id = \"neko_nikki\"\n\
             title = \"ねこにっき\"\n\
             kind = \"pipeline_run\"\n\
             implemented = false\n\
             pipeline_dir = \"templates/pipelines/neko_nikki\"\n",
        )
        .expect("write stub");

        let def = bind_and_enable_task(&tasks_dir, root, "neko_nikki", &pipeline).expect("bind");

        assert!(def.implemented && def.is_selectable());
        assert_eq!(
            def.pipeline_dir.as_deref(),
            Some(Path::new("templates/pipelines/fishing2"))
        );
        let source = fs::read_to_string(tasks_dir.join("neko_nikki.toml")).expect("read");
        assert!(source.contains("implemented = true"), "フリップ: {source}");
        assert!(
            source.contains("templates/pipelines/fishing2"),
            "pipeline_dir 書き戻り: {source}"
        );
        assert!(
            source.starts_with("# ねこにっきタスクのスケルトン"),
            "由来コメントは外科的編集で保全: {source}"
        );
    }

    #[test]
    fn register_refuses_existing_task_toml_leaving_file_untouched() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        let tasks_dir = root.join("templates/tasks");
        fs::create_dir_all(&tasks_dir).expect("mkdir");
        let pipeline = saved_pipeline(root, "fishing2");
        let existing = tasks_dir.join("fishing2.toml");
        let original = "# 既存定義 (絶対に書き換えさせない)。\n\
                        id = \"fishing2\"\n\
                        title = \"既存\"\n\
                        kind = \"launch_subcommand\"\n\
                        implemented = true\n";
        fs::write(&existing, original).expect("write existing");

        let err =
            register_and_enable_task(&tasks_dir, root, &pipeline, "fishing2", "新規", "Start")
                .expect_err("must refuse");

        assert!(
            matches!(err, ScenarioTaskError::TaskAlreadyExists(ref p) if *p == existing),
            "err: {err:?}"
        );
        assert_eq!(
            fs::read_to_string(&existing).expect("read"),
            original,
            "既存ファイルは 1 バイトも変わらない"
        );
    }

    #[test]
    fn register_rejects_unsafe_task_id_and_outside_root_pipeline() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        let tasks_dir = root.join("templates/tasks");
        let pipeline = saved_pipeline(root, "fishing2");

        for bad in ["a/b", "..", "", "a\\b", "a:b", "a<b", "a\"b"] {
            let err = register_and_enable_task(&tasks_dir, root, &pipeline, bad, "t", "Start")
                .expect_err("must reject");
            assert!(
                matches!(err, ScenarioTaskError::UnsafeTaskId { .. }),
                "bad {bad:?}: {err:?}"
            );
        }
        let outside = if cfg!(windows) {
            PathBuf::from(r"D:\elsewhere\pipeline")
        } else {
            PathBuf::from("/elsewhere/pipeline")
        };
        let err =
            bind_and_enable_task(&tasks_dir, root, "neko_nikki", &outside).expect_err("reject");
        assert!(
            matches!(err, ScenarioTaskError::NotRootRelative { .. }),
            "err: {err:?}"
        );
    }

    #[test]
    fn bind_rejects_unsafe_task_id_before_path_join() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        let tasks_dir = root.join("templates/tasks");
        fs::create_dir_all(&tasks_dir).expect("mkdir");
        let pipeline = saved_pipeline(root, "fishing2");

        for bad in ["a/b", "..", "", "a\\b", "a:b", "a<b", "a\"b"] {
            let err =
                bind_and_enable_task(&tasks_dir, root, bad, &pipeline).expect_err("must reject");
            assert!(
                matches!(err, ScenarioTaskError::UnsafeTaskId { .. }),
                "bad {bad:?}: {err:?}"
            );
        }
        assert!(
            fs::read_dir(&tasks_dir).expect("read_dir").next().is_none(),
            "不正 id では一切の副作用を残さない"
        );
    }

    #[test]
    fn register_compensates_task_toml_on_write_failure() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        let tasks_dir = root.join("templates/tasks");
        let pipeline = saved_pipeline(root, "fishing2");
        let long_id = "A".repeat(300);

        let err =
            register_and_enable_task(&tasks_dir, root, &pipeline, &long_id, "長い名前", "Start")
                .expect_err("write must fail");

        assert!(
            matches!(err, ScenarioTaskError::Write { .. }),
            "err: {err:?}"
        );
        let residue: Vec<_> = fs::read_dir(&tasks_dir)
            .expect("tasks_dir exists (create_dir_all 成功後)")
            .collect();
        assert!(
            residue.is_empty(),
            "write 失敗時は部分ファイルを含め残留ゼロ: {residue:?}"
        );
    }

    #[test]
    fn register_write_failure_keeps_preexisting_tasks_dir_file_intact() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        let tasks_dir = root.join("templates/tasks");
        fs::create_dir_all(root.join("templates")).expect("mkdir");
        fs::write(&tasks_dir, b"not a directory").expect("prepare file");
        let pipeline = saved_pipeline(root, "fishing2");

        let err =
            register_and_enable_task(&tasks_dir, root, &pipeline, "fishing2", "新規", "Start")
                .expect_err("must fail");

        assert!(
            matches!(err, ScenarioTaskError::Write { .. }),
            "err: {err:?}"
        );
        assert_eq!(
            fs::read(&tasks_dir).expect("read"),
            b"not a directory",
            "前提資産は 1 バイトも変わらない"
        );
    }

    #[test]
    fn register_propagates_enable_error_and_cleans_up_created_toml() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        let tasks_dir = root.join("templates/tasks");
        let ghost = root.join("templates/pipelines/ghost");

        let err =
            register_and_enable_task(&tasks_dir, root, &ghost, "ghost_task", "ゴースト", "Start")
                .expect_err("must fail");

        assert!(
            matches!(
                err,
                ScenarioTaskError::Enable(TaskError::PipelineNotLoadable { .. })
            ),
            "err: {err:?}"
        );
        assert!(
            !tasks_dir.join("ghost_task.toml").exists(),
            "有効化失敗時は登録 TOML を補償削除 (部分状態を残さない)"
        );
    }

    #[test]
    fn stub_options_filter_unimplemented_pipeline_run_only() {
        let parse =
            |src: &str, name: &str| TaskDefinition::parse_toml(src, Path::new(name)).unwrap();
        let defs = [
            parse(
                "id = \"launch\"\ntitle = \"起動\"\nkind = \"launch_subcommand\"\nimplemented = true\n",
                "launch.toml",
            ),
            parse(
                "id = \"stub\"\ntitle = \"スタブ\"\nkind = \"pipeline_run\"\nimplemented = false\npipeline_dir = \"x\"\n",
                "stub.toml",
            ),
            parse(
                "id = \"done\"\ntitle = \"実装済\"\nkind = \"pipeline_run\"\nimplemented = true\npipeline_dir = \"x\"\n",
                "done.toml",
            ),
        ];
        let stubs = stub_options(&defs);
        assert_eq!(stubs.len(), 1);
        assert_eq!(stubs[0].id, "stub");
        assert_eq!(stubs[0].title, "スタブ");
    }

    #[test]
    fn register_uses_task_id_as_title_when_title_empty() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        let tasks_dir = root.join("templates/tasks");
        let pipeline = saved_pipeline(root, "fishing2");

        let def = register_and_enable_task(&tasks_dir, root, &pipeline, "fishing2", "   ", "Start")
            .expect("register");
        assert_eq!(def.title, "fishing2");
    }
}
