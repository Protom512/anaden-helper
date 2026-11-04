use std::path::Path;

use image::DynamicImage;

use anaden_core::{Goal, ScreenRegion, StopCondition};
use anaden_vision::TaskDef;

use crate::app_state_pipeline_task::{PipelineActionKind, pipeline_task_spec};
use crate::library::{needle_roi_warning, template_structure_warning};
use crate::scenario_save::{ScenarioSaveError, ScenarioSaveOutcome, save_scenario_with_warnings};
use crate::scenario_state::ScenarioEditorState;

const AUTHORING_METHOD: &str = "ccoeff";

const DEFAULT_STATE: &str = "field";

const DEFAULT_THRESHOLD: f32 = 0.9;

#[derive(Debug, Clone, Default)]
struct PendingStep {
    tap: Option<(u32, u32)>,
    region: Option<[u32; 4]>,
}

#[derive(Debug, Clone)]
pub struct AuthoringStep {
    pub task: TaskDef,
    pub template: DynamicImage,
    pub tap: (u32, u32),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GestureOutcome {
    Pending,
    Confirmed { warnings: Vec<String> },
}

#[derive(Debug, thiserror::Error)]
pub enum AuthoringError {
    #[error("no current frame: push a frame before completing a step")]
    NoFrame,
    #[error("roi {roi:?} has zero width or height")]
    RoiEmpty { roi: [u32; 4] },
    #[error("roi {roi:?} does not intersect the current frame ({fw}x{fh})")]
    RoiOutOfFrame { roi: [u32; 4], fw: u32, fh: u32 },
    #[error("internal: task build failed for method `{method}`")]
    TaskBuild { method: &'static str },
}

#[derive(Debug, Clone)]
pub struct AuthoringSession {
    name: String,
    state: String,
    threshold: f32,
    goal: Goal,
    frame: Option<DynamicImage>,
    pending: PendingStep,
    steps: Vec<AuthoringStep>,
    warnings: Vec<String>,
    saved_to: Option<std::path::PathBuf>,
}

impl AuthoringSession {
    #[must_use]
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            state: DEFAULT_STATE.to_string(),
            threshold: DEFAULT_THRESHOLD,
            goal: Goal {
                name: "authoring_timeout".to_string(),
                stop: StopCondition::Timeout { secs: 600 },
            },
            frame: None,
            pending: PendingStep::default(),
            steps: Vec::new(),
            warnings: Vec::new(),
            saved_to: None,
        }
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn set_state(&mut self, state: &str) {
        self.state = state.to_string();
    }

    pub fn set_threshold(&mut self, threshold: f32) {
        self.threshold = threshold.clamp(0.5, 0.99);
    }

    pub fn set_goal(&mut self, goal: Goal) {
        self.goal = goal;
    }

    pub fn push_frame(&mut self, frame: &DynamicImage) {
        self.frame = Some(frame.clone());
    }

    #[must_use]
    pub fn pending_tap(&self) -> Option<(u32, u32)> {
        self.pending.tap
    }

    #[must_use]
    pub fn pending_region(&self) -> Option<[u32; 4]> {
        self.pending.region
    }

    pub fn record_tap(&mut self, pos: (u32, u32)) -> Result<GestureOutcome, AuthoringError> {
        let prev = self.pending.tap;
        self.pending.tap = Some(pos);
        match self.try_confirm() {
            Ok(outcome) => Ok(outcome),
            Err(e) => {
                self.pending.tap = prev;
                Err(e)
            }
        }
    }

    pub fn record_region(&mut self, roi: [u32; 4]) -> Result<GestureOutcome, AuthoringError> {
        let [_, _, w, h] = roi;
        if w == 0 || h == 0 {
            return Err(AuthoringError::RoiEmpty { roi });
        }
        if let Some(frame) = self.frame.as_ref()
            && clamp_region_to_frame(roi, frame.width(), frame.height()).is_none()
        {
            return Err(AuthoringError::RoiOutOfFrame {
                roi,
                fw: frame.width(),
                fh: frame.height(),
            });
        }
        let prev = self.pending.region;
        self.pending.region = Some(roi);
        match self.try_confirm() {
            Ok(outcome) => Ok(outcome),
            Err(e) => {
                self.pending.region = prev;
                Err(e)
            }
        }
    }

    pub fn undo(&mut self) -> bool {
        if self.pending.tap.is_some() || self.pending.region.is_some() {
            self.pending = PendingStep::default();
            return true;
        }
        self.steps.pop().is_some()
    }

    #[must_use]
    pub fn steps(&self) -> &[AuthoringStep] {
        &self.steps
    }

    #[must_use]
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    #[must_use]
    pub fn to_scenario(&self) -> ScenarioEditorState {
        let mut st = ScenarioEditorState::new(&self.name);
        for (index, step) in self.steps.iter().enumerate() {
            let mut task = step.task.clone();
            task.next = Some(
                self.steps
                    .get(index + 1)
                    .map_or_else(Vec::new, |next| vec![next.task.name.clone()]),
            );
            st.add_task(task);
        }
        st.add_goal(self.goal.clone());
        st.loaded_from = self.saved_to.clone();
        st
    }

    pub fn save(
        &mut self,
        pipelines_root: &Path,
    ) -> Result<ScenarioSaveOutcome, ScenarioSaveError> {
        let state = self.to_scenario();
        let pngs: Vec<(String, DynamicImage)> = self
            .steps
            .iter()
            .map(|step| (step.task.name.clone(), step.template.clone()))
            .collect();
        let outcome = save_scenario_with_warnings(&state, &pngs, pipelines_root)?;
        self.saved_to = Some(outcome.dir.clone());
        Ok(outcome)
    }

    fn try_confirm(&mut self) -> Result<GestureOutcome, AuthoringError> {
        let (Some(tap), Some(region)) = (self.pending.tap, self.pending.region) else {
            return Ok(GestureOutcome::Pending);
        };
        let Some(frame) = self.frame.as_ref() else {
            return Err(AuthoringError::NoFrame);
        };
        let Some(clamped) = clamp_region_to_frame(region, frame.width(), frame.height()) else {
            return Err(AuthoringError::RoiOutOfFrame {
                roi: region,
                fw: frame.width(),
                fh: frame.height(),
            });
        };
        let [cx, cy, cw, ch] = clamped;
        let crop = frame.crop_imm(cx, cy, cw, ch);
        let mut warnings = Vec::new();
        if clamped != region {
            warnings.push(format!(
                "警告: 認識領域 {roi:?} が現行フレーム ({fw}x{fh}) 外にはみ出したため \
                 {cw}x{ch} へクリップしました",
                roi = region,
                fw = frame.width(),
                fh = frame.height(),
            ));
        }
        if let Some(warning) = template_structure_warning(&crop) {
            warnings.push(warning);
        }
        if let Some(warning) = needle_roi_warning((crop.width(), crop.height()), Some(clamped)) {
            warnings.push(warning);
        }
        if !region_contains(clamped, tap) {
            warnings.push(format!(
                "警告: 記録タップ位置 ({tx}, {ty}) が認識領域 {clamped:?} の外です \
                 (実行時タップはマッチ中心から導かれるため確定しますが、意図した \
                 UI 要素とは別の領域を認識している恐れがあります)",
                tx = tap.0,
                ty = tap.1,
            ));
        }
        let name = self.next_step_name();
        let Some(task) = pipeline_task_spec(
            &name,
            &self.state,
            AUTHORING_METHOD,
            ScreenRegion::new(cx, cy, cw, ch),
            self.threshold,
            PipelineActionKind::ClickSelf,
        ) else {
            return Err(AuthoringError::TaskBuild {
                method: AUTHORING_METHOD,
            });
        };
        let step_warnings: Vec<String> = warnings.iter().map(|w| format!("{name}: {w}")).collect();
        self.steps.push(AuthoringStep {
            task,
            template: crop,
            tap,
        });
        self.warnings.extend(step_warnings.iter().cloned());
        self.pending = PendingStep::default();
        Ok(GestureOutcome::Confirmed {
            warnings: step_warnings,
        })
    }

    fn next_step_name(&self) -> String {
        format!("AuthoredStep{:02}", self.steps.len() + 1)
    }
}

fn clamp_region_to_frame(roi: [u32; 4], fw: u32, fh: u32) -> Option<[u32; 4]> {
    let [x, y, w, h] = roi;
    if w == 0 || h == 0 || x >= fw || y >= fh {
        return None;
    }
    Some([x, y, w.min(fw - x), h.min(fh - y)])
}

fn region_contains(roi: [u32; 4], pos: (u32, u32)) -> bool {
    let [x, y, w, h] = roi;
    pos.0 >= x && pos.1 >= y && pos.0 - x < w && pos.1 - y < h
}
