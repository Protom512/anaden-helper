use std::path::{Path, PathBuf};

use image::DynamicImage;

use crate::matcher::MatchResult;

use super::recognize::{probe_template, resolve_template};
use super::schema::{Action, Algorithm, TaskDef, TaskDefError};

#[derive(Debug, Clone)]
pub struct Task {
    def: TaskDef,
    template: PathBuf,
}

impl Task {
    pub(crate) fn from_def(def: TaskDef, root: &Path) -> Self {
        let template = resolve_template(&def.template, root);
        Self { def, template }
    }

    pub(crate) fn into_def(self) -> TaskDef {
        self.def
    }

    pub fn def(&self) -> &TaskDef {
        &self.def
    }

    pub fn name(&self) -> &str {
        &self.def.name
    }

    pub fn state(&self) -> &str {
        &self.def.state
    }

    pub fn base(&self) -> Option<&str> {
        self.def.base.as_deref()
    }

    pub fn algorithm(&self) -> Algorithm {
        self.def.algorithm
    }

    pub fn template(&self) -> &Path {
        &self.template
    }

    pub fn threshold(&self) -> f32 {
        self.def.threshold
    }

    pub fn action(&self) -> Action {
        self.def.action.clone().unwrap_or(Action::DoNothing)
    }

    pub fn next(&self) -> &[String] {
        self.def.next.as_deref().unwrap_or(&[])
    }

    pub fn detect(&self, screenshot: &DynamicImage) -> Result<Option<MatchResult>, TaskDefError> {
        Ok(probe_template(
            &self.template,
            self.def.roi,
            self.def.algorithm,
            self.def.threshold,
            screenshot,
        )?
        .matched)
    }
}
