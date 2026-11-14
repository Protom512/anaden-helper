use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anaden_core::ScreenRegion;
use image::DynamicImage;
use tracing::warn;

use crate::diagnose::DiagnoseEntry;
use crate::scale::{PC_CLIENT_HEIGHT_MEASURED, PC_CLIENT_WIDTH_MEASURED, roi_to_normalized};

use super::io::{load_pipeline_manifest, load_tasks};
use super::recognize::{probe_template, resolve_template, roi_to_region};
use super::schema::{Action, PIPELINE_MANIFEST_FILENAME, PipelineManifest, TaskDef, TaskDefError};
use super::task::Task;

use super::io::WATCH_DIRNAME;

#[derive(Debug, Clone, PartialEq)]
pub struct StepOutcome {
    pub matched_task: String,
    pub action: Action,
    pub next: Vec<String>,
    pub matched_region: ScreenRegion,
    pub matched_confidence: f32,
}

pub struct Pipeline {
    root: PathBuf,
    all: Vec<Task>,
    chain_len: usize,
    index: HashMap<String, usize>,
    manifest: Option<PipelineManifest>,
}

impl Pipeline {
    pub fn load(dir: &Path) -> Result<Self, TaskDefError> {
        let manifest = if dir.join(PIPELINE_MANIFEST_FILENAME).is_file() {
            Some(load_pipeline_manifest(dir)?)
        } else {
            None
        };
        let chain = load_tasks(dir)?;
        let watch = load_tasks(&dir.join(WATCH_DIRNAME))?;
        Ok(Self::build(dir, chain, watch, manifest))
    }

    #[must_use]
    pub fn from_defs(root: &Path, defs: Vec<TaskDef>) -> Self {
        Self::build(root, defs, Vec::new(), None)
    }

    fn build(
        root: &Path,
        chain: Vec<TaskDef>,
        watch: Vec<TaskDef>,
        manifest: Option<PipelineManifest>,
    ) -> Self {
        let chain_len = chain.len();
        let mut all: Vec<Task> = chain.into_iter().map(|d| Task::from_def(d, root)).collect();
        all.extend(watch.into_iter().map(|d| Task::from_def(d, root)));

        let mut index = HashMap::with_capacity(all.len());
        for (i, task) in all.iter().enumerate() {
            index.entry(task.name().to_string()).or_insert(i);
        }

        Self {
            root: root.to_path_buf(),
            all,
            chain_len,
            index,
            manifest,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn manifest(&self) -> Option<&PipelineManifest> {
        self.manifest.as_ref()
    }

    pub fn start_task(&self) -> Option<&str> {
        self.manifest.as_ref().map(|m| m.start_task.as_str())
    }

    pub fn tasks(&self) -> &[Task] {
        &self.all[..self.chain_len]
    }

    pub fn watch_tasks(&self) -> &[Task] {
        &self.all[self.chain_len..]
    }

    pub fn watch_names(&self) -> &[String] {
        self.manifest.as_ref().map_or(&[], |m| m.watch.as_slice())
    }

    pub fn task(&self, name: &str) -> Option<&Task> {
        self.index.get(name).and_then(|i| self.all.get(*i))
    }

    pub fn step(
        &self,
        screenshot: &DynamicImage,
        current: &str,
    ) -> Result<Option<StepOutcome>, TaskDefError> {
        let Some(task) = self.task(current) else {
            return Ok(None);
        };
        step_def(task.def(), task.template(), screenshot)
    }

    pub fn inspect(&self, screenshot: &DynamicImage) -> Vec<DiagnoseEntry> {
        let mut entries: Vec<DiagnoseEntry> = self
            .all
            .iter()
            .map(|t| diagnose_def(t.def(), t.template(), screenshot))
            .collect();
        sort_by_confidence_desc(&mut entries);
        entries
    }

    pub fn defs(&self) -> impl Iterator<Item = &TaskDef> {
        self.all[..self.chain_len].iter().map(Task::def)
    }

    pub fn into_defs(self) -> Vec<TaskDef> {
        self.all.into_iter().map(Task::into_def).collect()
    }

    pub fn into_chain_defs(mut self) -> Vec<TaskDef> {
        self.all.truncate(self.chain_len);
        self.into_defs()
    }
}

pub(crate) fn step_def(
    def: &TaskDef,
    template: &Path,
    screenshot: &DynamicImage,
) -> Result<Option<StepOutcome>, TaskDefError> {
    match probe_template(template, def.roi, def.algorithm, def.threshold, screenshot) {
        Ok(probe) => match probe.matched {
            Some(matched) => Ok(Some(StepOutcome {
                matched_task: def.name.clone(),
                action: def.action.clone().unwrap_or(Action::DoNothing),
                next: def.next.clone().unwrap_or_default(),
                matched_region: matched.region,
                matched_confidence: matched.confidence.0,
            })),
            None => Ok(None),
        },
        Err(e) => {
            warn!("task '{}' detect failed: {e}", def.name);
            Err(e)
        }
    }
}

pub(crate) fn diagnose_def(
    def: &TaskDef,
    template: &Path,
    screenshot: &DynamicImage,
) -> DiagnoseEntry {
    let norm_w = screenshot.width();
    let norm_h = screenshot.height();
    let sx = if PC_CLIENT_WIDTH_MEASURED == 0 {
        1.0
    } else {
        norm_w as f32 / PC_CLIENT_WIDTH_MEASURED as f32
    };
    let sy = if PC_CLIENT_HEIGHT_MEASURED == 0 {
        1.0
    } else {
        norm_h as f32 / PC_CLIENT_HEIGHT_MEASURED as f32
    };

    let scaled_roi = roi_to_region(def.roi).map(|r| {
        let s = roi_to_normalized([r.x, r.y, r.width, r.height], norm_w, norm_h);
        ScreenRegion::new(s[0], s[1], s[2], s[3])
    });

    let mut entry = DiagnoseEntry {
        task: def.name.clone(),
        template_path: template.to_path_buf(),
        algorithm: def.algorithm,
        threshold: def.threshold,
        best_confidence: None,
        best_region: None,
        scaled_roi,
        scale_factors: (sx, sy),
        needle_scaled_size: None,
        error: None,
    };

    match probe_template(template, def.roi, def.algorithm, 0.0, screenshot) {
        Ok(probe) => {
            entry.needle_scaled_size = probe.needle_size;
            match probe.matched {
                Some(m) => {
                    entry.best_confidence = Some(m.confidence.0);
                    entry.best_region = Some(m.region);
                }
                None => {
                    entry.error =
                        Some("needle larger than haystack (or empty scan space)".to_string());
                }
            }
        }
        Err(e) => entry.error = Some(e.to_string()),
    }

    entry
}

pub(crate) fn cmp_confidence_desc(a: &DiagnoseEntry, b: &DiagnoseEntry) -> std::cmp::Ordering {
    match (a.best_confidence, b.best_confidence) {
        (Some(x), Some(y)) => y.partial_cmp(&x).unwrap_or(std::cmp::Ordering::Equal),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    }
}

pub(crate) fn sort_by_confidence_desc(entries: &mut [DiagnoseEntry]) {
    entries.sort_by(cmp_confidence_desc);
}

pub(crate) fn resolved_for_root(def: &TaskDef, root: &Path) -> PathBuf {
    resolve_template(&def.template, root)
}
