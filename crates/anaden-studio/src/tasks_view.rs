use std::path::Path;

use crate::tasks::{TaskDefinition, TaskKind, resolve_start_task_name, spawn_args};

#[must_use]
pub fn checkbox_label(def: &TaskDefinition) -> String {
    format!(
        "{} ({}){}",
        def.title,
        def.kind.as_str(),
        if def.implemented {
            String::new()
        } else {
            " — 未実装".to_string()
        }
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskDetailView {
    pub id: String,
    pub title: String,
    pub kind: &'static str,
    pub pipeline_dir: Option<String>,
    pub start_task: Option<String>,
    pub spawn_args: Vec<String>,
    pub unimplemented: bool,
    pub unimplemented_reason: Option<String>,
}

impl TaskDetailView {
    #[must_use]
    pub fn args_preview(&self) -> String {
        self.spawn_args.join(" ")
    }
}

#[must_use]
pub fn task_detail_view(
    def: &TaskDefinition,
    target: &str,
    serial: Option<&str>,
    root: &Path,
) -> TaskDetailView {
    let pipeline_dir = def
        .pipeline_dir
        .as_ref()
        .map(|d| root.join(d).to_string_lossy().into_owned());
    let start_task = match def.kind {
        TaskKind::LaunchSubcommand => None,
        TaskKind::PipelineRun => match &def.start_task {
            Some(s) => Some(s.clone()),
            None => def
                .pipeline_dir
                .as_ref()
                .map(|d| root.join(d))
                .and_then(|abs| resolve_start_task_name(&abs)),
        },
    };
    let spawn_args = spawn_args(def, target, serial, root);
    let unimplemented_reason = if !def.implemented {
        Some("implemented = false (未実装タスク)".to_string())
    } else if spawn_args.is_empty() {
        Some("実行引数を解決できません (pipeline_dir/start_task 不明)".to_string())
    } else {
        None
    };
    TaskDetailView {
        id: def.id.clone(),
        title: def.title.clone(),
        kind: def.kind.as_str(),
        pipeline_dir,
        start_task,
        spawn_args,
        unimplemented: unimplemented_reason.is_some(),
        unimplemented_reason,
    }
}

#[must_use]
pub fn queue_position_label(selected_ids: &[String], id: &str) -> Option<String> {
    let pos = selected_ids.iter().position(|s| s == id)?;
    Some(format!("実行順 {}/{}", pos + 1, selected_ids.len()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueOrderRow {
    pub position: usize,
    pub id: String,
    pub title: String,
    pub runnable: bool,
}

#[must_use]
pub fn queue_order_rows(selected_ids: &[String], defs: &[TaskDefinition]) -> Vec<QueueOrderRow> {
    selected_ids
        .iter()
        .enumerate()
        .filter_map(|(i, id)| {
            defs.iter().find(|d| &d.id == id).map(|def| QueueOrderRow {
                position: i + 1,
                id: def.id.clone(),
                title: def.title.clone(),
                runnable: def.implemented,
            })
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    const LAUNCH_TOML: &str = r#"
id = "launch"
title = "ゲーム起動"
kind = "launch_subcommand"
implemented = true
"#;

    const FIELD_LOOP_TOML: &str = r#"
id = "field_loop_pc"
title = "フィールド周回"
kind = "pipeline_run"
implemented = true
pipeline_dir = "templates/pipelines/field_loop_pc"
start_task = "start"
"#;

    const LOGIN_TOML: &str = r#"
id = "login"
title = "ログイン"
kind = "pipeline_run"
implemented = false
pipeline_dir = "templates/pipelines/login"
"#;

    fn parse_all(sources: &[(&str, &str)]) -> Vec<TaskDefinition> {
        sources
            .iter()
            .map(|(name, src)| TaskDefinition::parse_toml(src, Path::new(name)).unwrap())
            .collect()
    }

    #[test]
    fn test_detail_view_launch_subcommand() {
        let def = TaskDefinition::parse_toml(LAUNCH_TOML, Path::new("launch.toml")).unwrap();
        let view = task_detail_view(&def, "windows", Some("ignored"), Path::new("/root"));
        assert_eq!(view.id, "launch");
        assert_eq!(view.title, "ゲーム起動");
        assert_eq!(view.kind, "launch_subcommand");
        assert_eq!(view.pipeline_dir, None);
        assert_eq!(view.start_task, None);
        assert_eq!(view.args_preview(), "launch");
        assert!(!view.unimplemented);
        assert_eq!(view.unimplemented_reason, None);
    }

    #[test]
    fn test_detail_view_pipeline_run_declared_start_task() {
        let def =
            TaskDefinition::parse_toml(FIELD_LOOP_TOML, Path::new("field_loop_pc.toml")).unwrap();
        let view = task_detail_view(&def, "windows", None, Path::new("/root"));
        assert_eq!(view.kind, "pipeline_run");
        let dir = view.pipeline_dir.as_deref().unwrap();
        assert!(
            dir.ends_with("templates/pipelines/field_loop_pc")
                || dir.ends_with("templates\\pipelines\\field_loop_pc"),
            "dir: {dir}"
        );
        assert_eq!(view.start_task.as_deref(), Some("start"));
        assert_eq!(view.spawn_args.len(), 3);
        assert_eq!(view.spawn_args.last().map(String::as_str), Some("start"));
        assert!(!view.unimplemented);
    }

    #[test]
    fn test_detail_view_pipeline_run_resolves_undeclared_start_task() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
        let src = r#"
id = "nav_to_field_pc"
title = "マップ移動"
kind = "pipeline_run"
implemented = true
pipeline_dir = "templates/pipelines/nav_to_field_pc"
"#;
        let def = TaskDefinition::parse_toml(src, Path::new("nav_to_field_pc.toml")).unwrap();
        let view = task_detail_view(&def, "windows", None, &root);
        assert_eq!(view.start_task.as_deref(), Some("TapToStartPc"));
        assert_eq!(
            view.spawn_args.last().map(String::as_str),
            Some("TapToStartPc")
        );
        assert!(!view.unimplemented);
    }

    #[test]
    fn test_queue_position_label_uses_check_order() {
        let selected = vec!["b".to_string(), "a".to_string(), "c".to_string()];
        assert_eq!(
            queue_position_label(&selected, "b").as_deref(),
            Some("実行順 1/3")
        );
        assert_eq!(
            queue_position_label(&selected, "a").as_deref(),
            Some("実行順 2/3")
        );
        assert_eq!(queue_position_label(&selected, "zzz"), None);
    }

    #[test]
    fn test_queue_order_rows_numbered_in_check_order() {
        let defs = parse_all(&[
            ("launch.toml", LAUNCH_TOML),
            ("field_loop_pc.toml", FIELD_LOOP_TOML),
        ]);
        let selected = vec!["field_loop_pc".to_string(), "launch".to_string()];
        let rows = queue_order_rows(&selected, &defs);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].position, 1);
        assert_eq!(rows[0].title, "フィールド周回");
        assert_eq!(rows[1].position, 2);
        assert_eq!(rows[1].title, "ゲーム起動");
        assert!(rows.iter().all(|r| r.runnable));
    }

    #[test]
    fn test_detail_view_unimplemented_task_carries_label_data() {
        let defs = parse_all(&[("login.toml", LOGIN_TOML)]);
        let view = task_detail_view(&defs[0], "windows", None, Path::new("/root"));
        assert!(view.unimplemented);
        let reason = view.unimplemented_reason.as_deref().unwrap();
        assert!(reason.contains("implemented = false"), "reason: {reason}");
    }

    #[test]
    fn test_detail_view_unresolvable_pipeline_fail_closed() {
        let src = r#"
id = "ghost"
title = "G"
kind = "pipeline_run"
implemented = true
pipeline_dir = "templates/pipelines/nonexistent-xyz"
"#;
        let def = TaskDefinition::parse_toml(src, Path::new("ghost.toml")).unwrap();
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
        let view = task_detail_view(&def, "windows", None, &root);
        assert!(view.spawn_args.is_empty());
        assert_eq!(view.start_task, None);
        assert!(view.unimplemented);
        let reason = view.unimplemented_reason.as_deref().unwrap();
        assert!(reason.contains("解決"), "reason: {reason}");
    }

    #[test]
    fn test_queue_order_rows_flags_unimplemented_for_gray() {
        let defs = parse_all(&[("launch.toml", LAUNCH_TOML), ("login.toml", LOGIN_TOML)]);
        let selected = vec!["login".to_string(), "launch".to_string()];
        let rows = queue_order_rows(&selected, &defs);
        assert_eq!(rows.len(), 2);
        assert!(!rows[0].runnable);
        assert_eq!(rows[0].title, "ログイン");
        assert!(rows[1].runnable);
    }

    #[test]
    fn test_queue_order_rows_skips_unknown_selected_id() {
        let defs = parse_all(&[("launch.toml", LAUNCH_TOML)]);
        let selected = vec!["ghost".to_string(), "launch".to_string()];
        let rows = queue_order_rows(&selected, &defs);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "launch");
        assert_eq!(rows[0].position, 2);
    }
}
