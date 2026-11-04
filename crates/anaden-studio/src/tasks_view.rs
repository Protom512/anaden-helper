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

    // ---- Issue #154 Shard 2 (UC-3): 表示モデル純関数 ----

    /// 正常系: launch_subcommand の詳細ビュー (windows・serial は使わない)。
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

    /// 正常系: pipeline_run 宣言済み start_task の詳細ビュー
    /// (pipeline_dir は root 結合済み・宣言値がそのまま使われる)。
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
        // Issue #188: --target 廃止で run <dir> <start> の 3 トークン。
        assert_eq!(view.spawn_args.len(), 3);
        assert_eq!(view.spawn_args.last().map(String::as_str), Some("start"));
        assert!(!view.unimplemented);
    }

    /// 正常系: start_task 未宣言タスクは resolve_start_task_name (manifest の
    /// start_task = TaskDef 名) の解決結果を詳細ビューへ反映する
    /// (リポジトリ実 pipeline で結合検証・DEF-006: stem でなく名前)。
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

    /// 正常系 (UC-3): 実行順位置ラベルはチェック順位置と総数を含む。
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

    /// 正常系 (UC-3): 実行順リスト行はチェック順に 1 始まりで番号付き。
    #[test]
    fn test_queue_order_rows_numbered_in_check_order() {
        let defs = parse_all(&[
            ("launch.toml", LAUNCH_TOML),
            ("field_loop_pc.toml", FIELD_LOOP_TOML),
        ]);
        // チェック順: field_loop_pc → launch (定義順と逆にチェック)。
        let selected = vec!["field_loop_pc".to_string(), "launch".to_string()];
        let rows = queue_order_rows(&selected, &defs);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].position, 1);
        assert_eq!(rows[0].title, "フィールド周回");
        assert_eq!(rows[1].position, 2);
        assert_eq!(rows[1].title, "ゲーム起動");
        assert!(rows.iter().all(|r| r.runnable));
    }

    /// エッジケース: 未実装タスク (implemented = false) は「未実装」ラベル用
    /// データを持つ (引数自体は解決可能でも implemented 優先)。
    #[test]
    fn test_detail_view_unimplemented_task_carries_label_data() {
        let defs = parse_all(&[("login.toml", LOGIN_TOML)]);
        let view = task_detail_view(&defs[0], "windows", None, Path::new("/root"));
        assert!(view.unimplemented);
        let reason = view.unimplemented_reason.as_deref().unwrap();
        assert!(reason.contains("implemented = false"), "reason: {reason}");
    }

    /// エッジケース: pipeline_dir が実在せず start_task も未宣言なら引数解決不能
    /// として fail-closed 表示 (実行時は queue_entries が拒否する)。
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

    /// エッジケース: 不整合 (未実装タスクが選択済み) でも行はグレー表示用に
    /// runnable = false で残る (fail-closed 表示)。
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

    /// エッジケース: 未知の選択 ID は表示から除外する (実行時 build が
    /// fail-closed で拒否するため表示側で番号がずれても誤実行はない)。
    #[test]
    fn test_queue_order_rows_skips_unknown_selected_id() {
        let defs = parse_all(&[("launch.toml", LAUNCH_TOML)]);
        let selected = vec!["ghost".to_string(), "launch".to_string()];
        let rows = queue_order_rows(&selected, &defs);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "launch");
        // 元の選択順位置を維持 (歯抜け番号 — 隠蔽しない)。
        assert_eq!(rows[0].position, 2);
    }
}
