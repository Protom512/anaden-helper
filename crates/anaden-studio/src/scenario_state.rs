use std::path::{Path, PathBuf};

use anaden_core::{Goal, StopCondition};
use anaden_vision::{PipelineManifest, TaskDef};

#[derive(Debug, Clone)]
pub struct ScenarioEditorState {
    pub name: String,
    pub start_task: String,
    pub goals: Vec<Goal>,
    pub watch: Vec<String>,
    pub tasks: Vec<TaskDef>,
    pub loaded_task_names: Vec<String>,
    pub loaded_task_files: Vec<(String, String)>,
    pub loaded_from: Option<std::path::PathBuf>,
}

impl ScenarioEditorState {
    #[must_use]
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            start_task: String::new(),
            goals: Vec::new(),
            watch: Vec::new(),
            tasks: Vec::new(),
            loaded_task_names: Vec::new(),
            loaded_task_files: Vec::new(),
            loaded_from: None,
        }
    }

    pub fn add_task(&mut self, task: TaskDef) {
        if self.start_task.is_empty() {
            self.start_task = task.name.clone();
        }
        self.tasks.push(task);
    }

    pub fn remove_task(&mut self, name: &str) -> bool {
        let Some(pos) = self.tasks.iter().position(|t| t.name == name) else {
            return false;
        };
        let removed = self.tasks.remove(pos);
        if self.start_task == removed.name {
            self.start_task = self
                .tasks
                .first()
                .map_or_else(String::new, |t| t.name.clone());
        }
        true
    }

    pub fn set_start_task(&mut self, name: &str) -> bool {
        if self.task(name).is_some() {
            self.start_task = name.to_string();
            true
        } else {
            false
        }
    }

    #[must_use]
    pub fn task(&self, name: &str) -> Option<&TaskDef> {
        self.tasks.iter().find(|t| t.name == name)
    }

    pub fn task_mut(&mut self, name: &str) -> Option<&mut TaskDef> {
        self.tasks.iter_mut().find(|t| t.name == name)
    }

    #[must_use]
    pub fn task_names(&self) -> Vec<&str> {
        self.tasks.iter().map(|t| t.name.as_str()).collect()
    }

    pub fn add_goal(&mut self, goal: Goal) {
        self.goals.push(goal);
    }

    pub fn remove_goal(&mut self, index: usize) -> bool {
        if index < self.goals.len() {
            self.goals.remove(index);
            true
        } else {
            false
        }
    }

    pub fn update_goal_stop(&mut self, index: usize, stop: StopCondition) -> bool {
        if let Some(goal) = self.goals.get_mut(index) {
            goal.stop = stop;
            true
        } else {
            false
        }
    }

    pub fn assign_template(&mut self, task_name: &str, png: &Path, pipeline_dir: &Path) -> bool {
        let reference = resolve_template_reference(png, pipeline_dir);
        match self.task_mut(task_name) {
            Some(task) => {
                task.template = PathBuf::from(reference);
                true
            }
            None => false,
        }
    }

    #[must_use]
    pub fn to_manifest(&self) -> PipelineManifest {
        PipelineManifest {
            start_task: self.start_task.clone(),
            goals: self.goals.clone(),
            watch: self.watch.clone(),
        }
    }
}

#[must_use]
pub fn resolve_template_reference(png: &Path, pipeline_dir: &Path) -> String {
    let forward = |p: &Path| p.to_string_lossy().replace('\\', "/");
    if !png.is_absolute() || pipeline_dir.as_os_str().is_empty() {
        return forward(png);
    }
    let target: Vec<_> = png.components().collect();
    let base: Vec<_> = pipeline_dir.components().collect();
    let common = target
        .iter()
        .zip(base.iter())
        .take_while(|(t, b)| t == b)
        .count();
    if common == 0 {
        return forward(png);
    }
    let mut rel = PathBuf::new();
    for _ in common..base.len() {
        rel.push("..");
    }
    for component in &target[common..] {
        rel.push(component.as_os_str());
    }
    if rel.as_os_str().is_empty() {
        return forward(png);
    }
    forward(&rel)
}

pub(crate) fn goal_summary(stop: &StopCondition) -> String {
    match stop {
        StopCondition::LoopCount { target } => format!("ループ {target} 回"),
        StopCondition::TemplateMatch { task, confidence } => {
            format!("テンプレ {task} @ {confidence:.2}")
        }
        StopCondition::Timeout { secs } => format!("{secs} 秒"),
        StopCondition::All { .. } => "ALL 合成".to_string(),
        StopCondition::Any { .. } => "ANY 合成".to_string(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::scenario_validate::ScenarioValidationError;
    use anaden_vision::{Action, Algorithm};

    fn task_def(name: &str, next: Option<Vec<&str>>) -> TaskDef {
        TaskDef {
            name: name.to_string(),
            state: "Field".to_string(),
            algorithm: Algorithm::Ccoeff,
            template: PathBuf::from(format!("{}.png", name.to_lowercase())),
            roi: Some([10, 20, 100, 50]),
            threshold: 0.8,
            base: None,
            action: Some(Action::ClickSelf),
            next: next.map(|v| v.iter().map(|s| s.to_string()).collect()),
        }
    }

    fn loop_goal(name: &str, target: u64) -> Goal {
        Goal {
            name: name.to_string(),
            stop: StopCondition::LoopCount { target },
        }
    }

    fn pipelines_root() -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(r"C:\work\templates\pipelines")
        } else {
            PathBuf::from("/work/templates/pipelines")
        }
    }

    #[test]
    fn new_state_starts_empty_and_reports_no_tasks() {
        let st = ScenarioEditorState::new("neko_nikki");
        assert!(st.tasks.is_empty());
        assert!(st.goals.is_empty());
        assert_eq!(st.start_task, "");
        assert!(matches!(
            st.validate(),
            Err(ScenarioValidationError::NoTasks)
        ));
    }

    #[test]
    fn add_task_adopts_first_task_as_start_and_keeps_it() {
        let mut st = ScenarioEditorState::new("s");
        st.add_task(task_def("First", None));
        assert_eq!(st.start_task, "First");
        st.add_task(task_def("Second", None));
        assert_eq!(st.start_task, "First");
        assert_eq!(st.task_names(), vec!["First", "Second"]);
    }

    #[test]
    fn remove_task_fixes_up_start_task() {
        let mut st = ScenarioEditorState::new("s");
        st.add_task(task_def("A", None));
        st.add_task(task_def("B", None));
        assert!(st.remove_task("A"));
        assert_eq!(st.start_task, "B");
        assert_eq!(st.task_names(), vec!["B"]);
        assert!(st.remove_task("B"));
        assert_eq!(st.start_task, "");
        assert!(!st.remove_task("B"));
    }

    #[test]
    fn set_start_task_accepts_only_known_names() {
        let mut st = ScenarioEditorState::new("s");
        st.add_task(task_def("A", None));
        assert!(!st.set_start_task("Ghost"));
        assert_eq!(st.start_task, "A");
        st.start_task = String::new();
        assert!(st.set_start_task("A"));
        assert_eq!(st.start_task, "A");
    }

    #[test]
    fn task_mut_enables_full_action_editing_including_swipe() {
        let mut st = ScenarioEditorState::new("s");
        st.add_task(task_def("A", None));
        let Some(t) = st.task_mut("A") else {
            panic!("task A must exist");
        };
        t.action = Some(Action::Swipe {
            from: anaden_core::ScreenRegion::new(100, 100, 30, 30),
            to: anaden_core::ScreenRegion::new(200, 300, 30, 30),
        });
        t.threshold = 0.7;
        let t = st.task("A").expect("A");
        assert!(matches!(t.action, Some(Action::Swipe { .. })));
        assert!((t.threshold - 0.7).abs() < 1e-6);
        assert!(st.task_mut("Ghost").is_none());
    }

    #[test]
    fn goal_editing_preserves_name_and_remove_goal() {
        let mut st = ScenarioEditorState::new("s");
        st.add_goal(loop_goal("g", 5));
        assert!(st.update_goal_stop(0, StopCondition::Timeout { secs: 60 }));
        assert_eq!(st.goals[0].name, "g");
        assert_eq!(st.goals[0].stop, StopCondition::Timeout { secs: 60 });
        assert!(!st.update_goal_stop(9, StopCondition::LoopCount { target: 1 }));
        assert!(st.remove_goal(0));
        assert!(st.goals.is_empty());
        assert!(!st.remove_goal(0));
    }

    #[test]
    fn resolve_template_reference_inside_pipeline_dir_is_bare_relative() {
        let dir = pipelines_root().join("fishing");
        let png = dir.join("scenes").join("title.png");
        assert_eq!(resolve_template_reference(&png, &dir), "scenes/title.png");
    }

    #[test]
    fn resolve_template_reference_from_sibling_dir_yields_parent_jump() {
        let root = pipelines_root();
        let png = root.join("field_loop_pc").join("hud_tr.png");
        let dir = root.join("fishing");
        assert_eq!(
            resolve_template_reference(&png, &dir),
            "../field_loop_pc/hud_tr.png"
        );
    }

    #[test]
    fn resolve_template_reference_normalizes_separator_for_relative_input() {
        let rel = if cfg!(windows) {
            "scenes\\title.png"
        } else {
            "scenes/title.png"
        };
        assert_eq!(
            resolve_template_reference(Path::new(rel), Path::new("ignored")),
            "scenes/title.png"
        );
    }

    #[cfg(windows)]
    #[test]
    fn resolve_template_reference_without_common_ancestor_keeps_absolute() {
        let png = Path::new(r"D:\library\title.png");
        let dir = Path::new(r"C:\work\templates\pipelines\my");
        assert_eq!(resolve_template_reference(png, dir), "D:/library/title.png");
    }

    #[test]
    fn assign_template_sets_forward_slash_relative_reference() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().join("my_pipeline");
        let png = dir.join("scenes").join("btn.png");
        let mut st = ScenarioEditorState::new("my_pipeline");
        st.add_task(task_def("Start", Some(vec![])));
        assert!(st.assign_template("Start", &png, &dir));
        let t = st.task("Start").expect("Start");
        assert_eq!(t.template, PathBuf::from("scenes/btn.png"));
        assert!(!st.assign_template("Ghost", &png, &dir));
    }

    #[test]
    fn to_manifest_carries_start_task_and_goals() {
        let mut st = ScenarioEditorState::new("s");
        st.add_task(task_def("A", None));
        st.add_goal(loop_goal("g", 7));
        let m = st.to_manifest();
        assert_eq!(m.start_task, "A");
        assert_eq!(m.goals, vec![loop_goal("g", 7)]);
    }
}
