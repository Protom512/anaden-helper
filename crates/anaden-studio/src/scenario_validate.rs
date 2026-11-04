use std::path::Path;

use anaden_core::GoalError;

use crate::scenario_state::ScenarioEditorState;

pub const SCREEN_WIDTH: u32 = 1258;
pub const SCREEN_HEIGHT: u32 = 708;

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ScenarioValidationError {
    #[error("scenario name must not be empty")]
    EmptyName,
    #[error("scenario name `{name}` is not a safe directory name")]
    UnsafeName { name: String },
    #[error("scenario must contain at least 1 task")]
    NoTasks,
    #[error("start_task must be set")]
    EmptyStartTask,
    #[error("start_task `{task}` does not match any TaskDef name")]
    UnknownStartTask { task: String },
    #[error("duplicate task name `{name}`")]
    DuplicateTaskName { name: String },
    #[error("task `{task}`: next reference `{next}` does not match any TaskDef name")]
    UnresolvedNext { task: String, next: String },
    #[error("goal[{index}] `{goal_name}` invalid: {source}")]
    GoalInvalid {
        index: usize,
        goal_name: String,
        #[source]
        source: GoalError,
    },
    #[error(
        "task `{task}`: roi {roi:?} exceeds screen {SCREEN_WIDTH}x{SCREEN_HEIGHT} or has zero size"
    )]
    RoiOutOfBounds { task: String, roi: [u32; 4] },
    #[error("task name `{name}` violates the pipeline node naming convention: {reason}")]
    TaskNaming { name: String, reason: String },
}

impl ScenarioEditorState {
    #[must_use]
    pub fn validation_issues(&self) -> Vec<ScenarioValidationError> {
        let mut issues = Vec::new();
        self.check_scenario_name(&mut issues);
        self.check_task_namespace(&mut issues);
        self.check_task_references(&mut issues);
        self.check_goals(&mut issues);
        issues
    }

    fn check_scenario_name(&self, issues: &mut Vec<ScenarioValidationError>) {
        if self.name.trim().is_empty() {
            issues.push(ScenarioValidationError::EmptyName);
        }
        let name = self.name.trim();
        if !name.is_empty() && Path::new(name).file_name() != Some(std::ffi::OsStr::new(name)) {
            issues.push(ScenarioValidationError::UnsafeName {
                name: self.name.clone(),
            });
        }
    }

    fn check_task_namespace(&self, issues: &mut Vec<ScenarioValidationError>) {
        if self.tasks.is_empty() {
            issues.push(ScenarioValidationError::NoTasks);
        }

        for (i, task) in self.tasks.iter().enumerate() {
            if self.tasks[..i].iter().any(|t| t.name == task.name) {
                issues.push(ScenarioValidationError::DuplicateTaskName {
                    name: task.name.clone(),
                });
            }
        }

        if self.start_task.trim().is_empty() {
            issues.push(ScenarioValidationError::EmptyStartTask);
        } else if !self.tasks.iter().any(|t| t.name == self.start_task) {
            issues.push(ScenarioValidationError::UnknownStartTask {
                task: self.start_task.clone(),
            });
        }
    }

    fn check_task_references(&self, issues: &mut Vec<ScenarioValidationError>) {
        for task in &self.tasks {
            let is_loaded = self.loaded_task_names.iter().any(|n| n == &task.name);
            if !is_loaded && let Some(reason) = crate::scenario_load::task_name_issue(&task.name) {
                issues.push(ScenarioValidationError::TaskNaming {
                    name: task.name.clone(),
                    reason: reason.to_string(),
                });
            }
            if let Some(nexts) = &task.next {
                for next in nexts {
                    if !self.tasks.iter().any(|t| &t.name == next) {
                        issues.push(ScenarioValidationError::UnresolvedNext {
                            task: task.name.clone(),
                            next: next.clone(),
                        });
                    }
                }
            }
            if let Some(roi) = task.roi
                && !is_loaded
                && !roi_within_screen(roi)
            {
                issues.push(ScenarioValidationError::RoiOutOfBounds {
                    task: task.name.clone(),
                    roi,
                });
            }
        }
    }

    fn check_goals(&self, issues: &mut Vec<ScenarioValidationError>) {
        for (index, goal) in self.goals.iter().enumerate() {
            if let Err(source) = goal.validate() {
                issues.push(ScenarioValidationError::GoalInvalid {
                    index,
                    goal_name: goal.name.clone(),
                    source,
                });
            }
        }
    }

    pub fn validate(&self) -> Result<(), ScenarioValidationError> {
        match self.validation_issues().into_iter().next() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}

fn roi_within_screen(roi: [u32; 4]) -> bool {
    let [x, y, w, h] = roi;
    w > 0 && h > 0 && x.saturating_add(w) <= SCREEN_WIDTH && y.saturating_add(h) <= SCREEN_HEIGHT
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use anaden_core::{Goal, StopCondition};
    use anaden_vision::{Action, Algorithm};
    use std::path::PathBuf;

    fn task_def(name: &str, next: Option<Vec<&str>>) -> anaden_vision::TaskDef {
        anaden_vision::TaskDef {
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

    #[test]
    fn validate_ok_for_complete_scenario() {
        let mut st = ScenarioEditorState::new("fishing2");
        st.add_task(task_def("Start", Some(vec!["Loop"])));
        st.add_task(anaden_vision::TaskDef {
            roi: None,
            ..task_def("Loop", Some(vec!["Start"]))
        });
        st.add_goal(loop_goal("loop50", 50));
        st.add_goal(Goal {
            name: "any".to_string(),
            stop: StopCondition::Any {
                conditions: vec![StopCondition::Timeout { secs: 3600 }],
            },
        });
        assert!(st.validation_issues().is_empty());
        assert!(st.validate().is_ok());
    }

    #[test]
    fn validate_flags_empty_name_no_tasks_empty_start() {
        let st = ScenarioEditorState::new("   ");
        let issues = st.validation_issues();
        assert!(issues.contains(&ScenarioValidationError::EmptyName));
        assert!(issues.contains(&ScenarioValidationError::NoTasks));
        assert!(issues.contains(&ScenarioValidationError::EmptyStartTask));
    }

    #[test]
    fn validate_flags_unsafe_directory_name() {
        for bad in ["a/b", "..", ".", "a\\b", "abc/"] {
            let mut st = ScenarioEditorState::new(bad);
            st.add_task(task_def("A", None));
            assert!(
                st.validation_issues()
                    .contains(&ScenarioValidationError::UnsafeName {
                        name: bad.to_string()
                    }),
                "name {bad:?} must be flagged unsafe"
            );
        }
        let mut ok = ScenarioEditorState::new("my_scenario");
        ok.add_task(task_def("A", None));
        assert!(
            !ok.validation_issues()
                .iter()
                .any(|i| matches!(i, ScenarioValidationError::UnsafeName { .. }))
        );
    }

    #[test]
    fn validate_flags_unknown_start_task() {
        let mut st = ScenarioEditorState::new("s");
        st.add_task(task_def("A", None));
        st.start_task = "Ghost".to_string();
        let issues = st.validation_issues();
        assert!(issues.contains(&ScenarioValidationError::UnknownStartTask {
            task: "Ghost".to_string()
        }));
    }

    #[test]
    fn validate_flags_unresolved_next_reference() {
        let mut st = ScenarioEditorState::new("s");
        st.add_task(task_def("Start", Some(vec!["Missing"])));
        let issues = st.validation_issues();
        assert!(issues.contains(&ScenarioValidationError::UnresolvedNext {
            task: "Start".to_string(),
            next: "Missing".to_string()
        }));
    }

    #[test]
    fn validate_flags_duplicate_task_names() {
        let mut st = ScenarioEditorState::new("s");
        st.add_task(task_def("A", None));
        st.add_task(task_def("A", None));
        let issues = st.validation_issues();
        assert!(
            issues.contains(&ScenarioValidationError::DuplicateTaskName {
                name: "A".to_string()
            })
        );
    }

    #[test]
    fn validate_delegates_to_goal_validate() {
        let mut st = ScenarioEditorState::new("s");
        st.add_task(task_def("A", None));
        st.add_goal(loop_goal("bad", 0));
        let issues = st.validation_issues();
        let expected = ScenarioValidationError::GoalInvalid {
            index: 0,
            goal_name: "bad".to_string(),
            source: GoalError::NonPositive { field: "target" },
        };
        assert!(issues.contains(&expected));
    }

    #[test]
    fn validate_flags_roi_out_of_bounds_and_zero_size() {
        let mut st = ScenarioEditorState::new("s");
        let mut edge = task_def("Edge", None);
        edge.roi = Some([1200, 600, 200, 200]);
        st.add_task(edge);
        let mut zero = task_def("Zero", None);
        zero.roi = Some([10, 10, 0, 40]);
        st.add_task(zero);
        let issues = st.validation_issues();
        assert!(issues.contains(&ScenarioValidationError::RoiOutOfBounds {
            task: "Edge".to_string(),
            roi: [1200, 600, 200, 200]
        }));
        assert!(issues.contains(&ScenarioValidationError::RoiOutOfBounds {
            task: "Zero".to_string(),
            roi: [10, 10, 0, 40]
        }));
    }

    #[test]
    fn naming_and_roi_checks_apply_only_to_non_baseline_tasks() {
        let mut st = ScenarioEditorState::new("field_loop");
        let mut loaded = task_def("TapHudTr", None);
        loaded.roi = Some([1080, 150, 180, 150]);
        st.add_task(loaded);
        st.loaded_task_names = vec!["TapHudTr".to_string()];
        assert!(
            st.validate().is_ok(),
            "baseline タスクは命名・ROI 検査の対象外: {:?}",
            st.validation_issues()
        );

        st.task_mut("TapHudTr").unwrap().name = "tap_hud_v2".to_string();
        assert!(
            st.validation_issues()
                .iter()
                .any(|i| matches!(i, ScenarioValidationError::TaskNaming { .. }))
        );

        st.task_mut("tap_hud_v2").unwrap().name = "FieldHudWidePc".to_string();
        assert!(
            st.validation_issues()
                .iter()
                .any(|i| matches!(i, ScenarioValidationError::RoiOutOfBounds { .. })),
            "baseline 外の ROI 超過は検出される"
        );
    }

    #[test]
    fn new_scenario_names_are_all_validated() {
        let mut st = ScenarioEditorState::new("s");
        st.add_task(task_def("Start", None));
        st.add_task(task_def("confirm_now", None));
        let issues = st.validation_issues();
        assert!(!issues.iter().any(|i| matches!(
            i,
            ScenarioValidationError::TaskNaming { name, .. } if name == "Start"
        )));
        assert!(
            issues.contains(&ScenarioValidationError::TaskNaming {
                name: "confirm_now".to_string(),
                reason: crate::scenario_load::task_name_issue("confirm_now")
                    .unwrap_or_default()
                    .to_string(),
            })
        );
    }
}
