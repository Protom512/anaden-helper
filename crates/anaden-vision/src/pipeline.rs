use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::SystemTime;

use image::DynamicImage;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, warn};

use anaden_core::{MatchConfidence, ScreenRegion};

use crate::ccoeff::{CcoeffVisionEngine, PreparedCcoeff};
use crate::engine::{SseVisionEngine, VisionEngine};
use crate::matcher::{MatchResult, TemplateMatcher};
use crate::template_quality::{needle_exceeds_roi_warning_for_path, unstructured_warning_for_path};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Algorithm {
    Sse,
    Ccoeff,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    ClickSelf,
    ClickRect {
        roi: ScreenRegion,
    },
    Swipe {
        from: ScreenRegion,
        to: ScreenRegion,
    },
    DoNothing,
    Stop,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ClickRectDef {
    roi: ScreenRegion,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SwipeDef {
    from: ScreenRegion,
    to: ScreenRegion,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BareActionDef {}

impl<'de> Deserialize<'de> for Action {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error;

        let raw = toml::Value::deserialize(deserializer)?;
        let table = raw
            .as_table()
            .ok_or_else(|| D::Error::custom("action must be a table with a `type` field"))?;

        let type_str = table
            .get("type")
            .and_then(|v| v.as_str())
            .ok_or_else(|| D::Error::custom("action requires a string `type` field"))?;

        let mut rest = toml::value::Table::new();
        for (k, v) in table {
            if k != "type" {
                rest.insert(k.clone(), v.clone());
            }
        }
        let rest_value = toml::Value::Table(rest);

        match type_str {
            "click_self" => {
                let _: BareActionDef = rest_value
                    .try_into()
                    .map_err(|e| D::Error::custom(format!("click_self action: {e}")))?;
                Ok(Action::ClickSelf)
            }
            "do_nothing" => {
                let _: BareActionDef = rest_value
                    .try_into()
                    .map_err(|e| D::Error::custom(format!("do_nothing action: {e}")))?;
                Ok(Action::DoNothing)
            }
            "stop" => {
                let _: BareActionDef = rest_value
                    .try_into()
                    .map_err(|e| D::Error::custom(format!("stop action: {e}")))?;
                Ok(Action::Stop)
            }
            "click_rect" => {
                let def: ClickRectDef = rest_value
                    .try_into()
                    .map_err(|e| D::Error::custom(format!("click_rect action: {e}")))?;
                Ok(Action::ClickRect { roi: def.roi })
            }
            "swipe" => {
                let def: SwipeDef = rest_value
                    .try_into()
                    .map_err(|e| D::Error::custom(format!("swipe action: {e}")))?;
                Ok(Action::Swipe {
                    from: def.from,
                    to: def.to,
                })
            }
            other => Err(D::Error::custom(format!("unknown action type `{other}`"))),
        }
    }
}

impl Serialize for Action {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap;

        struct RoiArray(ScreenRegion);

        impl Serialize for RoiArray {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                [self.0.x, self.0.y, self.0.width, self.0.height].serialize(serializer)
            }
        }

        let len = match self {
            Action::ClickSelf | Action::DoNothing | Action::Stop => 1,
            Action::ClickRect { .. } => 2,
            Action::Swipe { .. } => 3,
        };
        let mut map = serializer.serialize_map(Some(len))?;
        match self {
            Action::ClickSelf => map.serialize_entry("type", "click_self")?,
            Action::DoNothing => map.serialize_entry("type", "do_nothing")?,
            Action::Stop => map.serialize_entry("type", "stop")?,
            Action::ClickRect { roi } => {
                map.serialize_entry("type", "click_rect")?;
                map.serialize_entry("roi", &RoiArray(*roi))?;
            }
            Action::Swipe { from, to } => {
                map.serialize_entry("type", "swipe")?;
                map.serialize_entry("from", &RoiArray(*from))?;
                map.serialize_entry("to", &RoiArray(*to))?;
            }
        }
        map.end()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskDef {
    pub name: String,
    pub state: String,
    pub algorithm: Algorithm,
    pub template: PathBuf,
    #[serde(default)]
    pub roi: Option<[u32; 4]>,
    #[serde(default = "default_threshold")]
    pub threshold: f32,

    #[serde(default)]
    pub base: Option<String>,
    #[serde(default)]
    pub action: Option<Action>,
    #[serde(default)]
    pub next: Option<Vec<String>>,
}

fn default_threshold() -> f32 {
    MatchConfidence::DEFAULT_THRESHOLD.0
}

impl TaskDef {
    pub fn detect(
        &self,
        screenshot: &DynamicImage,
        template_root: &Path,
    ) -> Result<Option<MatchResult>, TaskDefError> {
        let norm_w = screenshot.width();
        let norm_h = screenshot.height();

        let tpl_path = if self.template.is_absolute() {
            self.template.clone()
        } else {
            template_root.join(&self.template)
        };
        let prepared = prepared_template(&tpl_path, norm_w, norm_h)?;

        let scaled_roi = self.roi_to_region().map(|r| {
            let s = crate::scale::roi_to_normalized([r.x, r.y, r.width, r.height], norm_w, norm_h);
            ScreenRegion::new(s[0], s[1], s[2], s[3])
        });
        let work = match &scaled_roi {
            Some(r) => crop_imm(screenshot, *r),
            None => screenshot.clone(),
        };

        let thr = MatchConfidence::new(self.threshold);
        let result = match self.algorithm {
            Algorithm::Sse => {
                let m = TemplateMatcher::threshold_only(thr);
                SseVisionEngine::new(m).match_template(&work, &prepared.needle)
            }
            Algorithm::Ccoeff => CcoeffVisionEngine::threshold_only(thr)
                .match_template_prepared(&work, &prepared.ccoeff),
        };

        Ok(result.map(|mut mr| {
            if let Some(r) = scaled_roi {
                mr.region = ScreenRegion::new(
                    mr.region.x + r.x,
                    mr.region.y + r.y,
                    mr.region.width,
                    mr.region.height,
                );
            }
            mr
        }))
    }

    fn roi_to_region(&self) -> Option<ScreenRegion> {
        let [x, y, w, h] = self.roi?;
        if w == 0 || h == 0 {
            return None;
        }
        Some(ScreenRegion::new(x, y, w, h))
    }
}

#[derive(Debug, Error)]
pub enum TaskDefError {
    #[error("template load failed {path}: {reason}")]
    TemplateLoadFailed { path: PathBuf, reason: String },

    #[error("TOML parse failed {path}: {reason}")]
    ParseFailed { path: PathBuf, reason: String },

    #[error("invalid task name in {path}: {name:?} ({issue})")]
    InvalidTaskName {
        path: PathBuf,
        name: String,
        issue: &'static str,
    },

    #[error("save failed: {reason}")]
    SaveFailed { reason: String },
}

#[must_use]
pub fn task_name_issue(name: &str) -> Option<&'static str> {
    if name.trim().is_empty() {
        return Some("name must not be empty");
    }
    if name != name.trim() {
        return Some("name must not have leading/trailing whitespace");
    }
    if name.contains('/') || name.contains('\\') {
        return Some("name must not contain path separators");
    }
    if name.contains("..") {
        return Some("name must not contain `..`");
    }
    if name.starts_with('.') {
        return Some("name must not start with a dot");
    }
    None
}

pub(crate) struct PreparedTemplate {
    pub(crate) needle: DynamicImage,
    pub(crate) ccoeff: PreparedCcoeff,
}

struct CachedTemplate {
    mtime: Option<SystemTime>,
    prepared: Arc<PreparedTemplate>,
}

struct TemplateCache {
    entries: Mutex<HashMap<(PathBuf, u32, u32), CachedTemplate>>,
}

impl Default for TemplateCache {
    fn default() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
        }
    }
}

impl TemplateCache {
    fn get_or_prepare(
        &self,
        path: &Path,
        norm_w: u32,
        norm_h: u32,
    ) -> Result<Arc<PreparedTemplate>, TaskDefError> {
        let key = (path.to_path_buf(), norm_w, norm_h);
        let mut entries = match self.entries.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(hit) = entries.get(&key) {
            if mtime_unchanged(path, hit.mtime) {
                return Ok(hit.prepared.clone());
            }
            debug!("template cache invalidated (mtime changed): {path:?}");
            entries.remove(&key);
        }
        let needle = image::open(path).map_err(|e| TaskDefError::TemplateLoadFailed {
            path: path.to_path_buf(),
            reason: e.to_string(),
        })?;
        let mtime = file_mtime(path);
        let normalized = crate::scale::needle_to_normalized(&needle, norm_w, norm_h);
        let ccoeff = PreparedCcoeff::prepare(&normalized);
        let prepared = Arc::new(PreparedTemplate {
            needle: normalized,
            ccoeff,
        });
        entries.insert(
            key,
            CachedTemplate {
                mtime,
                prepared: prepared.clone(),
            },
        );
        Ok(prepared)
    }
}

fn file_mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

fn mtime_unchanged(path: &Path, cached: Option<SystemTime>) -> bool {
    match (file_mtime(path), cached) {
        (Some(current), Some(cached)) => current == cached,
        (None, _) => true,
        (Some(_), None) => false,
    }
}

static TEMPLATE_CACHE: LazyLock<TemplateCache> = LazyLock::new(TemplateCache::default);

pub(crate) fn prepared_template(
    path: &Path,
    norm_w: u32,
    norm_h: u32,
) -> Result<Arc<PreparedTemplate>, TaskDefError> {
    TEMPLATE_CACHE.get_or_prepare(path, norm_w, norm_h)
}

pub fn load_pipeline(dir: &Path) -> Result<Vec<TaskDef>, TaskDefError> {
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

        if !def.template.is_absolute() && path.parent().is_some() {
            let parent = path.parent().expect("parent exists");
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

pub const PIPELINE_MANIFEST_FILENAME: &str = "pipeline.toml";

fn is_pipeline_manifest_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|n| n == PIPELINE_MANIFEST_FILENAME)
        .unwrap_or(false)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PipelineManifest {
    pub start_task: String,
    #[serde(default, rename = "goal", skip_serializing_if = "Vec::is_empty")]
    pub goals: Vec<anaden_core::Goal>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub watch: Vec<String>,
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
    let mut savable = def.clone();
    savable.template = PathBuf::from(relativize_template(&def.template, base));
    let content = to_pretty_toml(&savable).map_err(|e| TaskDefError::SaveFailed {
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

fn to_pretty_toml<T: Serialize + ?Sized>(value: &T) -> Result<String, toml::ser::Error> {
    let mut v = toml::Value::try_from(value)?;
    normalize_f32_floats(&mut v);
    toml::to_string(&v)
}

fn normalize_f32_floats(value: &mut toml::Value) {
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

fn relativize_template(template: &Path, base: &Path) -> String {
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

#[derive(Debug, Clone, PartialEq)]
pub struct StepOutcome {
    pub matched_task: String,
    pub action: Action,
    pub next: Vec<String>,
    pub matched_region: ScreenRegion,
    pub matched_confidence: f32,
}

pub fn run_step(
    tasks: &[TaskDef],
    screenshot: &DynamicImage,
    current_name: &str,
) -> Result<Option<StepOutcome>, TaskDefError> {
    let Some(task) = tasks.iter().find(|t| t.name == current_name) else {
        return Ok(None);
    };
    match task.detect(screenshot, Path::new("")) {
        Ok(Some(matched)) => {
            let action = task.action.clone().unwrap_or(Action::DoNothing);
            let next = task.next.clone().unwrap_or_default();
            Ok(Some(StepOutcome {
                matched_task: task.name.clone(),
                action,
                next,
                matched_region: matched.region,
                matched_confidence: matched.confidence.0,
            }))
        }
        Ok(None) => Ok(None),
        Err(e) => {
            warn!("task '{current_name}' detect failed: {e}");
            Err(e)
        }
    }
}

fn crop_imm(img: &DynamicImage, r: ScreenRegion) -> DynamicImage {
    let x = r.x.min(img.width().saturating_sub(1));
    let y = r.y.min(img.height().saturating_sub(1));
    let w = r.width.min(img.width().saturating_sub(x));
    let h = r.height.min(img.height().saturating_sub(y));
    img.crop_imm(x, y, w, h)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use image::{DynamicImage, GrayImage, ImageBuffer, Luma};
    use std::fs;
    use std::path::PathBuf;

    fn gradient_needle(w: u32, h: u32) -> GrayImage {
        let mut img = GrayImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let v = ((x + y) % 64) as u8;
                img.put_pixel(x, y, Luma([v]));
            }
        }
        img
    }

    fn embed(
        haystack_w: u32,
        haystack_h: u32,
        needle: &GrayImage,
        ox: u32,
        oy: u32,
        bg: u8,
    ) -> GrayImage {
        let mut img = GrayImage::from_pixel(haystack_w, haystack_h, Luma([bg]));
        for y in 0..needle.height() {
            for x in 0..needle.width() {
                let p = needle.get_pixel(x, y)[0];
                img.put_pixel(ox + x, oy + y, Luma([p]));
            }
        }
        img
    }

    fn luma_dyn(img: GrayImage) -> DynamicImage {
        DynamicImage::ImageLuma8(img)
    }

    fn write_toml(dir: &Path, filename: &str, body: &str) -> PathBuf {
        let path = dir.join(filename);
        fs::write(&path, body).expect("write toml");
        path
    }

    fn write_template(dir: &Path, needle: &GrayImage) -> PathBuf {
        let sub = dir.join("scenes").join("title");
        fs::create_dir_all(&sub).expect("mkdir");
        let p = sub.join("title_center.png");
        needle.save(&p).expect("save png");
        p
    }

    #[test]
    fn load_pipeline_parses_full_taskdef() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let toml_body = r#"
            name      = "TitleScreen"
            state     = "TitleScreen"
            algorithm = "ccoeff"
            template  = "scenes/title/title_center.png"
            roi       = [520, 320, 240, 80]
            threshold = 0.95
            base      = "TitleBase"
            next      = ["LoadGame"]
        "#;
        write_toml(tmp.path(), "title.toml", toml_body);

        let defs = load_pipeline(tmp.path()).expect("load ok");
        assert_eq!(defs.len(), 1, "exactly one task");
        let d = &defs[0];
        assert_eq!(d.name, "TitleScreen");
        assert_eq!(d.state, "TitleScreen");
        assert_eq!(d.algorithm, Algorithm::Ccoeff);
        assert_eq!(d.roi, Some([520, 320, 240, 80]));
        assert!((d.threshold - 0.95).abs() < 1e-6);
        assert_eq!(d.base.as_deref(), Some("TitleBase"));
        assert_eq!(d.next.as_deref(), Some(&["LoadGame".to_string()][..]));
        assert!(d.template.is_absolute(), "template must be absolute");
        assert_eq!(d.template, tmp.path().join("scenes/title/title_center.png"));
    }

    #[test]
    fn load_pipeline_rejects_unknown_field() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let toml_body = r#"
            name      = "X"
            state     = "X"
            algorithm = "ccoeff"
            template  = "t.png"
            threshild = 0.9
        "#;
        write_toml(tmp.path(), "bad.toml", toml_body);

        let err = load_pipeline(tmp.path()).expect_err("deny_unknown_fields must reject typo");
        assert!(matches!(err, TaskDefError::ParseFailed { .. }));
    }

    #[test]
    fn load_pipeline_rejects_unknown_algorithm() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let toml_body = r#"
            name      = "X"
            state     = "X"
            algorithm = "feature"
            template  = "t.png"
        "#;
        write_toml(tmp.path(), "bad.toml", toml_body);

        let err = load_pipeline(tmp.path()).expect_err("unknown variant must reject");
        assert!(matches!(err, TaskDefError::ParseFailed { .. }));
    }

    #[test]
    fn load_pipeline_rejects_path_unsafe_task_name() {
        for name in ["../../evil", "C:\\abs\\path", ".hidden", " padded ", ""] {
            let tmp = tempfile::tempdir().expect("tempdir");
            let toml_body = format!(
                r#"
                name      = '{name}'
                state     = "X"
                algorithm = "ccoeff"
                template  = "t.png"
            "#
            );
            write_toml(tmp.path(), "bad.toml", &toml_body);

            let err = load_pipeline(tmp.path())
                .expect_err(&format!("unsafe task name {name:?} must be rejected"));
            assert!(
                err.to_string().contains("invalid task name"),
                "unexpected error for {name:?}: {err:?}"
            );
        }
    }

    #[test]
    fn load_pipeline_accepts_cjk_task_name() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let toml_body = r#"
            name      = "秘境巡回"
            state     = "X"
            algorithm = "ccoeff"
            template  = "t.png"
        "#;
        write_toml(tmp.path(), "cjk.toml", toml_body);

        let defs = load_pipeline(tmp.path()).expect("CJK task name must load");
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, "秘境巡回");
    }

    #[test]
    fn load_pipeline_applies_default_threshold_when_omitted() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let toml_body = r#"
            name      = "X"
            state     = "X"
            algorithm = "sse"
            template  = "t.png"
        "#;
        write_toml(tmp.path(), "minimal.toml", toml_body);

        let defs = load_pipeline(tmp.path()).expect("load ok");
        assert_eq!(defs.len(), 1);
        assert!(
            (defs[0].threshold - MatchConfidence::DEFAULT_THRESHOLD.0).abs() < 1e-6,
            "omitted threshold must default to 0.95"
        );
        assert_eq!(defs[0].algorithm, Algorithm::Sse);
        assert_eq!(defs[0].roi, None);
    }

    #[test]
    fn load_pipeline_empty_dir_returns_empty_vec() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let defs = load_pipeline(tmp.path()).expect("empty dir ok");
        assert!(defs.is_empty());
    }

    #[test]
    fn load_pipeline_ignores_non_toml() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_toml(tmp.path(), "readme.md", "# not a task");
        write_toml(
            tmp.path(),
            "real.toml",
            "name = \"R\"\nstate = \"R\"\nalgorithm = \"sse\"\ntemplate = \"t.png\"\n",
        );
        let defs = load_pipeline(tmp.path()).expect("load ok");
        assert_eq!(defs.len(), 1, "only *.toml picked up");
        assert_eq!(defs[0].name, "R");
    }

    #[test]
    fn load_pipeline_succeeds_with_unstructured_template() {
        let tmp = tempfile::tempdir().expect("tempdir");
        GrayImage::from_pixel(40, 20, Luma([250u8]))
            .save(tmp.path().join("flat.png"))
            .expect("save flat png");
        write_toml(
            tmp.path(),
            "flat.toml",
            r#"
            name      = "Flat"
            state     = "Flat"
            algorithm = "ccoeff"
            template  = "flat.png"
            "#,
        );

        let defs = load_pipeline(tmp.path()).expect("load must succeed despite flat template");
        assert_eq!(
            defs.len(),
            1,
            "TaskDef is still loaded (warn-only, not rejected)"
        );
        assert!(defs[0].template.is_absolute());
        assert!(
            defs[0].template.exists(),
            "flat template path must still resolve"
        );
    }

    #[test]
    fn load_pipeline_succeeds_with_needle_exceeding_roi() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut needle = GrayImage::new(138, 20);
        for y in 0..20 {
            for x in 0..138 {
                let v = ((x * 2 + y * 3) % 200) as u8;
                needle.put_pixel(x, y, Luma([v]));
            }
        }
        needle
            .save(tmp.path().join("wide.png"))
            .expect("save wide png");
        write_toml(
            tmp.path(),
            "wide.toml",
            r#"
            name      = "Wide"
            state     = "Wide"
            algorithm = "ccoeff"
            template  = "wide.png"
            roi       = [8, 2, 121, 35]
            "#,
        );

        let defs = load_pipeline(tmp.path()).expect("load must succeed despite oversized needle");
        assert_eq!(
            defs.len(),
            1,
            "TaskDef is still loaded (warn-only, not rejected)"
        );
        assert_eq!(defs[0].roi, Some([8, 2, 121, 35]));
        assert!(
            defs[0].template.exists(),
            "oversized template path must still resolve"
        );
    }

    #[test]
    fn detect_with_roi_finds_embedded_position() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let needle = gradient_needle(40, 40);
        let screenshot = luma_dyn(embed(FULL_W, FULL_H, &needle, 600, 340, 128));
        write_template(tmp.path(), &needle);

        let task = TaskDef {
            name: "T".into(),
            state: "T".into(),
            algorithm: Algorithm::Ccoeff,
            template: tmp.path().join("scenes/title/title_center.png"),
            roi: Some([520, 320, 240, 80]),
            threshold: 0.9,
            base: None,
            action: None,
            next: None,
        };

        let m = task.detect(&screenshot, tmp.path()).expect("detect ok");
        let m = m.expect("should match within ROI");
        assert!(
            m.region.x >= 598 && m.region.x <= 602,
            "region.x in ROI offset coords: got {}",
            m.region.x
        );
        assert!(
            m.region.y >= 338 && m.region.y <= 342,
            "region.y in ROI offset coords: got {}",
            m.region.y
        );
        assert!(
            m.confidence.0 >= 0.9,
            "confidence >= threshold: got {}",
            m.confidence.0
        );
    }

    #[test]
    fn detect_with_roi_outside_returns_none() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let needle = gradient_needle(40, 40);
        let screenshot = luma_dyn(embed(FULL_W, FULL_H, &needle, 50, 50, 128));
        write_template(tmp.path(), &needle);

        let task = TaskDef {
            name: "T".into(),
            state: "T".into(),
            algorithm: Algorithm::Ccoeff,
            template: tmp.path().join("scenes/title/title_center.png"),
            roi: Some([520, 320, 240, 80]),
            threshold: 0.9,
            base: None,
            action: None,
            next: None,
        };

        let m = task.detect(&screenshot, tmp.path()).expect("detect ok");
        assert!(m.is_none(), "needle outside ROI must not match");
    }

    const FULL_W: u32 = 1258;
    const FULL_H: u32 = 708;
    const NEEDLE_PX: u32 = 12;

    #[test]
    fn detect_without_roi_finds_position_fullscan() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let needle = gradient_needle(NEEDLE_PX, NEEDLE_PX);
        let screenshot = luma_dyn(embed(FULL_W, FULL_H, &needle, 150, 75, 128));
        write_template(tmp.path(), &needle);

        let task = TaskDef {
            name: "T".into(),
            state: "T".into(),
            algorithm: Algorithm::Ccoeff,
            template: tmp.path().join("scenes/title/title_center.png"),
            roi: None,
            threshold: 0.9,
            base: None,
            action: None,
            next: None,
        };

        let m = task.detect(&screenshot, tmp.path()).expect("detect ok");
        let m = m.expect("should match full scan");
        assert!(
            m.region.x >= 148 && m.region.x <= 152,
            "region.x at embed: got {}",
            m.region.x
        );
        assert!(
            m.region.y >= 73 && m.region.y <= 77,
            "region.y at embed: got {}",
            m.region.y
        );
        assert!(
            m.confidence.0 >= 0.99,
            "exact embed confidence should be ~1.0: got {}",
            m.confidence.0
        );
    }

    #[test]
    fn detect_switches_sse_and_ccoeff() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let needle = gradient_needle(NEEDLE_PX, NEEDLE_PX);
        let screenshot = luma_dyn(embed(FULL_W, FULL_H, &needle, 150, 75, 128));
        write_template(tmp.path(), &needle);

        let tpl = tmp.path().join("scenes/title/title_center.png");

        let mk = |algo: Algorithm| TaskDef {
            name: "T".into(),
            state: "T".into(),
            algorithm: algo,
            template: tpl.clone(),
            roi: None,
            threshold: 0.9,
            base: None,
            action: None,
            next: None,
        };

        let sse_m = mk(Algorithm::Sse)
            .detect(&screenshot, tmp.path())
            .expect("sse ok")
            .expect("sse should match");
        let cc_m = mk(Algorithm::Ccoeff)
            .detect(&screenshot, tmp.path())
            .expect("ccoeff ok")
            .expect("ccoeff should match");

        for (label, x) in [("sse", sse_m.region.x), ("ccoeff", cc_m.region.x)] {
            assert!(
                (148..=152).contains(&x),
                "{label} region.x at embed: got {x}"
            );
        }
        for (label, y) in [("sse", sse_m.region.y), ("ccoeff", cc_m.region.y)] {
            assert!((73..=77).contains(&y), "{label} region.y at embed: got {y}");
        }
        assert!(
            sse_m.confidence.0 >= 0.9,
            "sse confidence >= 0.9: got {}",
            sse_m.confidence.0
        );
        assert!(
            cc_m.confidence.0 >= 0.9,
            "ccoeff confidence >= 0.9: got {}",
            cc_m.confidence.0
        );
    }

    fn shifted_needle(needle: &GrayImage, delta: i32) -> GrayImage {
        let mut out = GrayImage::new(needle.width(), needle.height());
        for (x, y, p) in needle.enumerate_pixels() {
            let v = (p[0] as i32 + delta).clamp(0, 255) as u8;
            out.put_pixel(x, y, Luma([v]));
        }
        out
    }

    #[test]
    fn detect_threshold_above_achievable_returns_none() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let base = gradient_needle(NEEDLE_PX, NEEDLE_PX);
        let screenshot = luma_dyn(embed(FULL_W, FULL_H, &base, 150, 75, 128));
        let tpl_needle = shifted_needle(&base, 60);
        write_template(tmp.path(), &tpl_needle);

        let low_task = TaskDef {
            name: "T".into(),
            state: "T".into(),
            algorithm: Algorithm::Sse,
            template: tmp.path().join("scenes/title/title_center.png"),
            roi: Some([140, 65, 40, 30]),
            threshold: 0.1,
            base: None,
            action: None,
            next: None,
        };
        let low = low_task
            .detect(&screenshot, tmp.path())
            .expect("detect ok")
            .expect("should match at low threshold");
        assert!(
            low.confidence.0 < 1.0,
            "brightness-shifted template must not be a perfect SSE match: got {}",
            low.confidence.0
        );
        let achievable = low.confidence.0;
        assert!(
            achievable < 0.95,
            "shift should noticeably degrade SSE confidence: got {achievable}"
        );

        let high_task = TaskDef {
            threshold: achievable + 0.05,
            ..low_task
        };
        let high = high_task
            .detect(&screenshot, tmp.path())
            .expect("detect ok");
        assert!(
            high.is_none(),
            "threshold above achievable confidence ({achievable}) must yield None"
        );
    }

    #[test]
    fn detect_missing_template_returns_load_error() {
        let screenshot = luma_dyn(ImageBuffer::from_pixel(100, 100, Luma([0u8])));
        let task = TaskDef {
            name: "T".into(),
            state: "T".into(),
            algorithm: Algorithm::Sse,
            template: PathBuf::from("/nonexistent/does_not_exist.png"),
            roi: None,
            threshold: 0.9,
            base: None,
            action: None,
            next: None,
        };

        let err = task
            .detect(&screenshot, Path::new("/tmp"))
            .expect_err("missing template");
        assert!(matches!(err, TaskDefError::TemplateLoadFailed { .. }));
    }

    fn taskdef_toml_with_action(action_block: &str) -> String {
        format!(
            r#"
            name      = "T"
            state     = "T"
            algorithm = "sse"
            template  = "t.png"
            {action_block}
            "#
        )
    }

    #[test]
    fn action_click_self_parses() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let body = taskdef_toml_with_action(
            r#"
            [action]
            type = "click_self"
            "#,
        );
        write_toml(tmp.path(), "t.toml", &body);

        let defs = load_pipeline(tmp.path()).expect("load ok");
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].action, Some(Action::ClickSelf));
    }

    #[test]
    fn action_click_rect_parses_roi() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let body = taskdef_toml_with_action(
            r#"
            [action]
            type = "click_rect"
            roi = [520, 320, 240, 80]
            "#,
        );
        write_toml(tmp.path(), "t.toml", &body);

        let defs = load_pipeline(tmp.path()).expect("load ok");
        assert_eq!(
            defs[0].action,
            Some(Action::ClickRect {
                roi: ScreenRegion::new(520, 320, 240, 80)
            })
        );
    }

    #[test]
    fn action_swipe_parses_from_to() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let body = taskdef_toml_with_action(
            r#"
            [action]
            type = "swipe"
            from = [100, 500, 40, 40]
            to   = [100, 100, 40, 40]
            "#,
        );
        write_toml(tmp.path(), "t.toml", &body);

        let defs = load_pipeline(tmp.path()).expect("load ok");
        assert_eq!(
            defs[0].action,
            Some(Action::Swipe {
                from: ScreenRegion::new(100, 500, 40, 40),
                to: ScreenRegion::new(100, 100, 40, 40),
            })
        );
    }

    #[test]
    fn action_do_nothing_and_stop() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_toml(
            tmp.path(),
            "dn.toml",
            &taskdef_toml_with_action(
                r#"
                [action]
                type = "do_nothing"
                "#,
            ),
        );
        write_toml(
            tmp.path(),
            "st.toml",
            &taskdef_toml_with_action(
                r#"
                [action]
                type = "stop"
                "#,
            ),
        );

        let defs = load_pipeline(tmp.path()).expect("load ok");
        assert_eq!(defs.len(), 2);
        let dn = defs
            .iter()
            .find(|d| d.name == "T" && d.action == Some(Action::DoNothing));
        assert!(dn.is_some(), "do_nothing variant parsed");
        let st = defs.iter().find(|d| d.action == Some(Action::Stop));
        assert!(st.is_some(), "stop variant parsed");
    }

    #[test]
    fn action_omitted_is_none() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let body = r#"
            name      = "T"
            state     = "T"
            algorithm = "sse"
            template  = "t.png"
        "#;
        write_toml(tmp.path(), "t.toml", body);

        let defs = load_pipeline(tmp.path()).expect("load ok");
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].action, None, "omitted action must be None");
    }

    #[test]
    fn action_unknown_type_rejected() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let body = taskdef_toml_with_action(
            r#"
            [action]
            type = "teleport"
            "#,
        );
        write_toml(tmp.path(), "t.toml", &body);

        let err = load_pipeline(tmp.path()).expect_err("unknown action type must reject");
        assert!(matches!(err, TaskDefError::ParseFailed { .. }));
    }

    #[test]
    fn action_click_rect_missing_roi_rejected() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let body = taskdef_toml_with_action(
            r#"
            [action]
            type = "click_rect"
            "#,
        );
        write_toml(tmp.path(), "t.toml", &body);

        let err = load_pipeline(tmp.path()).expect_err("missing roi must reject");
        assert!(matches!(err, TaskDefError::ParseFailed { .. }));
    }

    #[test]
    fn action_unknown_field_rejected() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let body = taskdef_toml_with_action(
            r#"
            [action]
            type = "click_self"
            extra = 1
            "#,
        );
        write_toml(tmp.path(), "t.toml", &body);

        let err = load_pipeline(tmp.path()).expect_err("deny_unknown_fields must reject extra");
        assert!(matches!(err, TaskDefError::ParseFailed { .. }));
    }

    fn run_step_setup(with_needle: bool) -> (DynamicImage, PathBuf) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let needle = gradient_needle(40, 40);
        let screenshot = if with_needle {
            luma_dyn(embed(FULL_W, FULL_H, &needle, 150, 75, 128))
        } else {
            luma_dyn(GrayImage::from_pixel(FULL_W, FULL_H, Luma([128u8])))
        };
        let tpl = write_template(tmp.path(), &needle);
        let _persisted_dir = tmp.keep();
        (screenshot, tpl)
    }

    #[test]
    fn run_step_match_returns_action_and_next() {
        let (screenshot, tpl) = run_step_setup(true);
        let tasks = vec![TaskDef {
            name: "Title".into(),
            state: "Title".into(),
            algorithm: Algorithm::Ccoeff,
            template: tpl.clone(),
            roi: None,
            threshold: 0.9,
            base: None,
            action: Some(Action::ClickSelf),
            next: Some(vec!["LoadGame".into()]),
        }];

        let out = run_step(&tasks, &screenshot, "Title")
            .expect("run_step ok")
            .expect("should match");
        assert_eq!(out.matched_task, "Title");
        assert_eq!(out.action, Action::ClickSelf);
        assert_eq!(out.next, vec!["LoadGame".to_string()]);

        let _ = std::fs::remove_file(&tpl);
    }

    #[test]
    fn run_step_match_omitted_action_defaults_do_nothing() {
        let (screenshot, tpl) = run_step_setup(true);
        let tasks = vec![TaskDef {
            name: "Title".into(),
            state: "Title".into(),
            algorithm: Algorithm::Ccoeff,
            template: tpl.clone(),
            roi: None,
            threshold: 0.9,
            base: None,
            action: None,
            next: None,
        }];

        let out = run_step(&tasks, &screenshot, "Title")
            .expect("run_step ok")
            .expect("should match");
        assert_eq!(out.matched_task, "Title");
        assert_eq!(
            out.action,
            Action::DoNothing,
            "omitted action defaults to DoNothing"
        );
        assert!(out.next.is_empty(), "omitted next defaults to empty Vec");

        let _ = std::fs::remove_file(&tpl);
    }

    #[test]
    fn run_step_no_match_returns_none() {
        let (screenshot, tpl) = run_step_setup(false);
        let tasks = vec![TaskDef {
            name: "Title".into(),
            state: "Title".into(),
            algorithm: Algorithm::Ccoeff,
            template: tpl.clone(),
            roi: None,
            threshold: 0.9,
            base: None,
            action: Some(Action::ClickSelf),
            next: None,
        }];

        let out = run_step(&tasks, &screenshot, "Title").expect("run_step ok");
        assert!(
            out.is_none(),
            "no needle in screenshot must yield Ok(None) (NoMatch)"
        );

        let _ = std::fs::remove_file(&tpl);
    }

    #[test]
    fn run_step_below_threshold_returns_none() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let base = gradient_needle(40, 40);
        let screenshot = luma_dyn(embed(FULL_W, FULL_H, &base, 150, 75, 128));
        let tpl_needle = shifted_needle(&base, 60);
        write_template(tmp.path(), &tpl_needle);

        let probe = TaskDef {
            name: "Title".into(),
            state: "Title".into(),
            algorithm: Algorithm::Sse,
            template: tmp.path().join("scenes/title/title_center.png"),
            roi: Some([140, 65, 60, 60]),
            threshold: 0.1,
            base: None,
            action: Some(Action::ClickSelf),
            next: None,
        };
        let probe_match = probe
            .detect(&screenshot, tmp.path())
            .expect("detect ok")
            .expect("should match at low threshold");
        let achievable = probe_match.confidence.0;
        assert!(
            achievable < 0.95,
            "shifted template must degrade SSE confidence below 0.95: got {achievable}"
        );

        let tasks = vec![TaskDef {
            threshold: achievable + 0.05,
            ..probe
        }];
        let out = run_step(&tasks, &screenshot, "Title").expect("run_step ok");
        assert!(
            out.is_none(),
            "threshold above achievable confidence ({achievable}) must yield Ok(None)"
        );
    }

    #[test]
    fn run_step_unknown_task_name_returns_none() {
        let (screenshot, tpl) = run_step_setup(true);
        let tasks = vec![TaskDef {
            name: "Title".into(),
            state: "Title".into(),
            algorithm: Algorithm::Ccoeff,
            template: tpl.clone(),
            roi: None,
            threshold: 0.9,
            base: None,
            action: Some(Action::ClickSelf),
            next: None,
        }];

        let out = run_step(&tasks, &screenshot, "NoSuchTask").expect("run_step ok");
        assert!(out.is_none(), "unknown task name must yield Ok(None)");

        let _ = std::fs::remove_file(&tpl);
    }

    #[test]
    fn detect_template_loaded_once_for_repeated_detects() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let needle = gradient_needle(40, 40);
        let tpl = write_template(tmp.path(), &needle);
        let frame1 = luma_dyn(embed(FULL_W, FULL_H, &needle, 620, 330, 128));
        let frame2 = luma_dyn(embed(FULL_W, FULL_H, &needle, 560, 360, 128));

        let task = TaskDef {
            name: "T".into(),
            state: "T".into(),
            algorithm: Algorithm::Ccoeff,
            template: tpl.clone(),
            roi: Some([520, 320, 240, 120]),
            threshold: 0.9,
            base: None,
            action: None,
            next: None,
        };

        let m1 = task
            .detect(&frame1, tmp.path())
            .expect("1st detect ok")
            .expect("1st detect must match");
        assert!(
            (618..=622).contains(&m1.region.x),
            "frame1 match near embed x=620: got {}",
            m1.region.x
        );

        std::fs::remove_file(&tpl).expect("remove template file");

        let m2 = task
            .detect(&frame2, tmp.path())
            .expect("2nd detect must be served from cache (no disk reload)");
        let m2 = m2.expect("2nd detect must still match from cached template");
        assert!(
            (558..=562).contains(&m2.region.x),
            "frame2 match near its embed x=560 (fresh frame instance): got {}",
            m2.region.x
        );
    }

    #[test]
    fn detect_cache_invalidated_on_mtime_change() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let needle = gradient_needle(40, 40);
        let tpl = write_template(tmp.path(), &needle);
        let frame = luma_dyn(embed(FULL_W, FULL_H, &needle, 620, 330, 128));

        let task = TaskDef {
            name: "T".into(),
            state: "T".into(),
            algorithm: Algorithm::Ccoeff,
            template: tpl.clone(),
            roi: Some([520, 320, 240, 120]),
            threshold: 0.9,
            base: None,
            action: None,
            next: None,
        };

        assert!(
            task.detect(&frame, tmp.path())
                .expect("1st detect ok")
                .is_some(),
            "embedded needle must match on first detect"
        );

        std::thread::sleep(std::time::Duration::from_millis(20));

        let mut inverted = GrayImage::new(40, 40);
        for y in 0..40 {
            for x in 0..40 {
                inverted.put_pixel(x, y, Luma([255 - ((x + y) % 64) as u8]));
            }
        }
        inverted.save(&tpl).expect("save inverted template");

        let m2 = task.detect(&frame, tmp.path()).expect("2nd detect ok");
        assert!(
            m2.is_none(),
            "rewritten template must be reloaded and evaluated (stale cache must not keep matching)"
        );
    }

    #[test]
    fn run_step_reuses_cached_template_for_verify_recheck() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let needle = gradient_needle(40, 40);
        let tpl = write_template(tmp.path(), &needle);
        let frame1 = luma_dyn(embed(FULL_W, FULL_H, &needle, 620, 330, 128));
        let frame2 = luma_dyn(embed(FULL_W, FULL_H, &needle, 560, 360, 128));

        let tasks = vec![TaskDef {
            name: "T".into(),
            state: "T".into(),
            algorithm: Algorithm::Ccoeff,
            template: tpl.clone(),
            roi: Some([520, 320, 240, 120]),
            threshold: 0.9,
            base: None,
            action: Some(Action::ClickSelf),
            next: None,
        }];

        let out1 = run_step(&tasks, &frame1, "T").expect("1st run_step must match");

        std::fs::remove_file(&tpl).expect("remove template file");

        let out2 = run_step(&tasks, &frame2, "T")
            .expect("verify-path run_step must reuse the cached template (F19: no reload)");
        assert!(
            out2.is_some(),
            "verify-path run_step must still match from the cached template (F19: no 2x cost)"
        );
        let _ = out1;
    }

    #[test]
    fn run_step_missing_template_returns_err_not_none() {
        let (screenshot, _tpl) = run_step_setup(true);
        let tasks = vec![TaskDef {
            name: "Title".into(),
            state: "Title".into(),
            algorithm: Algorithm::Ccoeff,
            template: PathBuf::from("/nonexistent/does_not_exist.png"),
            roi: None,
            threshold: 0.9,
            base: None,
            action: Some(Action::ClickSelf),
            next: None,
        }];

        let out = run_step(&tasks, &screenshot, "Title");
        let err = out.expect_err("missing template must surface Err (not squashed to None)");
        assert!(
            matches!(err, TaskDefError::TemplateLoadFailed { .. }),
            "error kind must be TemplateLoadFailed, got {err:?}"
        );
    }

    fn raw_feature_canvas() -> GrayImage {
        let mut img = GrayImage::new(FULL_W, FULL_H);
        for y in 0..FULL_H {
            for x in 0..FULL_W {
                let v = (((x * 2) + y) % 200) as u8;
                img.put_pixel(x, y, Luma([v]));
            }
        }
        img
    }

    const VAR_ROI: [u32; 4] = [400, 200, 120, 80];

    fn write_var_needle(tmp: &std::path::Path, raw: &GrayImage) -> PathBuf {
        let [x, y, w, h] = VAR_ROI;
        let mut needle = GrayImage::new(w, h);
        for dy in 0..h {
            for dx in 0..w {
                needle.put_pixel(dx, dy, *raw.get_pixel(x + dx, y + dy));
            }
        }
        write_template(tmp, &needle)
    }

    fn resized_screenshot(raw: &GrayImage, tw: u32, th: u32) -> DynamicImage {
        let dyn_raw = DynamicImage::ImageLuma8(raw.clone());
        dyn_raw.resize_exact(tw, th, image::imageops::FilterType::Triangle)
    }

    #[test]
    fn detect_matches_across_size_variants_dynamic_scale() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let raw = raw_feature_canvas();
        let tpl = write_var_needle(tmp.path(), &raw);
        let screenshot = luma_dyn(raw.clone());

        let task = TaskDef {
            name: "T".into(),
            state: "T".into(),
            algorithm: Algorithm::Ccoeff,
            template: tpl.clone(),
            roi: Some(VAR_ROI),
            threshold: 0.9,
            base: None,
            action: None,
            next: None,
        };
        let m = task
            .detect(&screenshot, tmp.path())
            .expect("detect ok")
            .expect("1258x708 native must match");
        assert!(
            m.confidence.0 >= 0.99,
            "native 1258x708 confidence ~1.0: got {}",
            m.confidence.0
        );
        assert!(
            (398..=402).contains(&m.region.x),
            "native region.x near ROI x=400: got {}",
            m.region.x
        );
        assert!(
            (198..=202).contains(&m.region.y),
            "native region.y near ROI y=200: got {}",
            m.region.y
        );

        let _ = std::fs::remove_file(&tpl);
    }

    #[test]
    fn detect_matches_across_size_variants_1280x720() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let raw = raw_feature_canvas();
        let tpl = write_var_needle(tmp.path(), &raw);
        let screenshot = resized_screenshot(&raw, 1280, 720);

        let task = TaskDef {
            name: "T".into(),
            state: "T".into(),
            algorithm: Algorithm::Ccoeff,
            template: tpl.clone(),
            roi: Some(VAR_ROI),
            threshold: 0.9,
            base: None,
            action: None,
            next: None,
        };
        let m = task
            .detect(&screenshot, tmp.path())
            .expect("detect ok")
            .expect("1280x720 must match via dynamic scale");
        assert!(
            m.confidence.0 >= 0.9,
            "1280x720 dynamic-scale match confidence >= 0.9: got {}",
            m.confidence.0
        );

        let _ = std::fs::remove_file(&tpl);
    }

    #[test]
    fn detect_matches_across_size_variants_letterboxed_crop_dims() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let raw = raw_feature_canvas();
        let tpl = write_var_needle(tmp.path(), &raw);
        let screenshot = resized_screenshot(&raw, 1000, 600);

        let task = TaskDef {
            name: "T".into(),
            state: "T".into(),
            algorithm: Algorithm::Ccoeff,
            template: tpl.clone(),
            roi: Some(VAR_ROI),
            threshold: 0.9,
            base: None,
            action: None,
            next: None,
        };
        let m = task
            .detect(&screenshot, tmp.path())
            .expect("detect ok")
            .expect("1000x600 must match via dynamic scale");
        assert!(
            m.confidence.0 >= 0.9,
            "1000x600 dynamic-scale match confidence >= 0.9: got {}",
            m.confidence.0
        );

        let _ = std::fs::remove_file(&tpl);
    }

    #[test]
    fn doc_recommended_toml_parses() {
        let toml_body = r#"
            name      = "TitleScreen"
            state     = "TitleScreen"
            algorithm = "ccoeff"
            template  = "scenes/title/title_center.png"
            roi       = [520, 320, 240, 80]
            threshold = 0.95
            action    = { type = "click_self" }
            next      = ["LoadGame"]
        "#;
        let def: TaskDef = toml::from_str(toml_body).expect("doc example must parse");
        assert_eq!(def.name, "TitleScreen");
        assert_eq!(def.state, "TitleScreen");
        assert_eq!(def.algorithm, Algorithm::Ccoeff);
        assert_eq!(def.template, PathBuf::from("scenes/title/title_center.png"));
        assert_eq!(def.roi, Some([520, 320, 240, 80]));
        assert!((def.threshold - 0.95).abs() < 1e-6);
        assert_eq!(def.action, Some(Action::ClickSelf));
        assert_eq!(def.next.as_deref(), Some(&["LoadGame".to_string()][..]));
    }

    #[test]
    fn doc_recommended_toml_click_rect_parses() {
        let toml_body = r#"
            name      = "LoadGame"
            state     = "Loading"
            algorithm = "ccoeff"
            template  = "scenes/title/load_game_area.png"
            threshold = 0.95
            action    = { type = "click_rect", roi = [540, 1600, 200, 200] }
            next      = ["Field"]
        "#;
        let def: TaskDef = toml::from_str(toml_body).expect("click_rect example must parse");
        assert_eq!(def.name, "LoadGame");
        assert_eq!(def.algorithm, Algorithm::Ccoeff);
        assert_eq!(
            def.action,
            Some(Action::ClickRect {
                roi: ScreenRegion::new(540, 1600, 200, 200)
            })
        );
        assert_eq!(def.next.as_deref(), Some(&["Field".to_string()][..]));
    }

    #[test]
    fn doc_recommended_toml_swipe_parses() {
        let toml_body = r#"
            name      = "ScrollDown"
            state     = "Menu"
            algorithm = "ccoeff"
            template  = "scenes/menu/scroll_handle.png"
            threshold = 0.90
            action    = { type = "swipe", from = [540, 1500, 40, 40], to = [540, 300, 40, 40] }
            next      = []
        "#;
        let def: TaskDef = toml::from_str(toml_body).expect("swipe example must parse");
        assert_eq!(def.name, "ScrollDown");
        assert_eq!(
            def.action,
            Some(Action::Swipe {
                from: ScreenRegion::new(540, 1500, 40, 40),
                to: ScreenRegion::new(540, 300, 40, 40),
            })
        );
        assert!(def.next.as_deref().map(|v| v.is_empty()).unwrap_or(false));
    }

    #[test]
    fn doc_recommended_toml_stop_parses() {
        let toml_body = r#"
            name      = "End"
            state     = "Unknown"
            algorithm = "sse"
            template  = "scenes/end/flag.png"
            threshold = 0.95
            action    = { type = "stop" }
        "#;
        let def: TaskDef = toml::from_str(toml_body).expect("stop example must parse");
        assert_eq!(def.algorithm, Algorithm::Sse);
        assert_eq!(def.action, Some(Action::Stop));
    }

    #[test]
    fn inline_action_does_not_swallow_following_keys() {
        let toml_body = r#"
            name      = "T"
            state     = "T"
            algorithm = "sse"
            template  = "t.png"
            action    = { type = "stop" }
            next      = ["AfterStop"]
        "#;
        let def: TaskDef =
            toml::from_str(toml_body).expect("inline action keeps next at top-level");
        assert_eq!(def.action, Some(Action::Stop));
        assert_eq!(def.next.as_deref(), Some(&["AfterStop".to_string()][..]));
    }

    #[test]
    fn doc_forbidden_namespace_table_rejected() {
        let toml_body = r#"
            [task.TitleScreen]
            algorithm = "ccoeff"
        "#;
        let err = toml::from_str::<TaskDef>(toml_body);
        assert!(err.is_err(), "namespace [task.X] must be rejected");
    }

    fn workspace_templates_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("templates")
    }

    fn assert_roi_within_1258x708(roi: [u32; 4], label: &str) {
        let [x, y, w, h] = roi;
        assert!(
            w > 0 && h > 0,
            "{label}: width/height must be > 0, got {roi:?}"
        );
        assert!(
            x + w <= 1258 && y + h <= 708,
            "{label}: ROI {roi:?} exceeds RAW 1258x708 PC capture space"
        );
    }

    #[test]
    fn pc_field_loop_pipeline_loads_with_click_self_actions() {
        let dir = workspace_templates_root()
            .join("pipelines")
            .join("field_loop_pc");
        let defs = load_pipeline(&dir).expect("field_loop_pc must load");
        assert_eq!(
            defs.len(),
            2,
            "field_loop_pc must contain tap_bottom + tap_hud_tr"
        );

        let by_name: std::collections::HashMap<&str, &TaskDef> =
            defs.iter().map(|d| (d.name.as_str(), d)).collect();

        let bottom = by_name
            .get("TapBottomStablePc")
            .expect("TapBottomStablePc task present");
        assert_eq!(bottom.algorithm, Algorithm::Ccoeff);
        assert_eq!(bottom.action, Some(Action::ClickSelf));
        assert_eq!(
            bottom.next.as_deref(),
            Some(&[][..]),
            "field loop task is terminal (next=[])"
        );
        assert_eq!(bottom.state, "Field");
        let bottom_roi = bottom.roi.expect("TapBottomStablePc has ROI");
        assert_roi_within_1258x708(bottom_roi, "TapBottomStablePc");
        assert!(
            bottom.template.is_absolute(),
            "template path must be absolute"
        );
        assert!(
            bottom.template.exists(),
            "template PNG must exist at {:?}",
            bottom.template
        );

        let hud = by_name.get("TapHudTrPc").expect("TapHudTrPc task present");
        assert_eq!(hud.algorithm, Algorithm::Ccoeff);
        assert_eq!(hud.action, Some(Action::ClickSelf));
        assert_eq!(hud.next.as_deref(), Some(&[][..]));
        let hud_roi = hud.roi.expect("TapHudTrPc has ROI");
        assert_roi_within_1258x708(hud_roi, "TapHudTrPc");
        assert!(hud.template.exists(), "hud template PNG must exist");
    }

    #[test]
    fn field_loop_pc_pipeline_manifest_loads_start_task_and_loop_count_goal() {
        let dir = workspace_templates_root()
            .join("pipelines")
            .join("field_loop_pc");
        let manifest = load_pipeline_manifest(&dir)
            .expect("field_loop_pc/pipeline.toml must load as manifest");

        assert_eq!(
            manifest.start_task, "TapBottomStablePc",
            "start_task must match TapBottomStablePc TaskDef name"
        );
        assert_eq!(
            manifest.goals.len(),
            1,
            "field_loop_pc manifest declares exactly one goal"
        );
        let goal = &manifest.goals[0];
        assert_eq!(goal.name, "field_loop_50");
        assert_eq!(
            goal.stop,
            anaden_core::StopCondition::LoopCount { target: 50 },
            "field_loop_pc goal must be UC-1 LoopCount(target=50)"
        );
        goal.validate().expect("field_loop_pc goal must validate");

        let defs = load_pipeline(&dir).expect("TaskDefs must load (manifest skipped)");
        assert_eq!(
            defs.len(),
            2,
            "load_pipeline must skip pipeline.toml and pick up only the 2 TaskDefs"
        );
        assert!(
            defs.iter().any(|d| d.name == "TapBottomStablePc"),
            "TapBottomStablePc TaskDef must be present alongside the manifest"
        );
        assert!(
            defs.iter().any(|d| d.name == manifest.start_task),
            "manifest.start_task '{}' must exist as a TaskDef in the same directory",
            manifest.start_task
        );
    }

    #[test]
    fn pc_nav_to_field_points_at_pc_field_hud_template() {
        let dir = workspace_templates_root()
            .join("pipelines")
            .join("nav_to_field_pc");
        let defs = load_pipeline(&dir).expect("nav_to_field_pc must load");
        assert!(
            defs.iter().any(|d| d.name == "FieldHudTopPc"),
            "nav_to_field_pc must contain FieldHudTopPc"
        );

        let nav = defs
            .iter()
            .find(|d| d.name == "FieldHudTopPc")
            .expect("FieldHudTopPc present");
        assert_eq!(nav.algorithm, Algorithm::Ccoeff);
        assert_eq!(
            nav.action,
            Some(Action::DoNothing),
            "nav target = stop (do_nothing)"
        );
        assert_eq!(nav.next.as_deref(), Some(&[][..]));
        let nav_roi = nav.roi.expect("FieldHudTopPc has ROI");
        assert_roi_within_1258x708(nav_roi, "FieldHudTopPc");
        assert!(
            nav.template.exists(),
            "nav template PNG must exist at {:?}",
            nav.template
        );
        assert!(
            nav.template.to_string_lossy().contains("field_pc"),
            "nav FieldHudTopPc must reference the pc-scoped field scene, got {:?}",
            nav.template
        );
    }

    #[test]
    fn pc_nav_to_field_contains_t3_cold_start_slices() {
        let dir = workspace_templates_root()
            .join("pipelines")
            .join("nav_to_field_pc");
        let defs = load_pipeline(&dir).expect("nav_to_field_pc must load");
        let by_name: std::collections::HashMap<&str, &TaskDef> =
            defs.iter().map(|d| (d.name.as_str(), d)).collect();

        let tap = by_name
            .get("TapToStartPc")
            .expect("TapToStartPc (T3 title cold-start slice) must exist");
        assert_eq!(tap.algorithm, Algorithm::Ccoeff);
        let action = tap
            .action
            .as_ref()
            .expect("TapToStartPc must have a click_rect action");
        assert!(
            matches!(action, Action::ClickRect { .. }),
            "TapToStartPc action must be click_rect (static tap on flickering Tap-to-Start), \
             got {:?}",
            action
        );
        assert!(
            tap.template.exists(),
            "TapToStartPc template PNG must exist at {:?}",
            tap.template
        );
        assert!(
            tap.template.to_string_lossy().contains("title_pc"),
            "TapToStartPc must reference pc-scoped title_pc scene, got {:?}",
            tap.template
        );
        assert!(
            tap.threshold >= 0.5,
            "TapToStartPc threshold {} must not be the legacy 0.01 hack (>= 0.5 expected)",
            tap.threshold
        );
        let tap_roi = tap.roi.expect("TapToStartPc has detection ROI");
        assert_roi_within_1258x708(tap_roi, "TapToStartPc");
        if let Action::ClickRect { roi } = action {
            assert_roi_within_1258x708(
                [roi.x, roi.y, roi.width, roi.height],
                "TapToStartPc click_rect.roi",
            );
            assert!(
                roi.x > 600,
                "TapToStartPc click_rect x={} should be in right half (~Tap-to-Start col 930)",
                roi.x
            );
        }

        let load = by_name
            .get("LoadGamePc")
            .expect("LoadGamePc (T3 title cold-start slice) must exist");
        let load_action = load
            .action
            .as_ref()
            .expect("LoadGamePc must have a click_rect action");
        assert!(
            matches!(load_action, Action::ClickRect { .. }),
            "LoadGamePc action must be click_rect, got {:?}",
            load_action
        );
        assert!(
            load.template.exists(),
            "LoadGamePc template PNG must exist at {:?}",
            load.template
        );
        assert!(
            load.template.to_string_lossy().contains("title_pc"),
            "LoadGamePc must reference pc-scoped title_pc scene, got {:?}",
            load.template
        );
        assert!(
            load.threshold >= 0.5,
            "LoadGamePc threshold {} must not be the legacy 0.01 hack (>= 0.5 expected)",
            load.threshold
        );
        let load_roi = load.roi.expect("LoadGamePc has detection ROI");
        assert_roi_within_1258x708(load_roi, "LoadGamePc");
        assert_eq!(
            tap.template, load.template,
            "TapToStartPc and LoadGamePc must share the stable title_logo_corner anchor \
             (Issue #208: version_label is a content-variable asset and was dropped as the \
             login/nav anchor)"
        );
        assert!(
            tap.template.to_string_lossy().contains("title_logo_corner"),
            "TapToStartPc must anchor on title_logo_corner.png, got {:?}",
            tap.template
        );

        assert_eq!(
            tap_roi,
            [604, 223, 160, 180],
            "TapToStartPc ROI must be the logo_corner slack window [604,223,160,180] \
             (Issue #208 anchor switch + drift-slack pair-update, Issue #210 widened)"
        );
        assert_eq!(
            load_roi,
            [604, 223, 160, 180],
            "LoadGamePc ROI must equal the TapToStartPc slack window [604,223,160,180] \
             (Issue #208: shared logo_corner anchor consumers keep the roi as a pair)"
        );
    }

    #[test]
    fn pc_nav_to_field_cold_start_chain_is_walkable_to_field() {
        let dir = workspace_templates_root()
            .join("pipelines")
            .join("nav_to_field_pc");
        let defs = load_pipeline(&dir).expect("nav_to_field_pc must load");
        assert!(
            !defs.is_empty(),
            "nav_to_field_pc must contain at least FieldHudTopPc"
        );

        let by_name: std::collections::HashMap<&str, &TaskDef> =
            defs.iter().map(|d| (d.name.as_str(), d)).collect();

        let terminal = by_name
            .get("FieldHudTopPc")
            .expect("FieldHudTopPc (terminal) must exist in nav_to_field_pc");
        assert_eq!(
            terminal.next.as_deref(),
            Some(&[][..]),
            "FieldHudTopPc is the field-arrival terminal: next must be empty"
        );

        for d in &defs {
            let next = d.next.clone().unwrap_or_default();
            for target in &next {
                assert!(
                    by_name.contains_key(target.as_str()),
                    "{}: next target '{}' does not exist in nav_to_field_pc \
                     (dangling chain link blocks cold-start)",
                    d.name,
                    target
                );
            }
        }

        let mut indegree: std::collections::HashMap<&str, usize> =
            defs.iter().map(|d| (d.name.as_str(), 0usize)).collect();
        for d in &defs {
            for target in d.next.as_deref().unwrap_or(&[]) {
                if *target != d.name
                    && let Some(slot) = indegree.get_mut(target.as_str())
                {
                    *slot += 1;
                }
            }
        }
        let starts: Vec<&str> = indegree
            .iter()
            .filter(|(_, deg)| **deg == 0)
            .map(|(&name, _)| name)
            .collect();
        assert_eq!(
            starts.len(),
            1,
            "nav_to_field_pc must have exactly one start (indegree=0); found {:?}. \
             Multiple starts make cold-start entry ambiguous.",
            starts
        );
        assert_eq!(
            starts[0], "TapToStartPc",
            "the sole start must be TapToStartPc (title cold-start entry)"
        );

        let start = "TapToStartPc";
        let target_terminal = "FieldHudTopPc";
        let mut visited: std::collections::HashSet<&str> = std::collections::HashSet::new();
        let mut frontier: std::collections::VecDeque<&str> = std::collections::VecDeque::new();
        frontier.push_back(start);
        visited.insert(start);
        let mut reached_terminal = false;
        while let Some(node) = frontier.pop_front() {
            if node == target_terminal {
                reached_terminal = true;
                break;
            }
            let def = by_name
                .get(node)
                .expect("walk node must be resolved by name (step2 guarantees existence)");
            for next in def.next.as_deref().unwrap_or(&[]) {
                if visited.insert(next.as_str()) {
                    frontier.push_back(next.as_str());
                }
            }
        }
        assert!(
            reached_terminal,
            "FieldHudTopPc is unreachable from start TapToStartPc by following `next` edges. \
             A broken start edge (TapToStartPc.next=[]) or a missing intermediate link leaves \
             the cold-start chain walkable only in part. Every node from start to terminal \
             must be connected via next."
        );

        let reachable: std::collections::HashSet<&str> = if reached_terminal {
            let mut v: std::collections::HashSet<&str> = std::collections::HashSet::new();
            let mut f: std::collections::VecDeque<&str> = std::collections::VecDeque::new();
            f.push_back(start);
            v.insert(start);
            while let Some(node) = f.pop_front() {
                let def = by_name.get(node).expect("resolved node");
                for next in def.next.as_deref().unwrap_or(&[]) {
                    if v.insert(next.as_str()) {
                        f.push_back(next.as_str());
                    }
                }
            }
            v
        } else {
            std::collections::HashSet::new()
        };
        for d in &defs {
            if d.name == "TapToStartPc" || d.name == "FieldHudTopPc" {
                continue;
            }
            assert!(
                reachable.contains(d.name.as_str()),
                "intermediate node '{}' is unreachable from start TapToStartPc: \
                 a disconnected sub-graph exists in nav_to_field_pc",
                d.name
            );
        }

        let points_at_field = defs.iter().any(|d| {
            d.next
                .as_ref()
                .map(|n| n.iter().any(|t| t == "FieldHudTopPc"))
                .unwrap_or(false)
        });
        assert!(
            points_at_field,
            "no slice transitions into FieldHudTopPc: the title->load->field chain is \
             incomplete (T3 title_pc/load_game slices not yet wired). Cold-start cannot \
             reach Field until an intermediate slice lists FieldHudTopPc in its next."
        );

        for d in &defs {
            if let Some(roi) = d.roi {
                assert_roi_within_1258x708(roi, &d.name);
            }
        }
    }

    #[test]
    fn pc_nav_to_field_one_cycle_walk_order_matches_cli_contract() {
        let dir = workspace_templates_root()
            .join("pipelines")
            .join("nav_to_field_pc");
        let defs = load_pipeline(&dir).expect("nav_to_field_pc must load");
        let by_name: std::collections::HashMap<&str, &TaskDef> =
            defs.iter().map(|d| (d.name.as_str(), d)).collect();

        const CLI_START_TASK: &str = "TapToStartPc";
        const EXPECTED_WALK_ORDER: [&str; 3] = ["TapToStartPc", "LoadGamePc", "FieldHudTopPc"];

        let mut actual_order: Vec<&str> = Vec::new();
        let mut visited: std::collections::HashSet<&str> = std::collections::HashSet::new();
        let mut cursor: Option<&str> = Some(CLI_START_TASK);
        while let Some(name) = cursor {
            if !visited.insert(name) {
                break;
            }
            actual_order.push(name);
            let def = by_name
                .get(name)
                .unwrap_or_else(|| panic!("walk node '{name}' must exist in nav_to_field_pc"));
            let next = def.next.as_deref().unwrap_or(&[]);
            assert!(
                next.len() <= 1,
                "{}: non-deterministic cold-start (next has {} targets {:?}); \
                 one-cycle CLI proof requires a single linear chain",
                name,
                next.len(),
                next
            );
            cursor = next.first().map(|s| s.as_str());
        }

        assert_eq!(
            actual_order, EXPECTED_WALK_ORDER,
            "live one-cycle walk order must be exactly {:?}. \
             The README/CLI invocation `anaden run --target windows ... TapToStartPc` \
             drives this ordered chain; any rename or reorder in nav_to_field_pc TOML \
             must be reflected here and in the README.",
            EXPECTED_WALK_ORDER
        );

        let terminal = by_name
            .get("FieldHudTopPc")
            .expect("FieldHudTopPc terminal must exist");
        assert_eq!(
            terminal.next.as_deref(),
            Some(&[][..]),
            "FieldHudTopPc must terminate the one-cycle walk (next=[]) so the live \
             proof completes in exactly one cycle"
        );
    }

    fn field_pc_dir() -> PathBuf {
        workspace_templates_root().join("scenes").join("field_pc")
    }

    #[test]
    fn pc_field_pc_scene_templates_load_and_validate() {
        let dir = field_pc_dir();
        let defs = load_pipeline(&dir).expect("field_pc scene dir must load");
        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        assert!(
            names.len() >= 3,
            "field_pc must contain template_01 + hud_top + hud_topright, got {names:?}"
        );

        for d in &defs {
            assert_eq!(
                d.algorithm,
                Algorithm::Ccoeff,
                "{}: PC field templates must use ccoeff",
                d.name
            );
            assert!(
                d.template.is_absolute(),
                "{}: template path must be absolute",
                d.name
            );
            assert!(
                d.template.exists(),
                "{}: template PNG must exist at {:?}",
                d.name,
                d.template
            );
            assert!(
                d.template
                    .parent()
                    .map(|p| p.ends_with("field_pc"))
                    .unwrap_or(false),
                "{}: template must live under scenes/field_pc/, got {:?}",
                d.name,
                d.template
            );
            let roi = d.roi.unwrap_or_else(|| {
                panic!("{}: PC field template must have an explicit ROI", d.name)
            });
            assert_roi_within_1258x708(roi, &d.name);
            assert!(
                d.threshold >= 0.95,
                "{}: ticket requires threshold>=0.95, got {}",
                d.name,
                d.threshold
            );
        }
    }

    #[test]
    fn pc_field_pc_namespace_does_not_collide_with_scenes_field() {
        let pc_dir = field_pc_dir();
        let legacy_dir = workspace_templates_root().join("scenes").join("field");
        assert!(pc_dir.exists(), "scenes/field_pc namespace must exist");
        assert!(
            legacy_dir.exists(),
            "legacy scenes/field must still exist (not clobbered)"
        );

        let pc_names: Vec<String> = load_pipeline(&pc_dir)
            .expect("field_pc load")
            .into_iter()
            .map(|d| d.name)
            .collect();
        let legacy_stems: Vec<String> = std::fs::read_dir(&legacy_dir)
            .expect("read scenes/field")
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let p = e.path();
                let is_toml = p
                    .extension()
                    .and_then(|x| x.to_str())
                    .map(|x| x.eq_ignore_ascii_case("toml"))
                    .unwrap_or(false);
                if is_toml {
                    p.file_stem()?.to_str().map(String::from)
                } else {
                    None
                }
            })
            .collect();
        for n in &pc_names {
            assert!(
                !legacy_stems.contains(n),
                "PC scene task '{n}' collides with legacy 20:9 scenes/field namespace"
            );
        }
        assert!(
            !pc_names.iter().any(|n| legacy_stems.contains(n)),
            "no PC task name may equal a legacy scenes/field filename stem"
        );
    }

    fn field_pc_probe_path() -> Option<PathBuf> {
        let primary = workspace_templates_root()
            .join("captures")
            .join("field_pc_probe.png");
        if primary.exists() {
            return Some(primary);
        }
        let fallback = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("capture_probe.png");
        if fallback.exists() {
            return Some(fallback);
        }
        None
    }

    #[test]
    #[cfg_attr(not(feature = "pc-e2e"), ignore)]
    fn pc_field_pc_templates_match_real_capture_above_threshold() {
        let dir = field_pc_dir();
        let probe_path = field_pc_probe_path().unwrap_or_else(|| {
            panic!(
                "field_pc_probe.png not found (neither templates/captures/ nor \
                 workspace-root capture_probe.png). PC-probe E2E requires a real \
                 PrintWindow capture — run with `--features pc-e2e --run-ignored all` only \
                 when the probe is provisioned."
            )
        });
        let defs = load_pipeline(&dir).expect("field_pc load");
        let screenshot = image::open(&probe_path).expect("open field_pc_probe.png");

        for d in &defs {
            let m = d
                .detect(&screenshot, Path::new(""))
                .unwrap_or_else(|e| panic!("{} detect error: {e}", d.name));
            let m = m.unwrap_or_else(|| {
                panic!(
                    "{}: must match field_pc_probe.png at threshold {} (got None)",
                    d.name, d.threshold
                )
            });
            assert!(
                m.confidence.0 >= d.threshold,
                "{}: confidence {} below threshold {} on real PC capture",
                d.name,
                m.confidence.0,
                d.threshold
            );
            assert_roi_within_1258x708(
                [m.region.x, m.region.y, m.region.width, m.region.height],
                &format!("{} match region", d.name),
            );
            println!(
                "{}: conf={:.4} region=[{},{},{},{}] (threshold {:.2})",
                d.name,
                m.confidence.0,
                m.region.x,
                m.region.y,
                m.region.width,
                m.region.height,
                d.threshold
            );
        }
    }

    #[test]
    #[cfg_attr(not(feature = "pc-e2e"), ignore)]
    fn pc_field_loop_pc_templates_match_real_capture_above_threshold() {
        let dir = workspace_templates_root()
            .join("pipelines")
            .join("field_loop_pc");
        let probe_path = field_pc_probe_path().unwrap_or_else(|| {
            panic!(
                "field_pc_probe.png not found (neither templates/captures/ nor \
                 workspace-root capture_probe.png). PC-probe E2E requires a real \
                 PrintWindow capture — run with `--features pc-e2e --run-ignored all` only \
                 when the probe is provisioned."
            )
        });
        let defs = load_pipeline(&dir).expect("field_loop_pc load");
        let screenshot = image::open(&probe_path).expect("open field_pc_probe.png");

        for d in &defs {
            let m = d
                .detect(&screenshot, Path::new(""))
                .unwrap_or_else(|e| panic!("{} detect error: {e}", d.name));
            let m = m.unwrap_or_else(|| {
                panic!(
                    "{}: must match field_pc_probe.png at threshold {} (got None)",
                    d.name, d.threshold
                )
            });
            assert!(
                m.confidence.0 >= d.threshold,
                "{}: confidence {} below threshold {} on real PC capture",
                d.name,
                m.confidence.0,
                d.threshold
            );
            assert_roi_within_1258x708(
                [m.region.x, m.region.y, m.region.width, m.region.height],
                &format!("{} match region", d.name),
            );
        }
    }

    fn menu_pc_dir() -> PathBuf {
        workspace_templates_root().join("scenes").join("menu_pc")
    }

    #[test]
    fn pc_menu_pc_scene_templates_load_and_validate() {
        let dir = menu_pc_dir();
        let defs = load_pipeline(&dir).expect("menu_pc scene dir must load");
        assert_eq!(
            defs.len(),
            7,
            "menu_pc must contain bag/board/gacha/grasta/info/party/record (7 tasks), got {}",
            defs.len()
        );

        for d in &defs {
            assert_eq!(
                d.algorithm,
                Algorithm::Sse,
                "{}: PC menu templates must use sse",
                d.name
            );
            assert_eq!(
                d.state, "menu_pc",
                "{}: PC menu task state must be 'menu_pc'",
                d.name
            );
            assert!(
                d.template.is_absolute(),
                "{}: template path must be absolute",
                d.name
            );
            assert!(
                d.template.exists(),
                "{}: template PNG must exist at {:?}",
                d.name,
                d.template
            );
            assert!(
                d.template
                    .parent()
                    .map(|p| p.ends_with("menu_pc"))
                    .unwrap_or(false),
                "{}: template must live under scenes/menu_pc/, got {:?}",
                d.name,
                d.template
            );
            let roi = d.roi.unwrap_or_else(|| {
                panic!("{}: PC menu template must have an explicit ROI", d.name)
            });
            assert_roi_within_1258x708(roi, &d.name);
            assert!(
                d.threshold >= 0.95,
                "{}: ticket requires threshold>=0.95, got {}",
                d.name,
                d.threshold
            );
        }
    }

    #[test]
    fn pc_menu_pc_namespace_does_not_collide_with_scenes_field_or_menu() {
        let pc_dir = menu_pc_dir();
        assert!(pc_dir.exists(), "scenes/menu_pc namespace must exist");

        let pc_names: Vec<String> = load_pipeline(&pc_dir)
            .expect("menu_pc load")
            .into_iter()
            .map(|d| d.name)
            .collect();

        for legacy_seg in ["field", "menu"] {
            let legacy_dir = workspace_templates_root().join("scenes").join(legacy_seg);
            assert!(
                legacy_dir.exists(),
                "legacy scenes/{legacy_seg} must still exist (not clobbered)"
            );
            let legacy_stems: Vec<String> = std::fs::read_dir(&legacy_dir)
                .unwrap_or_else(|e| panic!("read scenes/{legacy_seg}: {e}"))
                .filter_map(|e| e.ok())
                .filter_map(|e| {
                    let p = e.path();
                    let is_toml = p
                        .extension()
                        .and_then(|x| x.to_str())
                        .map(|x| x.eq_ignore_ascii_case("toml"))
                        .unwrap_or(false);
                    if is_toml {
                        p.file_stem()?.to_str().map(String::from)
                    } else {
                        None
                    }
                })
                .collect();
            for n in &pc_names {
                assert!(
                    !legacy_stems.contains(n),
                    "PC menu task '{n}' collides with legacy 20:9 scenes/{legacy_seg} namespace"
                );
            }
        }
        assert!(
            !pc_names.is_empty(),
            "menu_pc must define at least one task for namespace isolation check"
        );
    }

    fn menu_pc_probe_path() -> Option<PathBuf> {
        let primary = workspace_templates_root()
            .join("captures")
            .join("menu_pc_probe.png");
        if primary.exists() {
            return Some(primary);
        }
        let fallback = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("menu_pc_probe.png");
        if fallback.exists() {
            return Some(fallback);
        }
        None
    }

    #[test]
    #[cfg_attr(not(feature = "pc-e2e"), ignore)]
    fn pc_menu_pc_templates_match_real_capture_above_threshold() {
        let dir = menu_pc_dir();
        let probe_path = menu_pc_probe_path().unwrap_or_else(|| {
            panic!(
                "menu_pc_probe.png not found (neither templates/captures/ nor \
                 workspace root) or menu_pc namespace missing at {:?}. PC-probe E2E \
                 requires a real PrintWindow capture — run with `--features pc-e2e \
                 --run-ignored all` only when the probe is provisioned.",
                dir
            )
        });

        let defs = load_pipeline(&dir).expect("menu_pc load");
        assert!(
            defs.len() >= 7,
            "menu_pc must contain 7 bottom-bar icon tasks, got {}",
            defs.len()
        );

        let screenshot = image::open(&probe_path).expect("open menu_pc_probe.png");

        for d in &defs {
            let m = d
                .detect(&screenshot, Path::new(""))
                .unwrap_or_else(|e| panic!("{} detect error: {e}", d.name));
            let m = m.unwrap_or_else(|| {
                panic!(
                    "{}: must match menu_pc_probe.png at threshold {} (got None)",
                    d.name, d.threshold
                )
            });
            assert!(
                d.threshold >= 0.95,
                "{}: threshold {} below menu_pc acceptance floor 0.95",
                d.name,
                d.threshold
            );
            assert!(
                m.confidence.0 >= d.threshold,
                "{}: confidence {} below threshold {} on real PC menu capture",
                d.name,
                m.confidence.0,
                d.threshold
            );
            assert_roi_within_1258x708(
                [m.region.x, m.region.y, m.region.width, m.region.height],
                &format!("{} match region", d.name),
            );
            println!(
                "{}: conf={:.4} region=[{},{},{},{}] (threshold {:.2})",
                d.name,
                m.confidence.0,
                m.region.x,
                m.region.y,
                m.region.width,
                m.region.height,
                d.threshold
            );
        }
    }

    fn title_pc_dir() -> PathBuf {
        workspace_templates_root().join("scenes").join("title_pc")
    }

    #[test]
    fn pc_title_pc_scene_templates_load_and_validate() {
        let dir = title_pc_dir();
        assert!(dir.exists(), "scenes/title_pc namespace must exist (T3)");
        let defs = load_pipeline(&dir).expect("title_pc scene dir must load");
        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        assert!(
            names.contains(&"TitlePcVersionLabel"),
            "title_pc must contain TitlePcVersionLabel, got {names:?}"
        );
        assert!(
            names.contains(&"TitlePcLogoCorner"),
            "title_pc must contain TitlePcLogoCorner, got {names:?}"
        );

        for d in &defs {
            assert_eq!(
                d.algorithm,
                Algorithm::Ccoeff,
                "{}: PC title templates must use ccoeff",
                d.name
            );
            assert_eq!(
                d.state, "title_pc",
                "{}: PC title task state must be 'title_pc'",
                d.name
            );
            assert!(
                d.template.is_absolute(),
                "{}: template path must be absolute",
                d.name
            );
            assert!(
                d.template.exists(),
                "{}: template PNG must exist at {:?}",
                d.name,
                d.template
            );
            assert!(
                d.template.to_string_lossy().contains("title_pc"),
                "{}: template must live under scenes/title_pc/, got {:?}",
                d.name,
                d.template
            );
            let roi = d.roi.expect("title_pc template must have an ROI");
            assert_roi_within_1258x708(roi, &d.name);
            assert!(
                roi[2] <= 180 && roi[3] <= 180,
                "{}: sub-template ROI {:?} exceeds small-template ceiling (~180px) \
                 (large templates are background-diff sensitive per TASKS.md:30-33)",
                d.name,
                roi
            );
        }
    }

    #[test]
    fn pc_title_pc_namespace_does_not_collide() {
        let pc_dir = title_pc_dir();
        assert!(pc_dir.exists(), "scenes/title_pc namespace must exist");
        let pc_names: Vec<String> = load_pipeline(&pc_dir)
            .expect("title_pc load")
            .into_iter()
            .map(|d| d.name)
            .collect();
        assert!(
            !pc_names.is_empty(),
            "title_pc must define at least one task for namespace isolation check"
        );
        let forbidden = [
            "title_center",
            "load_game_area",
            "TitleCenter",
            "LoadGameArea",
        ];
        for n in &pc_names {
            assert!(
                !forbidden.contains(&n.as_str()),
                "title_pc task '{n}' collides with legacy 20:9 large-template namespace"
            );
        }
        for d in load_pipeline(&pc_dir).expect("title_pc reload") {
            assert_ne!(
                d.state, "Title",
                "title_pc task {} state must be pc-scoped 'title_pc', not legacy 'Title'",
                d.name
            );
        }
    }

    fn title_pc_probe_path() -> Option<PathBuf> {
        let primary = workspace_templates_root()
            .join("captures")
            .join("title_pc_probe.png");
        if primary.exists() {
            return Some(primary);
        }
        let fallback = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("title_pc_probe.png");
        if fallback.exists() {
            return Some(fallback);
        }
        None
    }

    #[test]
    #[cfg_attr(not(feature = "pc-e2e"), ignore)]
    fn pc_title_pc_templates_match_real_capture_above_threshold() {
        let dir = title_pc_dir();
        let probe_path = title_pc_probe_path().unwrap_or_else(|| {
            panic!(
                "title_pc_probe.png not found (neither templates/captures/ nor \
                 workspace root) or title_pc namespace missing at {:?}. T3 title_pc \
                 sub-templates are provisionally placed in RAW-1258 space; real-probe E2E \
                 is deferred until a PC title-frame capture is provisioned (Issue #12). \
                 Run with `--features pc-e2e --run-ignored all` only when the probe exists.",
                dir
            )
        });
        let defs = load_pipeline(&dir).expect("title_pc load");
        let probe = image::open(&probe_path).expect("open title_pc_probe.png");

        let screenshot = crate::scale::ScreenScaler::new()
            .normalize_capture(&probe)
            .0;
        let (sw, sh) = (screenshot.width(), screenshot.height());

        for d in &defs {
            if d.name == "TitlePcVersionLabel" {
                println!(
                    "TitlePcVersionLabel: skipped — content-variable asset (version/ID text \
                     changes across captures, Issue #208); not a cross-capture probe-match target"
                );
                continue;
            }
            let m = d
                .detect(&screenshot, Path::new(""))
                .unwrap_or_else(|e| panic!("{} detect error: {e}", d.name));
            let m = m.unwrap_or_else(|| {
                panic!(
                    "{}: must match title_pc_probe.png at threshold {} (got None)",
                    d.name, d.threshold
                )
            });
            assert!(
                m.confidence.0 >= d.threshold,
                "{}: confidence {} below threshold {} on real PC title capture \
                 (ROI/threshold may need re-derivation against the real probe)",
                d.name,
                m.confidence.0,
                d.threshold
            );
            assert!(
                m.region.x + m.region.width <= sw && m.region.y + m.region.height <= sh,
                "{}: match region [{},{},{},{}] must fit within normalized probe {}x{}",
                d.name,
                m.region.x,
                m.region.y,
                m.region.width,
                m.region.height,
                sw,
                sh
            );
            println!(
                "{}: conf={:.4} region=[{},{},{},{}] (threshold {:.2}) on {}x{} normalized probe",
                d.name,
                m.confidence.0,
                m.region.x,
                m.region.y,
                m.region.width,
                m.region.height,
                d.threshold,
                sw,
                sh
            );
        }
    }

    #[test]
    #[cfg_attr(not(feature = "pc-e2e"), ignore)]
    fn pc_nav_to_field_pc_load_game_matches_title_probe() {
        let dir = workspace_templates_root()
            .join("pipelines")
            .join("nav_to_field_pc");
        let defs = load_pipeline(&dir).expect("nav_to_field_pc must load");
        let load = defs
            .iter()
            .find(|d| d.name == "LoadGamePc")
            .expect("LoadGamePc must exist in nav_to_field_pc");

        let probe_path = title_pc_probe_path().unwrap_or_else(|| {
            panic!(
                "title_pc_probe.png not found (neither templates/captures/ nor \
                 workspace root). LoadGamePc must match the title probe (the logo \
                 corner persists after Tap-to-Start) — run with `--features pc-e2e \
                 --run-ignored all` only when the probe exists, as with the other \
                 pc_* E2E tests"
            )
        });
        let probe = image::open(&probe_path).expect("open title_pc_probe.png");

        let screenshot = crate::scale::ScreenScaler::new()
            .normalize_capture(&probe)
            .0;

        let m = load
            .detect(&screenshot, Path::new(""))
            .unwrap_or_else(|e| panic!("LoadGamePc detect error: {e}"));
        let m = m.unwrap_or_else(|| {
            panic!(
                "LoadGamePc: must match title_pc_probe.png at threshold {} (got None) — \
                 roi {:?} must point at the needle re-crop origin [624,263,120,120] \
                 (Issue #192)",
                load.threshold, load.roi
            )
        });
        assert!(
            m.confidence.0 >= load.threshold,
            "LoadGamePc: confidence {} below threshold {} on real PC title capture \
             (roi/threshold may need re-derivation against the real probe)",
            m.confidence.0,
            load.threshold
        );
        println!(
            "LoadGamePc: conf={:.4} region=[{},{},{},{}] (threshold {:.2}) on {}x{} \
             normalized title probe",
            m.confidence.0,
            m.region.x,
            m.region.y,
            m.region.width,
            m.region.height,
            load.threshold,
            screenshot.width(),
            screenshot.height()
        );
    }

    #[test]
    #[cfg_attr(not(feature = "pc-e2e"), ignore)]
    fn pc_login_tap_title_matches_title_probe() {
        let dir = workspace_templates_root().join("pipelines").join("login");
        let defs = load_pipeline(&dir).expect("login must load");
        let tap = defs
            .iter()
            .find(|d| d.name == "LoginTapTitlePc")
            .expect("LoginTapTitlePc must exist in login pipeline");

        let probe_path = title_pc_probe_path().unwrap_or_else(|| {
            panic!(
                "title_pc_probe.png not found (neither templates/captures/ nor \
                 workspace root). LoginTapTitlePc must match the title probe — run with \
                 `--features pc-e2e --run-ignored all` only when the probe exists, as with \
                 the other pc_* E2E tests"
            )
        });
        let probe = image::open(&probe_path).expect("open title_pc_probe.png");

        let screenshot = crate::scale::ScreenScaler::new()
            .normalize_capture(&probe)
            .0;

        let m = tap
            .detect(&screenshot, Path::new(""))
            .unwrap_or_else(|e| panic!("LoginTapTitlePc detect error: {e}"));
        let m = m.unwrap_or_else(|| {
            panic!(
                "LoginTapTitlePc: must match title_pc_probe.png at threshold {} (got None) — \
                 the logo_corner anchor (Issue #208) with slack roi {:?} must fire on the \
                 probe capture too",
                tap.threshold, tap.roi
            )
        });
        assert!(
            m.confidence.0 >= tap.threshold,
            "LoginTapTitlePc: confidence {} below threshold {} on real PC title capture \
             (roi/threshold may need re-derivation against the real probe)",
            m.confidence.0,
            tap.threshold
        );
        println!(
            "LoginTapTitlePc: conf={:.4} region=[{},{},{},{}] (threshold {:.2}) on {}x{} \
             normalized title probe",
            m.confidence.0,
            m.region.x,
            m.region.y,
            m.region.width,
            m.region.height,
            tap.threshold,
            screenshot.width(),
            screenshot.height()
        );
    }

    #[test]
    fn pc_title_pc_readme_resume_procedure_matches_code_facts() {
        let readme =
            std::fs::read_to_string(workspace_templates_root().join("..").join("README.md"))
                .expect("README.md must be readable from anaden-vision");

        assert!(
            readme.contains("PC 版 title cold-start 再開手順"),
            "README must contain the resume procedure section (Issue #12 Branch B)"
        );
        assert!(
            readme.contains("title_pc_probe.png"),
            "README procedure must reference the probe artifact title_pc_probe.png"
        );
        assert!(
            readme.contains("pc_title_pc_templates_match_real_capture_above_threshold"),
            "README procedure must name the E2E test to run on device reconnect"
        );

        let defs = load_pipeline(&title_pc_dir()).expect("title_pc scene dir must load");
        let by_name: std::collections::HashMap<&str, &TaskDef> =
            defs.iter().map(|d| (d.name.as_str(), d)).collect();

        let version_label = by_name
            .get("TitlePcVersionLabel")
            .expect("TitlePcVersionLabel must exist in title_pc scene");
        let vl_roi = version_label.roi.expect("version_label must have roi");
        let vl_roi_str = format!("[{},{},{},{}]", vl_roi[0], vl_roi[1], vl_roi[2], vl_roi[3]);
        assert!(
            readme.contains(&vl_roi_str),
            "README must cite version_label roi {} (matched against real TOML)",
            vl_roi_str
        );
        let vl_thr_str = format!("threshold = {}", version_label.threshold);
        let _ = vl_thr_str;

        let logo_corner = by_name
            .get("TitlePcLogoCorner")
            .expect("TitlePcLogoCorner must exist in title_pc scene");
        let lc_roi = logo_corner.roi.expect("title_logo_corner must have roi");
        let lc_roi_str = format!("[{},{},{},{}]", lc_roi[0], lc_roi[1], lc_roi[2], lc_roi[3]);
        assert!(
            readme.contains(&lc_roi_str),
            "README must cite title_logo_corner roi {} (matched against real TOML)",
            lc_roi_str
        );

        assert!(
            readme.contains("title_center.png") && readme.contains("load_game_area.png"),
            "README must cross-reference the 20:9 large-template known blockers \
             (title_center.png / load_game_area.png) as the design rationale"
        );

        assert!(
            readme.contains("title_pc_probe_path")
                && readme.contains("pc-e2e")
                && readme.contains("--run-ignored"),
            "README must cite title_pc_probe_path() and the R1 three-valued gate \
             (pc-e2e feature + --run-ignored command)"
        );
    }

    #[test]
    fn pc_title_pc_readme_skip_reason_describes_r1_gate_not_absence_skip() {
        let readme =
            std::fs::read_to_string(workspace_templates_root().join("..").join("README.md"))
                .expect("README.md must be readable from anaden-vision");

        assert!(
            readme.contains("absence-skip") && readme.contains("pc-e2e"),
            "README skip-reason prose must be migrated to the R1 three-valued gate \
             (mention pc-e2e); pure absence-skip-return description is obsolete under R1"
        );
    }

    fn title_region_runs(gray: &GrayImage, y0: u32, y1: u32, xs: u32, xe: u32) -> Vec<(u32, u32)> {
        let band_h = (y1 - y0) as usize;
        let mut col_var: Vec<u64> = Vec::with_capacity((xe - xs) as usize);
        for x in xs..xe {
            let mut vals: Vec<u8> = Vec::with_capacity(band_h);
            for y in y0..y1 {
                vals.push(gray.get_pixel(x, y).0[0]);
            }
            let mean = vals.iter().map(|v| *v as u64).sum::<u64>() as f64 / vals.len() as f64;
            let var: u64 = vals
                .iter()
                .map(|v| {
                    let d = *v as f64 - mean;
                    (d * d) as u64
                })
                .sum();
            col_var.push(var);
        }
        let max_var = *col_var.iter().max().unwrap_or(&1) as f64;
        let sm: Vec<f64> = (0..col_var.len())
            .map(|i| {
                let a = col_var[i.saturating_sub(1)];
                let b = col_var[i];
                let c = col_var[(i + 1).min(col_var.len() - 1)];
                (a + b + c) as f64 / 3.0
            })
            .collect();
        let thr = max_var * 0.10;
        let mut runs: Vec<(u32, u32)> = Vec::new();
        let mut i = 0;
        while i < sm.len() {
            if sm[i] > thr {
                let s = i;
                while i < sm.len() && sm[i] > thr {
                    i += 1;
                }
                runs.push((xs + s as u32, xs + i as u32));
            } else {
                i += 1;
            }
        }
        runs
    }

    #[test]
    fn pc_title_pc_roi_derivation_matches_column_variance_runs_or_documents_gap() {
        let defs = load_pipeline(&title_pc_dir()).expect("title_pc scene dir must load");
        let by_name: std::collections::HashMap<&str, &TaskDef> =
            defs.iter().map(|d| (d.name.as_str(), d)).collect();

        let version_label = by_name
            .get("TitlePcVersionLabel")
            .expect("TitlePcVersionLabel must exist");
        let logo_corner = by_name
            .get("TitlePcLogoCorner")
            .expect("TitlePcLogoCorner must exist");

        assert_eq!(
            version_label.roi,
            Some([8, 2, 138, 20]),
            "version_label ROI must be the top-left ID label band [8,2,138,20] \
             (Issue #182 live E2E run-180, PC RAW 1258x708 space; template regenerated \
             138x20 stddev 53.1). The prior [712,8,121,35] top-right version band crop \
             was structurally flat (stddev 3.76) and could never match; the even older \
             Issue #13 provisional [1046,668,112,28]=bottom-right was a 20:9 self-crop \
             artifact"
        );
        assert_eq!(
            logo_corner.roi,
            Some([604, 223, 160, 180]),
            "title_logo_corner ROI must be the drift-absorbing slack window [604,223,160,180] (Issue #210)
             around the small-feature anchor [624,263,120,120] \
             (selected by find_logo_corner_subfeature.rs as the highest-energy 120x120 \
             window inside the operator-dragged band [164,63,932,405] on title_pc_probe.png \
             2026-07-07, PC RAW 1258x708 space). The full 932x405 band exceeded the \
             small-template ceiling (roi[2]<=130 && roi[3]<=130) ~7x and is background-diff \
             sensitive; DEFER option (b) re-cropped it into a stable small feature. The old \
             Issue #13 provisional [140,60,60,60] was a 20:9 self-crop artifact"
        );

        let probe_path = match title_pc_probe_path() {
            Some(p) => p,
            None => {
                eprintln!(
                    "skip geometric corroboration: title_pc_probe.png not found. ROI retain \
                     contract (A) still verified above; the probe is tracked so a fresh clone \
                     should contain it — re-run after `git checkout templates/captures/`."
                );
                return;
            }
        };

        let probe = image::open(&probe_path).expect("open title_pc_probe.png");
        assert_eq!(
            (probe.width(), probe.height()),
            (1918, 1048),
            "title_pc_probe.png raw capture must be 1918x1048 RGBA (operator window size, \
             not operator-controllable); the driver normalizes it via crop_to_content + \
             normalize. If the capture dimensions change the normalization premise must be \
             re-validated."
        );

        let normalized = crate::scale::ScreenScaler::new()
            .normalize_capture(&probe)
            .0;
        let gray = normalized.to_luma8();
        let norm_w = normalized.width();
        let norm_h = normalized.height();

        let vl_roi = version_label.roi.expect("(A) pinned version_label roi");
        let vl = crate::scale::roi_to_normalized(vl_roi, norm_w, norm_h);
        let vl_runs = title_region_runs(
            &gray,
            vl[1],
            vl[1] + vl[3],
            vl[0].saturating_sub(12),
            vl[0] + vl[2] + 12,
        );
        assert!(
            vl_runs
                .iter()
                .any(|(s, e)| *e > vl[0] && *s < vl[0] + vl[2]),
            "version_label: normalized-probe band must contain a column-variance run \
             overlapping the scaled ROI {:?}. This is the geometric corroboration the old \
             retain-old-value guard was waiting for; got runs={vl_runs:?}",
            vl
        );

        let lc_roi = logo_corner.roi.expect("(A) pinned logo_corner roi");
        let lc = crate::scale::roi_to_normalized(lc_roi, norm_w, norm_h);
        let lc_runs = title_region_runs(
            &gray,
            lc[1],
            lc[1] + lc[3],
            lc[0].saturating_sub(12),
            lc[0] + lc[2] + 12,
        );
        assert!(
            lc_runs
                .iter()
                .any(|(s, e)| *e > lc[0] && *s < lc[0] + lc[2]),
            "title_logo_corner: normalized-probe band must contain a column-variance run \
             overlapping the scaled small-feature ROI {:?} (selected by \
             find_logo_corner_subfeature.rs inside the operator-dragged band). got runs={lc_runs:?}",
            lc
        );
    }

    #[test]
    fn pc_title_pc_version_label_template_has_structure_above_stddev_threshold() {
        let path = title_pc_dir().join("version_label.png");
        let img = image::open(&path).expect("version_label.png must open");
        let stddev = crate::template_quality::template_luma_stddev(&img);
        assert!(
            crate::template_quality::template_is_structured(&img),
            "version_label.png luminance stddev {stddev:.2} must be >= {:.1} — a \
             near-uniform template (the pre-Issue-#182 one measured 3.76) can never match \
             under TM_CCOEFF_NORMED (real-device evidence: 65 iterations, 0 fires)",
            crate::template_quality::TEMPLATE_MIN_LUMA_STDDEV
        );
    }

    #[test]
    fn pc_title_pc_logo_corner_consumers_share_matching_roi_and_template_dims() {
        const EXPECTED_ROI: [u32; 4] = [604, 223, 160, 180];
        const ANCHOR_ORIGIN: [u32; 4] = [624, 263, 120, 120];
        const SLACK_X: u32 = 20;
        const SLACK_DOWN: u32 = 20;
        const SLACK_UP: u32 = 40;
        let pipelines = workspace_templates_root().join("pipelines");
        let consumers = [
            (
                "login/tap_title",
                pipelines.join("login").join("tap_title.toml"),
            ),
            (
                "nav_to_field_pc/tap_to_start",
                pipelines.join("nav_to_field_pc").join("tap_to_start.toml"),
            ),
            (
                "nav_to_field_pc/load_game",
                pipelines.join("nav_to_field_pc").join("load_game.toml"),
            ),
        ];
        for (label, path) in consumers {
            let body = std::fs::read_to_string(&path)
                .expect("consumer TOML must be readable (repo-tracked template bank)");
            let def = toml::from_str::<TaskDef>(&body)
                .unwrap_or_else(|e| panic!("{label}: TOML parse failed: {e}"));
            assert_eq!(
                def.roi,
                Some(EXPECTED_ROI),
                "{label}: logo_corner consumer roi must be the slack window \
                 [604,223,160,180] — the shared needle is 120x120 and cross-capture drift \
                 (measured (+2,-1) px at #208, then ~20px upward at Issue #210 live \
                 verification) needs slack; a needle-exact roi ([624,263,120,120]) \
                 collapses conf 0.99->0.78 when the best placement falls outside the roi. \
                 Template swaps and roi updates must land as a pair across all three \
                 consumers",
            );
            assert!(
                def.template
                    .to_string_lossy()
                    .contains("title_logo_corner.png"),
                "{label}: consumer must reference title_logo_corner.png (Issue #208 stable \
                 anchor), got {:?}",
                def.template
            );
        }

        let img = image::open(title_pc_dir().join("title_logo_corner.png"))
            .expect("title_logo_corner.png must open");
        assert_eq!(
            (img.width(), img.height()),
            (ANCHOR_ORIGIN[2], ANCHOR_ORIGIN[3]),
            "title_logo_corner.png dims {}x{} must equal the anchor origin w/h {}x{} \
             (the slack roi is derived from this crop origin)",
            img.width(),
            img.height(),
            ANCHOR_ORIGIN[2],
            ANCHOR_ORIGIN[3]
        );
        assert_eq!(
            (
                ANCHOR_ORIGIN[0] - SLACK_X,
                ANCHOR_ORIGIN[1] - SLACK_UP,
                ANCHOR_ORIGIN[2] + 2 * SLACK_X,
                ANCHOR_ORIGIN[3] + SLACK_UP + SLACK_DOWN,
            ),
            (
                EXPECTED_ROI[0],
                EXPECTED_ROI[1],
                EXPECTED_ROI[2],
                EXPECTED_ROI[3]
            ),
            "slack window must be anchor padded x+/-{SLACK_X} / up-{SLACK_UP} down+{SLACK_DOWN} \n             (#210 live upward ~20px drift measured; top headlane is wider)",
        );
    }

    #[test]
    fn pc_title_pc_version_label_scene_def_roi_matches_template_dims() {
        const EXPECTED_ROI: [u32; 4] = [8, 2, 138, 20];
        let path = title_pc_dir().join("version_label.toml");
        let body = std::fs::read_to_string(&path).expect("version_label.toml must be readable");
        let def = toml::from_str::<TaskDef>(&body)
            .unwrap_or_else(|e| panic!("version_label.toml parse failed: {e}"));
        assert_eq!(
            def.roi,
            Some(EXPECTED_ROI),
            "version_label scene roi must stay [8,2,138,20] paired with the 138x20 needle \
             (scenes-only detector since Issue #208; a needle-wider-than-roi regression is \
             a permanent NoMatch)"
        );
        let img = image::open(title_pc_dir().join("version_label.png"))
            .expect("version_label.png must open");
        assert_eq!(
            (img.width(), img.height()),
            (EXPECTED_ROI[2], EXPECTED_ROI[3]),
            "version_label.png dims {}x{} must equal the scene roi w/h {}x{}",
            img.width(),
            img.height(),
            EXPECTED_ROI[2],
            EXPECTED_ROI[3]
        );
    }

    #[test]
    fn load_pipeline_skips_pipeline_manifest_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_toml(
            tmp.path(),
            "pipeline.toml",
            r#"
            start_task = "TapBottom"

            [[goal]]
            name = "farm50"
            [goal.stop]
            LoopCount = { target = 50 }
            "#,
        );

        let defs = load_pipeline(tmp.path()).expect("load must skip manifest, no error");
        assert!(
            defs.is_empty(),
            "pipeline.toml must NOT be parsed as a TaskDef; got {defs:?}"
        );
    }

    #[test]
    fn manifest_parses_start_task_and_loop_count_goal() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_toml(
            tmp.path(),
            "pipeline.toml",
            r#"
            start_task = "TapBottom"

            [[goal]]
            name = "farm50"
            [goal.stop]
            LoopCount = { target = 50 }
            "#,
        );

        let manifest = load_pipeline_manifest(tmp.path()).expect("manifest must load");
        assert_eq!(manifest.start_task, "TapBottom");
        assert_eq!(manifest.goals.len(), 1, "exactly one goal");
        let goal = &manifest.goals[0];
        assert_eq!(goal.name, "farm50");
        assert_eq!(
            goal.stop,
            anaden_core::StopCondition::LoopCount { target: 50 }
        );
    }

    #[test]
    fn manifest_parses_template_match_goal() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_toml(
            tmp.path(),
            "pipeline.toml",
            r#"
            start_task = "DetectClear"

            [[goal]]
            name = "find_clear"
            [goal.stop.TemplateMatch]
            task = "clear_button"
            confidence = 0.85
            "#,
        );

        let manifest = load_pipeline_manifest(tmp.path()).expect("manifest must load");
        assert_eq!(manifest.start_task, "DetectClear");
        let goal = &manifest.goals[0];
        assert_eq!(goal.name, "find_clear");
        assert_eq!(
            goal.stop,
            anaden_core::StopCondition::TemplateMatch {
                task: "clear_button".to_string(),
                confidence: 0.85
            }
        );
    }

    #[test]
    fn manifest_parses_timeout_goal() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_toml(
            tmp.path(),
            "pipeline.toml",
            r#"
            start_task = "Farm"

            [[goal]]
            name = "one_hour_limit"
            [goal.stop]
            Timeout = { secs = 3600 }
            "#,
        );

        let manifest = load_pipeline_manifest(tmp.path()).expect("manifest must load");
        assert_eq!(manifest.start_task, "Farm");
        let goal = &manifest.goals[0];
        assert_eq!(goal.name, "one_hour_limit");
        assert_eq!(
            goal.stop,
            anaden_core::StopCondition::Timeout { secs: 3600 }
        );
    }

    #[test]
    fn manifest_without_goals_loads_as_empty_goals_for_backcompat() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_toml(
            tmp.path(),
            "pipeline.toml",
            r#"
            start_task = "TapBottom"
            "#,
        );

        let manifest = load_pipeline_manifest(tmp.path()).expect("manifest must load");
        assert_eq!(manifest.start_task, "TapBottom");
        assert!(
            manifest.goals.is_empty(),
            "no [[goal]] declaration => empty goals => infinite loop (backcompat)"
        );
    }

    #[test]
    fn manifest_missing_start_task_rejected() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_toml(
            tmp.path(),
            "pipeline.toml",
            r#"
            [[goal]]
            name = "g"
            [goal.stop]
            LoopCount = { target = 1 }
            "#,
        );

        let err = load_pipeline_manifest(tmp.path()).expect_err("start_task is required");
        assert!(matches!(err, TaskDefError::ParseFailed { .. }));
    }

    #[test]
    fn manifest_rejects_unknown_top_field() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_toml(
            tmp.path(),
            "pipeline.toml",
            r#"
            start_task = "X"
            bogus = true
            "#,
        );

        let err = load_pipeline_manifest(tmp.path()).expect_err("unknown field must reject");
        assert!(matches!(err, TaskDefError::ParseFailed { .. }));
    }

    #[test]
    fn manifest_rejects_unknown_goal_field() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_toml(
            tmp.path(),
            "pipeline.toml",
            r#"
            start_task = "X"

            [[goal]]
            name = "g"
            bogus = 1
            [goal.stop]
            LoopCount = { target = 1 }
            "#,
        );

        let err = load_pipeline_manifest(tmp.path()).expect_err("unknown goal field must reject");
        assert!(matches!(err, TaskDefError::ParseFailed { .. }));
    }

    #[test]
    fn manifest_missing_file_returns_error() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let err = load_pipeline_manifest(tmp.path()).expect_err("missing manifest must error");
        assert!(matches!(err, TaskDefError::ParseFailed { .. }));
    }

    #[test]
    fn manifest_parses_all_inline_table_form() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_toml(
            tmp.path(),
            "pipeline.toml",
            r#"
            start_task = "Farm"

            [[goal]]
            name = "loop_and_timeout"
            [goal.stop.All]
            conditions = [
              { LoopCount = { target = 50 } },
              { Timeout = { secs = 3600 } },
            ]
            "#,
        );

        let manifest = load_pipeline_manifest(tmp.path()).expect("manifest must load");
        let goal = &manifest.goals[0];
        assert_eq!(goal.name, "loop_and_timeout");
        match &goal.stop {
            anaden_core::StopCondition::All { conditions } => {
                assert_eq!(conditions.len(), 2, "All must carry 2 children");
                assert_eq!(
                    conditions[0],
                    anaden_core::StopCondition::LoopCount { target: 50 }
                );
                assert_eq!(
                    conditions[1],
                    anaden_core::StopCondition::Timeout { secs: 3600 }
                );
            }
            other => panic!("expected All, got {other:?}"),
        }
    }

    #[test]
    fn manifest_parses_any_subtable_array_form() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_toml(
            tmp.path(),
            "pipeline.toml",
            r#"
            start_task = "Farm"

            [[goal]]
            name = "match_either"
            [goal.stop.Any]
            [[goal.stop.Any.conditions]]
            TemplateMatch = { task = "clear_button", confidence = 0.85 }
            [[goal.stop.Any.conditions]]
            TemplateMatch = { task = "boss_dead", confidence = 0.9 }
            "#,
        );

        let manifest = load_pipeline_manifest(tmp.path()).expect("manifest must load");
        let goal = &manifest.goals[0];
        match &goal.stop {
            anaden_core::StopCondition::Any { conditions } => {
                assert_eq!(conditions.len(), 2, "Any must carry 2 children in order");
                assert!(matches!(
                    conditions[0],
                    anaden_core::StopCondition::TemplateMatch { ref task, .. } if task == "clear_button"
                ));
                assert!(matches!(
                    conditions[1],
                    anaden_core::StopCondition::TemplateMatch { ref task, .. } if task == "boss_dead"
                ));
            }
            other => panic!("expected Any, got {other:?}"),
        }
    }

    #[test]
    fn manifest_parses_nested_all_containing_any() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_toml(
            tmp.path(),
            "pipeline.toml",
            r#"
            start_task = "Farm"

            [[goal]]
            name = "complex"
            [goal.stop.All]
            conditions = [
              { LoopCount = { target = 10 } },
              { Any = { conditions = [ { Timeout = { secs = 60 } }, { LoopCount = { target = 5 } } ] } },
            ]
            "#,
        );

        let manifest = load_pipeline_manifest(tmp.path()).expect("manifest must load");
        let goal = &manifest.goals[0];
        match &goal.stop {
            anaden_core::StopCondition::All { conditions } => {
                assert_eq!(conditions.len(), 2);
                assert!(matches!(
                    conditions[0],
                    anaden_core::StopCondition::LoopCount { target: 10 }
                ));
                match &conditions[1] {
                    anaden_core::StopCondition::Any { conditions: inner } => {
                        assert_eq!(inner.len(), 2);
                        assert!(matches!(
                            inner[0],
                            anaden_core::StopCondition::Timeout { secs: 60 }
                        ));
                        assert!(matches!(
                            inner[1],
                            anaden_core::StopCondition::LoopCount { target: 5 }
                        ));
                    }
                    other => panic!("expected nested Any, got {other:?}"),
                }
            }
            other => panic!("expected All, got {other:?}"),
        }
    }

    #[test]
    fn manifest_rejects_unknown_field_in_composition_child() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_toml(
            tmp.path(),
            "pipeline.toml",
            r#"
            start_task = "Farm"

            [[goal]]
            name = "bad_child"
            [goal.stop.All]
            conditions = [
              { LoopCount = { target = 50 } },
              { LoopCount = { target = 1, bogus = 2 } },
            ]
            "#,
        );

        let err = load_pipeline_manifest(tmp.path())
            .expect_err("unknown field inside composition child must reject");
        assert!(matches!(err, TaskDefError::ParseFailed { .. }));
    }

    #[test]
    fn manifest_rejects_unknown_field_on_all_variant() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_toml(
            tmp.path(),
            "pipeline.toml",
            r#"
            start_task = "Farm"

            [[goal]]
            name = "bad_all"
            [goal.stop.All]
            extra_key = true
            conditions = [{ LoopCount = { target = 1 } }]
            "#,
        );

        let err = load_pipeline_manifest(tmp.path()).expect_err("unknown field on All must reject");
        assert!(matches!(err, TaskDefError::ParseFailed { .. }));
    }

    #[test]
    fn manifest_parses_empty_conditions_without_runtime_panic() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_toml(
            tmp.path(),
            "pipeline.toml",
            r#"
            start_task = "Farm"

            [[goal]]
            name = "empty_all"
            [goal.stop.All]
            conditions = []
            "#,
        );

        let parsed = load_pipeline_manifest(tmp.path());
        let manifest = parsed.expect("empty conditions parses at manifest layer (no panic)");
        assert_eq!(manifest.goals.len(), 1);
        assert!(matches!(
            &manifest.goals[0].stop,
            anaden_core::StopCondition::All { conditions } if conditions.is_empty()
        ));
    }

    #[test]
    fn manifest_parses_multiple_goals_in_order() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_toml(
            tmp.path(),
            "pipeline.toml",
            r#"
            start_task = "Farm"

            [[goal]]
            name = "loop_50"
            [goal.stop]
            LoopCount = { target = 50 }

            [[goal]]
            name = "timeout_1h"
            [goal.stop]
            Timeout = { secs = 3600 }
            "#,
        );

        let manifest = load_pipeline_manifest(tmp.path()).expect("manifest must load");
        assert_eq!(manifest.start_task, "Farm");
        assert_eq!(manifest.goals.len(), 2, "two goals in declaration order");
        assert_eq!(manifest.goals[0].name, "loop_50");
        assert_eq!(manifest.goals[1].name, "timeout_1h");
        assert_eq!(
            manifest.goals[0].stop,
            anaden_core::StopCondition::LoopCount { target: 50 }
        );
        assert_eq!(
            manifest.goals[1].stop,
            anaden_core::StopCondition::Timeout { secs: 3600 }
        );
    }

    #[test]
    fn load_pipeline_and_load_pipeline_manifest_are_separated() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_toml(
            tmp.path(),
            "tap_bottom.toml",
            r#"
            name = "TapBottom"
            state = "Field"
            algorithm = "ccoeff"
            template = "t.png"
            "#,
        );
        write_toml(
            tmp.path(),
            "pipeline.toml",
            r#"
            start_task = "TapBottom"

            [[goal]]
            name = "farm50"
            [goal.stop]
            LoopCount = { target = 50 }
            "#,
        );

        let defs = load_pipeline(tmp.path()).expect("task defs load");
        assert_eq!(defs.len(), 1, "only TaskDef picked up, manifest skipped");
        assert_eq!(defs[0].name, "TapBottom");

        let manifest = load_pipeline_manifest(tmp.path()).expect("manifest load");
        assert_eq!(manifest.start_task, "TapBottom");
        assert_eq!(manifest.goals.len(), 1);
        assert_eq!(
            manifest.goals[0].stop,
            anaden_core::StopCondition::LoopCount { target: 50 }
        );
    }

    #[test]
    fn fishing_pipeline_taskdefs_load() {
        let dir = workspace_templates_root().join("pipelines").join("fishing");
        let defs = load_pipeline(&dir).expect("fishing pipeline must load");
        assert!(
            defs.iter().any(|d| d.name == "FishingStartPc"),
            "fishing must contain FishingStartPc, got {:?}",
            defs.iter().map(|d| d.name.clone()).collect::<Vec<_>>()
        );
        let start = defs
            .iter()
            .find(|d| d.name == "FishingStartPc")
            .expect("FishingStartPc present");
        assert_eq!(start.algorithm, Algorithm::Ccoeff);
        assert!(start.template.exists(), "template must exist");
        let roi = start.roi.expect("FishingStartPc has ROI");
        assert_roi_within_1258x708(roi, "FishingStartPc");
    }

    #[test]
    fn fishing_pipeline_manifest_loads() {
        let dir = workspace_templates_root().join("pipelines").join("fishing");
        let manifest =
            load_pipeline_manifest(&dir).expect("fishing/pipeline.toml must load as manifest");
        assert_eq!(manifest.start_task, "FishingStartPc");
        let defs = load_pipeline(&dir).expect("TaskDefs must load (manifest skipped)");
        assert!(
            defs.iter().any(|d| d.name == manifest.start_task),
            "manifest.start_task must exist as a TaskDef"
        );
        assert!(
            manifest.goals.iter().all(|g| g.validate().is_ok()),
            "all fishing goals must validate"
        );
    }

    fn sample_task_def() -> TaskDef {
        TaskDef {
            name: "T".into(),
            state: "Field".into(),
            algorithm: Algorithm::Ccoeff,
            template: PathBuf::from("hud_tr.png"),
            roi: None,
            threshold: 0.7,
            base: None,
            action: None,
            next: None,
        }
    }

    #[test]
    fn save_task_def_roundtrips_through_load_pipeline() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let def = TaskDef {
            name: "TitleScreen".into(),
            state: "TitleScreen".into(),
            algorithm: Algorithm::Ccoeff,
            template: PathBuf::from("scenes/title/title_center.png"),
            roi: Some([520, 320, 240, 80]),
            threshold: 0.95,
            base: Some("TitleBase".into()),
            action: Some(Action::ClickRect {
                roi: anaden_core::ScreenRegion::new(625, 273, 30, 30),
            }),
            next: Some(vec!["LoadGame".into()]),
        };
        let path = tmp.path().join("title.toml");
        save_task_def(&def, &path).expect("save must succeed");

        let defs = load_pipeline(tmp.path()).expect("saved TOML must load");
        assert_eq!(defs.len(), 1, "exactly one task");
        let d = &defs[0];
        assert_eq!(d.name, "TitleScreen");
        assert_eq!(d.state, "TitleScreen");
        assert_eq!(d.algorithm, Algorithm::Ccoeff);
        assert_eq!(d.roi, Some([520, 320, 240, 80]));
        assert!((d.threshold - 0.95).abs() < 1e-6);
        assert_eq!(d.base.as_deref(), Some("TitleBase"));
        assert_eq!(
            d.action.as_ref(),
            Some(&Action::ClickRect {
                roi: anaden_core::ScreenRegion::new(625, 273, 30, 30)
            })
        );
        assert_eq!(d.next.as_deref(), Some(&["LoadGame".to_string()][..]));
        assert_eq!(
            d.template,
            tmp.path().join("scenes/title/title_center.png"),
            "template must re-absolutize against the saved TOML's parent"
        );
    }

    #[test]
    fn saved_taskdef_key_set_is_exactly_known_fields() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let def = TaskDef {
            roi: Some([1, 2, 3, 4]),
            base: Some("Base".into()),
            action: Some(Action::ClickSelf),
            next: Some(vec!["N".into()]),
            ..sample_task_def()
        };
        let path = tmp.path().join("t.toml");
        save_task_def(&def, &path).expect("save");
        let content = fs::read_to_string(&path).expect("read");
        let v: toml::Value = toml::from_str(&content).expect("saved TOML is a value");
        let mut keys: Vec<&str> = v
            .as_table()
            .expect("top level must be a table")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        let mut expected = [
            "action",
            "algorithm",
            "base",
            "name",
            "next",
            "roi",
            "state",
            "template",
            "threshold",
        ];
        expected.sort_unstable();
        assert_eq!(keys, expected, "no unknown keys, no missing keys");
    }

    #[test]
    fn save_task_def_relativizes_absolute_template_forward_slash() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let pipelines = tmp.path().join("pipelines");
        let fishing = pipelines.join("fishing");
        let tpl = pipelines.join("field_loop_pc").join("hud_tr.png");
        fs::create_dir_all(&fishing).expect("mkdir fishing");
        fs::create_dir_all(tpl.parent().expect("tpl parent")).expect("mkdir field_loop_pc");
        fs::write(&tpl, b"png").expect("write template file");

        let def = TaskDef {
            name: "FishingStartPc".into(),
            template: tpl,
            roi: Some([1038, 0, 218, 120]),
            action: Some(Action::ClickSelf),
            next: Some(vec!["FishingStartPc".into()]),
            ..sample_task_def()
        };
        let saved_path = fishing.join("fishing_start.toml");
        save_task_def(&def, &saved_path).expect("save");

        let content = fs::read_to_string(&saved_path).expect("read");
        assert!(
            content.contains("template = \"../field_loop_pc/hud_tr.png\""),
            "forward-slash relative form expected:\n{content}"
        );
        let defs = load_pipeline(&fishing).expect("reload");
        assert_eq!(defs.len(), 1);
        assert!(
            defs[0].template.exists(),
            "resolved template must exist: {:?}",
            defs[0].template
        );
    }

    #[test]
    fn save_task_def_template_under_pipeline_dir_saves_relative() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let def = TaskDef {
            template: tmp.path().join("scenes").join("x.png"),
            ..sample_task_def()
        };
        let path = tmp.path().join("task.toml");
        save_task_def(&def, &path).expect("save");
        let content = fs::read_to_string(&path).expect("read");
        assert!(
            content.contains("template = \"scenes/x.png\""),
            "in-dir relative form expected:\n{content}"
        );
    }

    #[test]
    fn relativize_template_without_common_ancestor_keeps_absolute() {
        let got = relativize_template(Path::new("Z:\\a\\b.png"), Path::new("C:\\x\\y"));
        assert_eq!(got, "Z:/a/b.png");
    }

    #[test]
    fn save_task_def_rejects_manifest_convention_filename() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let err = save_task_def(
            &sample_task_def(),
            &tmp.path().join(PIPELINE_MANIFEST_FILENAME),
        )
        .expect_err("manifest filename must be rejected");
        assert!(matches!(err, TaskDefError::SaveFailed { .. }));
    }

    #[test]
    fn save_task_def_missing_parent_dir_errors() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let err = save_task_def(
            &sample_task_def(),
            &tmp.path().join("no-such-dir").join("t.toml"),
        )
        .expect_err("missing parent dir must fail");
        assert!(matches!(err, TaskDefError::SaveFailed { .. }));
    }

    #[test]
    fn save_pipeline_manifest_roundtrips_through_load() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let manifest = PipelineManifest {
            watch: Vec::new(),
            start_task: "Farm".into(),
            goals: vec![
                anaden_core::Goal {
                    name: "loop_50".into(),
                    stop: anaden_core::StopCondition::LoopCount { target: 50 },
                },
                anaden_core::Goal {
                    name: "find_clear".into(),
                    stop: anaden_core::StopCondition::TemplateMatch {
                        task: "clear".into(),
                        confidence: 0.85,
                    },
                },
            ],
        };
        save_pipeline_manifest(&manifest, tmp.path()).expect("save");
        let back = load_pipeline_manifest(tmp.path()).expect("reload");
        assert_eq!(back, manifest);
    }

    #[test]
    fn save_pipeline_manifest_empty_goals_omits_goal_key() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let manifest = PipelineManifest {
            watch: Vec::new(),
            start_task: "TapBottom".into(),
            goals: Vec::new(),
        };
        save_pipeline_manifest(&manifest, tmp.path()).expect("save");
        let content =
            fs::read_to_string(tmp.path().join(PIPELINE_MANIFEST_FILENAME)).expect("read");
        assert!(
            !content.contains("goal"),
            "goal key must be omitted:\n{content}"
        );
    }

    #[test]
    fn save_pipeline_manifest_writes_goal_array_table_rename() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let manifest = PipelineManifest {
            watch: Vec::new(),
            start_task: "TapBottom".into(),
            goals: vec![anaden_core::Goal {
                name: "farm50".into(),
                stop: anaden_core::StopCondition::LoopCount { target: 50 },
            }],
        };
        save_pipeline_manifest(&manifest, tmp.path()).expect("save");
        let content =
            fs::read_to_string(tmp.path().join(PIPELINE_MANIFEST_FILENAME)).expect("read");
        assert!(
            content.contains("[[goal]]"),
            "[[goal]] expected:\n{content}"
        );
        assert!(
            !content.contains("[[goals]]"),
            "[[goals]] must not appear:\n{content}"
        );
    }

    #[test]
    fn manifest_parses_watch_task_names() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_toml(
            tmp.path(),
            "pipeline.toml",
            r#"
            start_task = "TapBottom"
            watch = ["levelup_popup", "close_btn"]
            "#,
        );

        let manifest = load_pipeline_manifest(tmp.path()).expect("manifest must load");
        assert_eq!(
            manifest.watch,
            vec!["levelup_popup".to_string(), "close_btn".to_string()],
            "watch must parse as a string array"
        );
    }

    #[test]
    fn manifest_without_watch_defaults_to_empty() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_toml(
            tmp.path(),
            "pipeline.toml",
            r#"
            start_task = "TapBottom"
            "#,
        );

        let manifest = load_pipeline_manifest(tmp.path()).expect("manifest must load");
        assert!(manifest.watch.is_empty());
    }

    #[test]
    fn save_pipeline_manifest_empty_watch_omits_watch_key() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let manifest = PipelineManifest {
            watch: Vec::new(),
            start_task: "TapBottom".into(),
            goals: Vec::new(),
        };
        save_pipeline_manifest(&manifest, tmp.path()).expect("save");
        let content =
            fs::read_to_string(tmp.path().join(PIPELINE_MANIFEST_FILENAME)).expect("read");
        assert!(
            !content.contains("watch"),
            "watch key must be omitted:\n{content}"
        );
    }

    #[test]
    fn save_pipeline_manifest_watch_roundtrips() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let manifest = PipelineManifest {
            watch: vec!["levelup_popup".into(), "close_btn".into()],
            start_task: "TapBottom".into(),
            goals: Vec::new(),
        };
        save_pipeline_manifest(&manifest, tmp.path()).expect("save");
        let content =
            fs::read_to_string(tmp.path().join(PIPELINE_MANIFEST_FILENAME)).expect("read");
        assert!(
            content.contains("watch = ["),
            "watch must serialize as string array:\n{content}"
        );
        let back = load_pipeline_manifest(tmp.path()).expect("reload");
        assert_eq!(back, manifest);
    }

    #[test]
    fn save_pipeline_manifest_missing_dir_errors() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let err = save_pipeline_manifest(
            &PipelineManifest {
                watch: Vec::new(),
                start_task: "X".into(),
                goals: Vec::new(),
            },
            &tmp.path().join("no-such-dir"),
        )
        .expect_err("missing dir must fail");
        assert!(matches!(err, TaskDefError::SaveFailed { .. }));
    }

    #[test]
    fn saved_manifest_matches_handwritten_value_form_for_existing_pipelines() {
        let pipelines = workspace_templates_root().join("pipelines");
        let mut checked = 0;
        for entry in fs::read_dir(&pipelines).expect("read pipelines dir") {
            let dir = entry.expect("entry").path();
            if !dir.join(PIPELINE_MANIFEST_FILENAME).exists() {
                continue;
            }
            let handwritten =
                fs::read_to_string(dir.join(PIPELINE_MANIFEST_FILENAME)).expect("handwritten");
            let manifest = load_pipeline_manifest(&dir)
                .expect("existing manifest must load (handwritten form is parseable)");
            let out = tempfile::tempdir().expect("tempdir");
            save_pipeline_manifest(&manifest, out.path()).expect("save");
            let saved =
                fs::read_to_string(out.path().join(PIPELINE_MANIFEST_FILENAME)).expect("saved");
            let hw: toml::Value = toml::from_str(&handwritten).expect("handwritten value");
            let sv: toml::Value = toml::from_str(&saved).expect("saved value");
            assert_eq!(
                sv, hw,
                "manifest form mismatch dir={dir:?}\n--handwritten--\n{handwritten}\n--saved--\n{saved}"
            );
            checked += 1;
        }
        assert!(
            checked >= 3,
            "expected >= 3 existing manifests (field_loop_pc/fishing/login), checked {checked}"
        );
    }

    #[test]
    fn saved_taskdefs_match_handwritten_value_form_for_existing_pipelines() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let copy_root = tmp.path().join("templates");
        copy_toml_files_only(&workspace_templates_root(), &copy_root);
        let pipelines = copy_root.join("pipelines");
        let mut checked = 0;
        for entry in fs::read_dir(&pipelines).expect("read pipelines dir") {
            let dir = entry.expect("entry").path();
            let defs = load_pipeline(&dir).expect("taskdefs load");
            for f in fs::read_dir(&dir).expect("read pipeline dir") {
                let p = f.expect("entry").path();
                let is_toml = p
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| e.eq_ignore_ascii_case("toml"));
                if !is_toml || is_pipeline_manifest_path(&p) {
                    continue;
                }
                let handwritten = fs::read_to_string(&p).expect("handwritten");
                let hw: toml::Value = toml::from_str(&handwritten).expect("handwritten value");
                let name = hw
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or_else(|| panic!("TaskDef file {p:?} has no name"))
                    .to_string();
                let def = defs
                    .iter()
                    .find(|d| d.name == name)
                    .unwrap_or_else(|| panic!("TaskDef {name} not loaded from {dir:?}"))
                    .clone();
                save_task_def(&def, &p).expect("save in place");
                let saved = fs::read_to_string(&p).expect("saved");
                let sv: toml::Value = toml::from_str(&saved).expect("saved value");
                assert_eq!(
                    sv, hw,
                    "taskdef form mismatch file={p:?}\n--handwritten--\n{handwritten}\n--saved--\n{saved}"
                );
                checked += 1;
            }
        }
        assert!(
            checked >= 8,
            "expected all 8 handwritten TaskDefs, checked {checked}"
        );
    }

    fn copy_toml_files_only(src: &Path, dst: &Path) {
        fs::create_dir_all(dst).expect("mkdir dst");
        for entry in fs::read_dir(src).expect("read src") {
            let p = entry.expect("entry").path();
            if p.is_dir() {
                copy_toml_files_only(&p, &dst.join(p.file_name().expect("dir name")));
            } else {
                let is_toml = p
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| e.eq_ignore_ascii_case("toml"));
                if is_toml {
                    fs::copy(&p, dst.join(p.file_name().expect("file name"))).expect("copy toml");
                }
            }
        }
    }

    #[test]
    fn cache_serves_vanished_file_and_reloads_when_cached_mtime_unknown() {
        let missing = Path::new("z-anaden-missing-template-probe.png");
        assert!(
            mtime_unchanged(missing, None),
            "vanished file must keep serving the cached template (pinned by detect_template_loaded_once_for_repeated_detects)"
        );
        assert!(
            mtime_unchanged(missing, Some(SystemTime::UNIX_EPOCH)),
            "vanished file stays served regardless of the cached mtime"
        );
        let dir = tempfile::tempdir().unwrap();
        let existing = dir.path().join("probe.png");
        fs::write(&existing, b"probe").unwrap();
        let mtime = file_mtime(&existing).expect("mtime of written probe file");
        assert!(mtime_unchanged(&existing, Some(mtime)));
        assert!(!mtime_unchanged(&existing, Some(SystemTime::UNIX_EPOCH)));
        assert!(
            !mtime_unchanged(&existing, None),
            "existing file with unknown cached mtime must reload to recover verifiability"
        );
    }
}
