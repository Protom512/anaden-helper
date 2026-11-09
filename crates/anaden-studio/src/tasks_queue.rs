use std::collections::BTreeMap;
use std::path::Path;

use crate::history::RunOutcome;
use crate::tasks_def::{TaskDefinition, TaskError, TaskKind, load_task_definitions};

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TaskQueue {
    selected: Vec<String>,
}

impl TaskQueue {
    pub fn new() -> Self {
        Self {
            selected: Vec::new(),
        }
    }

    pub fn toggle(&mut self, def: &TaskDefinition) -> Result<(), TaskError> {
        if !def.is_selectable() {
            return Err(TaskError::NotImplemented { id: def.id.clone() });
        }
        if let Some(pos) = self.selected.iter().position(|id| id == &def.id) {
            self.selected.remove(pos);
        } else {
            self.selected.push(def.id.clone());
        }
        Ok(())
    }

    pub fn selected_ids(&self) -> &[String] {
        &self.selected
    }

    pub fn build(&self, defs: &[TaskDefinition]) -> Result<Vec<TaskDefinition>, TaskError> {
        let by_id: BTreeMap<&str, &TaskDefinition> =
            defs.iter().map(|d| (d.id.as_str(), d)).collect();
        self.selected
            .iter()
            .map(|id| {
                by_id
                    .get(id.as_str())
                    .copied()
                    .cloned()
                    .ok_or_else(|| TaskError::UnknownTask(id.clone()))
            })
            .collect()
    }

    pub fn clear(&mut self) {
        self.selected.clear();
    }
}

#[derive(Debug, Default, Clone)]
pub struct TaskListState {
    defs: Vec<TaskDefinition>,
    queue: TaskQueue,
}

impl TaskListState {
    pub fn load(dir: &Path) -> Result<Self, TaskError> {
        Ok(Self {
            defs: load_task_definitions(dir)?,
            queue: TaskQueue::new(),
        })
    }

    pub fn definitions(&self) -> &[TaskDefinition] {
        &self.defs
    }

    pub fn find(&self, id: &str) -> Option<&TaskDefinition> {
        self.defs.iter().find(|d| d.id == id)
    }

    pub fn is_selected(&self, id: &str) -> bool {
        self.queue.selected_ids().iter().any(|s| s == id)
    }

    pub fn selected_count(&self) -> usize {
        self.queue.selected_ids().len()
    }

    pub fn selected_ids(&self) -> &[String] {
        self.queue.selected_ids()
    }

    pub fn toggle(&mut self, id: &str) -> Result<(), TaskError> {
        let def = self
            .find(id)
            .ok_or_else(|| TaskError::UnknownTask(id.to_string()))?
            .clone();
        self.queue.toggle(&def)
    }

    pub fn spawn_specs(
        &self,
        program: &str,
        target: &str,
        serial: Option<&str>,
        root: &Path,
    ) -> Result<Vec<crate::childproc::SpawnSpec>, TaskError> {
        Ok(self
            .queue_entries(program, target, serial, root)?
            .into_iter()
            .map(|e| e.spec)
            .collect())
    }

    pub fn queue_entries(
        &self,
        program: &str,
        target: &str,
        serial: Option<&str>,
        root: &Path,
    ) -> Result<Vec<QueueEntry>, TaskError> {
        let built = self.queue.build(&self.defs)?;
        if built.is_empty() {
            return Ok(Vec::new());
        }
        built
            .iter()
            .map(|def| {
                let mut args = spawn_args(def, target, serial, root);
                if args.is_empty() {
                    return Err(TaskError::UnknownTask(format!(
                        "タスク {} の実行引数を解決できません (pipeline_dir/start_task 不明)",
                        def.id
                    )));
                }
                if matches!(def.kind, TaskKind::PipelineRun) {
                    args = crate::runner_exec::with_report_file_arg(
                        &args,
                        &crate::history::new_report_path(),
                    );
                }
                Ok(QueueEntry {
                    label: def.title.clone(),
                    spec: crate::childproc::SpawnSpec::new(program, args),
                })
            })
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueEntry {
    pub label: String,
    pub spec: crate::childproc::SpawnSpec,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueueState {
    Pending,
    Running {
        current: usize,
    },
    PausedAfterFailure {
        current: usize,
        exit_code: Option<i32>,
    },
    PausedAfterBudgetExhausted {
        current: usize,
    },
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueueAction {
    Start(crate::childproc::SpawnSpec),
    WaitForExit,
    QueueCompleted,
    Noop,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueExec {
    state: QueueState,
    entries: Vec<QueueEntry>,
    aborted: bool,
}

impl QueueExec {
    pub fn new(entries: Vec<QueueEntry>) -> Self {
        Self {
            state: QueueState::Pending,
            entries,
            aborted: false,
        }
    }

    pub fn state(&self) -> &QueueState {
        &self.state
    }

    pub fn total(&self) -> usize {
        self.entries.len()
    }

    pub fn entries(&self) -> &[QueueEntry] {
        &self.entries
    }

    pub fn current_entry(&self) -> Option<&QueueEntry> {
        match self.state {
            QueueState::Running { current }
            | QueueState::PausedAfterFailure { current, .. }
            | QueueState::PausedAfterBudgetExhausted { current } => self.entries.get(current),
            QueueState::Pending | QueueState::Completed => None,
        }
    }

    pub fn start(&mut self) -> QueueAction {
        if !matches!(self.state, QueueState::Pending) {
            return QueueAction::Noop;
        }
        let Some(first) = self.entries.first() else {
            return QueueAction::Noop;
        };
        self.state = QueueState::Running { current: 0 };
        QueueAction::Start(first.spec.clone())
    }

    pub fn on_exit(&mut self, code: Option<i32>) -> QueueAction {
        let outcome = if code == Some(0) {
            RunOutcome::Success
        } else {
            RunOutcome::Failed
        };
        self.on_exit_classified(outcome, code)
    }

    pub fn on_exit_classified(
        &mut self,
        outcome: RunOutcome,
        exit_code: Option<i32>,
    ) -> QueueAction {
        let QueueState::Running { current } = self.state else {
            return QueueAction::Noop;
        };
        match outcome {
            RunOutcome::Success => self.advance_from(current),
            RunOutcome::BudgetExhausted => {
                self.state = QueueState::PausedAfterBudgetExhausted { current };
                QueueAction::WaitForExit
            }
            RunOutcome::Failed | RunOutcome::Cancelled | RunOutcome::Unknown => {
                self.state = QueueState::PausedAfterFailure { current, exit_code };
                QueueAction::WaitForExit
            }
        }
    }

    pub fn resume(&mut self) -> QueueAction {
        let current = match self.state {
            QueueState::PausedAfterFailure { current, .. }
            | QueueState::PausedAfterBudgetExhausted { current } => current,
            QueueState::Pending | QueueState::Running { .. } | QueueState::Completed => {
                return QueueAction::Noop;
            }
        };
        self.advance_from(current)
    }

    pub fn abort(&mut self) {
        self.aborted = true;
        self.state = QueueState::Completed;
        self.entries.clear();
    }

    pub fn is_aborted(&self) -> bool {
        self.aborted
    }

    #[must_use]
    pub fn summary(&self) -> String {
        if self.aborted {
            return "中止".to_string();
        }
        match &self.state {
            QueueState::Pending => format!("待機: {} タスク", self.entries.len()),
            QueueState::Running { current } => format!(
                "実行中 {}/{}: {}",
                current + 1,
                self.entries.len(),
                self.entries
                    .get(*current)
                    .map(|e| e.label.as_str())
                    .unwrap_or("?")
            ),
            QueueState::PausedAfterFailure { current, exit_code } => {
                let code_disp = match exit_code {
                    Some(c) => format!("exit={c}"),
                    None => "exit=不明".to_string(),
                };
                format!(
                    "失敗停止 {}/{} ({code_disp}): {} — 継続/停止を選択",
                    current + 1,
                    self.entries.len(),
                    self.entries
                        .get(*current)
                        .map(|e| e.label.as_str())
                        .unwrap_or("?")
                )
            }
            QueueState::PausedAfterBudgetExhausted { current } => {
                format!(
                    "予算切れ停止 {}/{} (成果ゼロ打ち切り): {} — 継続/停止を選択",
                    current + 1,
                    self.entries.len(),
                    self.entries
                        .get(*current)
                        .map(|e| e.label.as_str())
                        .unwrap_or("?")
                )
            }
            QueueState::Completed => {
                format!("完了 {}/{}", self.entries.len(), self.entries.len())
            }
        }
    }

    #[must_use]
    pub fn entry_marker(&self, index: usize) -> &'static str {
        match &self.state {
            QueueState::Pending => "待機",
            QueueState::Completed => "完了",
            QueueState::Running { current } => {
                if index < *current {
                    "完了"
                } else if index == *current {
                    "実行中"
                } else {
                    "待機"
                }
            }
            QueueState::PausedAfterFailure { current, .. } => {
                if index < *current {
                    "完了"
                } else if index == *current {
                    "失敗"
                } else {
                    "待機"
                }
            }
            QueueState::PausedAfterBudgetExhausted { current } => {
                if index < *current {
                    "完了"
                } else if index == *current {
                    "予算切れ"
                } else {
                    "待機"
                }
            }
        }
    }

    fn advance_from(&mut self, current: usize) -> QueueAction {
        match self.entries.get(current + 1) {
            Some(next) => {
                self.state = QueueState::Running {
                    current: current + 1,
                };
                QueueAction::Start(next.spec.clone())
            }
            None => {
                self.state = QueueState::Completed;
                QueueAction::QueueCompleted
            }
        }
    }
}

pub fn resolve_start_task(pipeline_dir: &Path) -> Option<String> {
    let entries = std::fs::read_dir(pipeline_dir).ok()?;
    let mut stems: Vec<String> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension().and_then(|e| e.to_str()) == Some("toml")
                && p.file_stem().and_then(|s| s.to_str()) != Some("pipeline")
        })
        .filter_map(|p| p.file_stem().and_then(|s| s.to_str()).map(String::from))
        .collect();
    stems.sort();
    stems.into_iter().next()
}

pub fn resolve_start_task_name(pipeline_dir: &Path) -> Option<String> {
    let manifest_path = pipeline_dir.join(anaden_vision::PIPELINE_MANIFEST_FILENAME);
    if manifest_path.is_file() {
        return anaden_vision::load_pipeline_manifest(pipeline_dir)
            .ok()
            .map(|m| m.start_task);
    }
    let first_stem = resolve_start_task(pipeline_dir)?;
    let content = std::fs::read_to_string(pipeline_dir.join(format!("{first_stem}.toml"))).ok()?;
    let value: toml::Value = toml::from_str(&content).ok()?;
    value.get("name").and_then(|v| v.as_str()).map(String::from)
}

pub fn spawn_args(
    def: &TaskDefinition,
    _target: &str,
    _serial: Option<&str>,
    root: &Path,
) -> Vec<String> {
    match def.kind {
        TaskKind::LaunchSubcommand => vec!["launch".to_string()],
        TaskKind::PipelineRun => {
            let Some(dir) = &def.pipeline_dir else {
                return Vec::new();
            };
            let abs = root.join(dir);
            let Some(start) = def
                .start_task
                .clone()
                .or_else(|| resolve_start_task_name(&abs))
            else {
                return Vec::new();
            };
            vec!["run".to_string(), abs.to_string_lossy().into_owned(), start]
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::tasks_def::test_util::{FIELD_LOOP_TOML, LAUNCH_TOML, LOGIN_TOML};

    fn parse_all(sources: &[(&str, &str)]) -> Vec<TaskDefinition> {
        sources
            .iter()
            .map(|(name, src)| TaskDefinition::parse_toml(src, Path::new(name)).unwrap())
            .collect()
    }

    #[test]
    fn test_builds_queue_in_check_order() {
        let defs = parse_all(&[
            ("launch.toml", LAUNCH_TOML),
            ("field_loop_pc.toml", FIELD_LOOP_TOML),
        ]);
        let mut queue = TaskQueue::new();
        queue.toggle(&defs[0]).unwrap();
        queue.toggle(&defs[1]).unwrap();
        let built = queue.build(&defs).unwrap();
        let ids: Vec<&str> = built.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(ids, vec!["launch", "field_loop_pc"]);
        assert_eq!(built[0].kind, TaskKind::LaunchSubcommand);
        assert_eq!(built[1].kind, TaskKind::PipelineRun);
    }

    #[test]
    fn test_toggle_off_removes_from_queue() {
        let defs = parse_all(&[
            ("launch.toml", LAUNCH_TOML),
            ("field_loop_pc.toml", FIELD_LOOP_TOML),
        ]);
        let mut queue = TaskQueue::new();
        queue.toggle(&defs[0]).unwrap();
        queue.toggle(&defs[1]).unwrap();
        queue.toggle(&defs[0]).unwrap();
        assert_eq!(queue.selected_ids(), &["field_loop_pc".to_string()]);
        assert_eq!(queue.build(&defs).unwrap().len(), 1);
    }

    #[test]
    fn test_not_implemented_task_cannot_be_toggled() {
        let defs = parse_all(&[("login.toml", LOGIN_TOML)]);
        assert!(!defs[0].is_selectable());
        let mut queue = TaskQueue::new();
        let err = queue.toggle(&defs[0]).unwrap_err();
        assert!(
            matches!(err, TaskError::NotImplemented { ref id } if id == "login"),
            "unexpected: {err:?}"
        );
        assert!(queue.selected_ids().is_empty());
    }

    #[test]
    fn test_build_with_unknown_selected_id_fails_closed() {
        let defs = parse_all(&[("launch.toml", LAUNCH_TOML)]);
        let mut queue = TaskQueue::new();
        queue.toggle(&defs[0]).unwrap();
        queue.selected.push("ghost".to_string());
        let err = queue.build(&defs).unwrap_err();
        assert!(matches!(err, TaskError::UnknownTask(ref id) if id == "ghost"));
    }

    #[test]
    fn test_spawn_args_launch_subcommand_windows_no_serial() {
        let def = TaskDefinition::parse_toml(LAUNCH_TOML, Path::new("launch.toml")).unwrap();
        let args = spawn_args(&def, "windows", Some("ignored"), Path::new("/root"));
        assert_eq!(args, vec!["launch"]);
    }

    #[test]
    fn test_spawn_args_pipeline_run_with_declared_start_task() {
        let def =
            TaskDefinition::parse_toml(FIELD_LOOP_TOML, Path::new("field_loop_pc.toml")).unwrap();
        let args = spawn_args(&def, "windows", None, Path::new("/root"));
        assert_eq!(
            args,
            vec!["run", "/root\\templates/pipelines/field_loop_pc", "start"]
        );
    }

    #[test]
    fn test_resolve_start_task_picks_first_taskdef_stem() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("pipeline.toml"), "manifest = true").unwrap();
        std::fs::write(tmp.path().join("zz_last.toml"), "x = 1").unwrap();
        std::fs::write(tmp.path().join("aa_first.toml"), "x = 1").unwrap();
        assert_eq!(resolve_start_task(tmp.path()).as_deref(), Some("aa_first"));
        assert_eq!(resolve_start_task(Path::new("nonexistent-xyz")), None);
    }

    #[test]
    fn test_spawn_args_pipeline_run_resolves_start_task_from_dir() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
        let src = r#"
id = "nav_to_field_pc"
title = "マップ移動"
kind = "pipeline_run"
implemented = true
pipeline_dir = "templates/pipelines/nav_to_field_pc"
"#;
        let def = TaskDefinition::parse_toml(src, Path::new("nav_to_field_pc.toml")).unwrap();
        let args = spawn_args(&def, "windows", None, &root);
        assert_eq!(args.len(), 3);
        assert_eq!(args[0], "run");
        assert!(
            args[1].ends_with("templates\\pipelines\\nav_to_field_pc")
                || args[1].ends_with("templates/pipelines/nav_to_field_pc")
        );
        assert_eq!(args[2], "TapToStartPc");
    }

    #[test]
    fn test_spawn_args_pipeline_run_login_manifest_start_task() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
        let src = r#"
id = "login"
title = "ログイン"
kind = "pipeline_run"
implemented = true
pipeline_dir = "templates/pipelines/login"
"#;
        let def = TaskDefinition::parse_toml(src, Path::new("login.toml")).unwrap();
        let args = spawn_args(&def, "windows", None, &root);
        assert_eq!(args.len(), 3);
        assert_eq!(args[0], "run");
        assert!(
            args[1].ends_with("templates\\pipelines\\login")
                || args[1].ends_with("templates/pipelines/login")
        );
        assert_eq!(args[2], "LoginTapTitlePc");
    }

    #[test]
    fn test_spawn_args_pipeline_run_manifest_start_task_wins_over_stems() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("pipeline.toml"),
            "start_task = \"FromManifest\"",
        )
        .unwrap();
        std::fs::write(tmp.path().join("aa_first.toml"), "name = \"AaTaskName\"\n").unwrap();
        assert_eq!(
            resolve_start_task_name(tmp.path()).as_deref(),
            Some("FromManifest")
        );
    }

    #[test]
    fn test_resolve_start_task_name_manifestless_uses_taskdef_name() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("aa_first.toml"),
            "name = \"AaTaskName\"\ntemplate = \"x.png\"\n",
        )
        .unwrap();
        std::fs::write(
            tmp.path().join("zz_last.toml"),
            "name = \"ZzTaskName\"\ntemplate = \"y.png\"\n",
        )
        .unwrap();
        assert_eq!(
            resolve_start_task_name(tmp.path()).as_deref(),
            Some("AaTaskName")
        );
    }

    #[test]
    fn test_resolve_start_task_name_manifestless_nameless_toml_is_none() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("aa_first.toml"), "x = 1\n").unwrap();
        assert_eq!(resolve_start_task_name(tmp.path()), None);
    }

    #[test]
    fn test_resolve_start_task_name_broken_manifest_fails_closed() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("pipeline.toml"), "not valid toml =").unwrap();
        std::fs::write(tmp.path().join("aa_first.toml"), "name = \"AaTaskName\"\n").unwrap();
        assert_eq!(resolve_start_task_name(tmp.path()), None);
    }

    #[test]
    fn test_spawn_args_pipeline_run_missing_dir_and_task_returns_empty() {
        let src = r#"
id = "ghost"
title = "G"
kind = "pipeline_run"
implemented = true
pipeline_dir = "templates/pipelines/nonexistent-xyz"
"#;
        let def = TaskDefinition::parse_toml(src, Path::new("ghost.toml")).unwrap();
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
        assert!(spawn_args(&def, "windows", None, &root).is_empty());
    }

    fn queue_entry(label: &str, arg0: &str) -> QueueEntry {
        QueueEntry {
            label: label.to_string(),
            spec: crate::childproc::SpawnSpec::new("anaden", vec![arg0.to_string()]),
        }
    }

    #[test]
    fn test_queue_start_returns_first_entry_in_check_order() {
        let mut q = QueueExec::new(vec![
            queue_entry("ゲーム起動", "launch"),
            queue_entry("周回", "run"),
        ]);
        assert!(matches!(q.state(), QueueState::Pending));
        let action = q.start();
        let QueueAction::Start(spec) = action else {
            panic!("start must return Start, got {action:?}");
        };
        assert_eq!(spec.args[0], "launch");
        assert!(matches!(q.state(), QueueState::Running { current: 0 }));
        assert_eq!(q.total(), 2);
    }

    #[test]
    fn test_queue_exit_zero_starts_next_task() {
        let mut q = QueueExec::new(vec![queue_entry("A", "launch"), queue_entry("B", "run")]);
        let _ = q.start();
        let QueueAction::Start(spec) = q.on_exit(Some(0)) else {
            panic!("on_exit(0) must return Start");
        };
        assert_eq!(spec.args[0], "run");
        assert!(matches!(q.state(), QueueState::Running { current: 1 }));
    }

    #[test]
    fn test_queue_exit_zero_on_last_completes_queue() {
        let mut q = QueueExec::new(vec![queue_entry("A", "launch"), queue_entry("B", "run")]);
        let _ = q.start();
        let _ = q.on_exit(Some(0));
        assert!(matches!(q.on_exit(Some(0)), QueueAction::QueueCompleted));
        assert!(matches!(q.state(), QueueState::Completed));
    }

    #[test]
    fn test_queue_resume_after_failure_starts_next() {
        let mut q = QueueExec::new(vec![queue_entry("A", "launch"), queue_entry("B", "run")]);
        let _ = q.start();
        let _ = q.on_exit(Some(1));
        let QueueAction::Start(spec) = q.resume() else {
            panic!("resume must return Start");
        };
        assert_eq!(spec.args[0], "run");
        assert!(matches!(q.state(), QueueState::Running { current: 1 }));
    }

    #[test]
    fn test_queue_resume_on_last_failure_completes_queue() {
        let mut q = QueueExec::new(vec![queue_entry("A", "launch")]);
        let _ = q.start();
        let _ = q.on_exit(Some(1));
        assert!(matches!(q.resume(), QueueAction::QueueCompleted));
        assert!(matches!(q.state(), QueueState::Completed));
    }

    #[test]
    fn test_queue_summary_shows_progress() {
        let mut q = QueueExec::new(vec![
            queue_entry("周回", "run"),
            queue_entry("起動", "launch"),
        ]);
        assert_eq!(q.summary(), "待機: 2 タスク");
        let _ = q.start();
        assert!(q.summary().contains("1/2"), "summary: {}", q.summary());
        assert!(q.summary().contains("周回"), "summary: {}", q.summary());
        let _ = q.on_exit(Some(0));
        assert!(q.summary().contains("2/2"), "summary: {}", q.summary());
        let _ = q.on_exit(Some(3));
        assert!(q.summary().contains("exit=3"), "summary: {}", q.summary());
        let _ = q.resume();
        assert_eq!(q.summary(), "完了 2/2");
    }

    #[test]
    fn test_queue_entry_marker_progression() {
        let mut q = QueueExec::new(vec![queue_entry("A", "a"), queue_entry("B", "b")]);
        assert_eq!(q.entry_marker(0), "待機");
        let _ = q.start();
        assert_eq!(q.entry_marker(0), "実行中");
        assert_eq!(q.entry_marker(1), "待機");
        let _ = q.on_exit(Some(0));
        assert_eq!(q.entry_marker(0), "完了");
        assert_eq!(q.entry_marker(1), "実行中");
        let _ = q.on_exit(Some(1));
        assert_eq!(q.entry_marker(1), "失敗");
        let _ = q.resume();
        assert_eq!(q.entry_marker(0), "完了");
        assert_eq!(q.entry_marker(1), "完了");
    }

    #[test]
    fn test_queue_start_empty_is_rejected() {
        let mut q = QueueExec::new(Vec::new());
        assert!(matches!(q.start(), QueueAction::Noop));
        assert!(matches!(q.state(), QueueState::Pending));
        assert_eq!(q.total(), 0);
    }

    #[test]
    fn test_queue_start_while_running_is_rejected() {
        let mut q = QueueExec::new(vec![queue_entry("A", "a")]);
        let _ = q.start();
        assert!(matches!(q.start(), QueueAction::Noop));
        assert!(matches!(q.state(), QueueState::Running { current: 0 }));
        let _ = q.on_exit(Some(1));
        assert!(matches!(q.start(), QueueAction::Noop));
        let _ = q.resume();
        assert!(matches!(q.start(), QueueAction::Noop));
    }

    #[test]
    fn test_queue_failure_pauses_without_auto_continue() {
        let mut q = QueueExec::new(vec![queue_entry("A", "a"), queue_entry("B", "b")]);
        let _ = q.start();
        assert!(matches!(q.on_exit(Some(2)), QueueAction::WaitForExit));
        assert!(matches!(
            q.state(),
            QueueState::PausedAfterFailure {
                current: 0,
                exit_code: Some(2)
            }
        ));
    }

    #[test]
    fn test_queue_exit_none_code_pauses() {
        let mut q = QueueExec::new(vec![queue_entry("A", "a")]);
        let _ = q.start();
        let _ = q.on_exit(None);
        assert!(matches!(
            q.state(),
            QueueState::PausedAfterFailure {
                current: 0,
                exit_code: None
            }
        ));
    }

    #[test]
    fn test_queue_on_exit_paused_is_noop() {
        let mut q = QueueExec::new(vec![queue_entry("A", "a")]);
        let _ = q.start();
        let _ = q.on_exit(Some(1));
        assert!(matches!(q.on_exit(Some(0)), QueueAction::Noop));
        assert!(matches!(q.state(), QueueState::PausedAfterFailure { .. }));
    }

    #[test]
    fn test_queue_budget_exhausted_pauses_without_auto_continue() {
        let mut q = QueueExec::new(vec![queue_entry("A", "a"), queue_entry("B", "b")]);
        let _ = q.start();
        assert!(matches!(
            q.on_exit_classified(crate::history::RunOutcome::BudgetExhausted, Some(0)),
            QueueAction::WaitForExit
        ));
        assert!(matches!(
            q.state(),
            QueueState::PausedAfterBudgetExhausted { current: 0 }
        ));
        let QueueAction::Start(spec) = q.resume() else {
            panic!("resume must return Start");
        };
        assert_eq!(spec.args[0], "b");
        assert!(matches!(q.state(), QueueState::Running { current: 1 }));
    }

    #[test]
    fn test_queue_classified_success_auto_continues() {
        let mut q = QueueExec::new(vec![queue_entry("A", "a"), queue_entry("B", "b")]);
        let _ = q.start();
        let QueueAction::Start(spec) =
            q.on_exit_classified(crate::history::RunOutcome::Success, Some(0))
        else {
            panic!("classified success must auto-continue");
        };
        assert_eq!(spec.args[0], "b");
    }

    #[test]
    fn test_queue_classified_failed_pauses_with_exit_code() {
        let mut q = QueueExec::new(vec![queue_entry("A", "a")]);
        let _ = q.start();
        assert!(matches!(
            q.on_exit_classified(crate::history::RunOutcome::Failed, Some(2)),
            QueueAction::WaitForExit
        ));
        assert!(matches!(
            q.state(),
            QueueState::PausedAfterFailure {
                current: 0,
                exit_code: Some(2)
            }
        ));
    }

    #[test]
    fn test_queue_budget_exhausted_summary_and_marker() {
        let mut q = QueueExec::new(vec![queue_entry("周回", "run"), queue_entry("次", "x")]);
        let _ = q.start();
        let _ = q.on_exit_classified(crate::history::RunOutcome::BudgetExhausted, Some(0));
        let summary = q.summary();
        assert!(summary.contains("予算切れ"), "summary: {summary}");
        assert!(summary.contains("1/2"), "summary: {summary}");
        assert_eq!(q.entry_marker(0), "予算切れ");
        assert_eq!(q.entry_marker(1), "待機");
    }

    #[test]
    fn test_queue_abort_discards_from_any_state() {
        let mut q = QueueExec::new(vec![queue_entry("A", "a"), queue_entry("B", "b")]);
        let _ = q.start();
        q.abort();
        assert!(matches!(q.state(), QueueState::Completed));
        assert!(q.entries().is_empty());
        assert!(q.is_aborted());
        assert_eq!(q.summary(), "中止");
        let mut p = QueueExec::new(vec![queue_entry("A", "a")]);
        p.abort();
        assert!(matches!(p.state(), QueueState::Completed));
    }

    #[test]
    fn test_task_list_state_builds_queue_entries_in_check_order() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
        let mut list = TaskListState::load(&root.join("templates/tasks")).unwrap();
        list.toggle("field_loop_pc").unwrap();
        list.toggle("launch").unwrap();
        let entries = list
            .queue_entries("anaden", "windows", None, &root)
            .unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].label, "フィールド周回");
        assert_eq!(entries[0].spec.args[0], "run");
        assert_eq!(entries[1].label, "ゲーム起動");
        assert_eq!(entries[1].spec.args[0], "launch");
    }

    #[test]
    fn test_queue_entries_pipeline_run_carries_report_file_launch_does_not() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("a_run.toml"),
            "id = \"r\"\ntitle = \"R\"\nkind = \"pipeline_run\"\nimplemented = true\npipeline_dir = \"templates/pipelines/missing-xyz\"\nstart_task = \"start\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("b_launch.toml"),
            "id = \"l\"\ntitle = \"L\"\nkind = \"launch_subcommand\"\nimplemented = true\n",
        )
        .unwrap();
        let mut list = TaskListState::load(dir.path()).unwrap();
        list.toggle("r").unwrap();
        list.toggle("l").unwrap();
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
        let entries = list
            .queue_entries("anaden", "windows", None, &root)
            .unwrap();
        assert_eq!(entries.len(), 2);
        let run = entries.iter().find(|e| e.label == "R").unwrap();
        let flag_at = run.spec.args.iter().position(|t| t == "--report-file");
        assert!(
            flag_at.is_some(),
            "pipeline_run args must carry --report-file: {:?}",
            run.spec.args
        );
        assert!(
            run.spec
                .args
                .get(flag_at.unwrap() + 1)
                .is_some_and(|p| !p.is_empty()),
            "report path must follow the flag: {:?}",
            run.spec.args
        );
        let launch = entries.iter().find(|e| e.label == "L").unwrap();
        assert_eq!(launch.spec.args, vec!["launch".to_string()]);
    }
}
