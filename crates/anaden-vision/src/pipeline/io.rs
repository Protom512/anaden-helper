use std::path::{Path, PathBuf};

use serde::Serialize;
use tracing::{debug, warn};

use crate::template_quality::{needle_exceeds_roi_warning_for_path, unstructured_warning_for_path};

use super::schema::{
    PIPELINE_MANIFEST_FILENAME, PipelineManifest, TaskDef, TaskDefError, TaskRow,
    is_pipeline_manifest_path, task_name_issue,
};

pub(crate) const WATCH_DIRNAME: &str = "watch";

pub(crate) fn load_tasks(dir: &Path) -> Result<Vec<TaskDef>, TaskDefError> {
    let mut defs = Vec::new();
    if !dir.exists() {
        debug!("Pipeline directory does not exist: {:?}", dir);
        return Ok(defs);
    }

    for entry in std::fs::read_dir(dir).map_err(|e| TaskDefError::ParseFailed {
        path: dir.to_path_buf(),
        reason: e.to_string(),
    })? {
        let entry = entry.map_err(|e| TaskDefError::ParseFailed {
            path: dir.to_path_buf(),
            reason: e.to_string(),
        })?;
        let path = entry.path();

        let is_toml = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("toml"))
            .unwrap_or(false);
        if !is_toml {
            continue;
        }

        if is_pipeline_manifest_path(&path) {
            debug!("Skipping pipeline manifest (not a TaskDef): {:?}", path);
            continue;
        }

        let content = std::fs::read_to_string(&path).map_err(|e| TaskDefError::ParseFailed {
            path: path.clone(),
            reason: e.to_string(),
        })?;
        let mut def: TaskDef = toml::from_str(&content).map_err(|e| TaskDefError::ParseFailed {
            path: path.clone(),
            reason: e.to_string(),
        })?;

        if let Some(issue) = task_name_issue(&def.name) {
            return Err(TaskDefError::InvalidTaskName {
                path: path.clone(),
                name: def.name.clone(),
                issue,
            });
        }

        if !def.template.is_absolute()
            && let Some(parent) = path.parent()
        {
            def.template = parent.join(&def.template);
        }

        if let Some(warning) = unstructured_warning_for_path(&def.template) {
            warn!("task '{}': {warning}", def.name);
        }

        if let Some(warning) = needle_exceeds_roi_warning_for_path(&def.template, def.roi) {
            warn!("task '{}': {warning}", def.name);
        }

        debug!("Loaded pipeline task '{}' from {:?}", def.name, path);
        defs.push(def);
    }

    Ok(defs)
}

pub fn load_pipeline(dir: &Path) -> Result<Vec<TaskDef>, TaskDefError> {
    load_tasks(dir)
}

pub fn load_pipeline_manifest(dir: &Path) -> Result<PipelineManifest, TaskDefError> {
    let manifest_path = dir.join(PIPELINE_MANIFEST_FILENAME);
    let content =
        std::fs::read_to_string(&manifest_path).map_err(|e| TaskDefError::ParseFailed {
            path: manifest_path.clone(),
            reason: format!("manifest read failed: {e}"),
        })?;
    let manifest: PipelineManifest =
        toml::from_str(&content).map_err(|e| TaskDefError::ParseFailed {
            path: manifest_path,
            reason: e.to_string(),
        })?;
    Ok(manifest)
}

pub fn load_watch_defs(dir: &Path) -> Result<Vec<TaskDef>, TaskDefError> {
    load_tasks(&dir.join(WATCH_DIRNAME))
}

pub fn save_task_def(def: &TaskDef, path: &Path) -> Result<(), TaskDefError> {
    if is_pipeline_manifest_path(path) {
        return Err(TaskDefError::SaveFailed {
            reason: format!(
                "refusing to save a TaskDef into the manifest convention path {path:?} \
                 (load_pipeline skips it; save the manifest via save_pipeline_manifest instead)"
            ),
        });
    }
    let base = path.parent().unwrap_or_else(|| Path::new(""));
    let relative = PathBuf::from(relativize_template(&def.template, base));
    let row = TaskRow::from_def(def, relative);
    let content = to_pretty_toml(&row).map_err(|e| TaskDefError::SaveFailed {
        reason: e.to_string(),
    })?;
    std::fs::write(path, content).map_err(|e| TaskDefError::SaveFailed {
        reason: format!("write {path:?}: {e}"),
    })
}

pub fn save_pipeline_manifest(manifest: &PipelineManifest, dir: &Path) -> Result<(), TaskDefError> {
    let content = to_pretty_toml(manifest).map_err(|e| TaskDefError::SaveFailed {
        reason: e.to_string(),
    })?;
    let path = dir.join(PIPELINE_MANIFEST_FILENAME);
    std::fs::write(&path, content).map_err(|e| TaskDefError::SaveFailed {
        reason: format!("write {path:?}: {e}"),
    })
}

pub(crate) fn to_pretty_toml<T: Serialize + ?Sized>(value: &T) -> Result<String, toml::ser::Error> {
    let mut v = toml::Value::try_from(value)?;
    normalize_f32_floats(&mut v);
    toml::to_string(&v)
}

pub(crate) fn normalize_f32_floats(value: &mut toml::Value) {
    match value {
        toml::Value::Float(f) => {
            if let Ok(shortest) = format!("{}", *f as f32).parse::<f64>() {
                *f = shortest;
            }
        }
        toml::Value::Array(items) => {
            for item in items {
                normalize_f32_floats(item);
            }
        }
        toml::Value::Table(table) => {
            for (_, v) in table.iter_mut() {
                normalize_f32_floats(v);
            }
        }
        _ => {}
    }
}

pub(crate) fn relativize_template(template: &Path, base: &Path) -> String {
    let forward = |p: &Path| p.to_string_lossy().replace('\\', "/");
    if !template.is_absolute() || base.as_os_str().is_empty() {
        return forward(template);
    }
    let target: Vec<_> = template.components().collect();
    let base_components: Vec<_> = base.components().collect();
    let common = target
        .iter()
        .zip(base_components.iter())
        .take_while(|(t, b)| t == b)
        .count();
    if common == 0 {
        return forward(template);
    }
    let mut rel = PathBuf::new();
    for _ in common..base_components.len() {
        rel.push("..");
    }
    for c in &target[common..] {
        rel.push(c.as_os_str());
    }
    if rel.as_os_str().is_empty() {
        return forward(template);
    }
    forward(&rel)
}
