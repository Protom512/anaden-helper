//! パイプライン TOML のワイヤ表現。
//!
//! ここに置く型は「ディスクに書かれているもの」だけを表す。実行時の意味
//! （テンプレートの解決、アクションの既定値）は [`super::task::Task`] が持つ。
//!
//! [`TaskDef`] は `Deserialize` のみを実装し、`Serialize` は持たない。
//! 書き込みは [`super::io::save_task_def`] の 1 経路だけにしてあり、
//! 相対化を忘れた第 2 の書き手はコンパイルできない。

use std::path::PathBuf;

use anaden_core::{MatchConfidence, ScreenRegion};
use serde::{Deserialize, Serialize};

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
pub(crate) struct ClickRectDef {
    roi: ScreenRegion,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SwipeDef {
    from: ScreenRegion,
    to: ScreenRegion,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BareActionDef {}

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

/// タスク 1 行の入力側。ディスクから読むときだけ使う。
///
/// `state` は全 TOML が必須キーとして持つが、実行時には読まれない。
/// `base` は任意キーで、現状の生産コードでは読まれない。どちらも
/// ラウンドトリップのために保持する。
#[derive(Debug, Clone, PartialEq, Deserialize)]
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

/// タスク 1 行の出力側。フィールド順は [`TaskDef`] と一致させる。
///
/// 書き込み専用の型を分けることで、`TaskDef` を直接 serialize する
/// 第 2 の書き手（相対化も float 正規化もせずに書く経路）を型で禁止する。
#[derive(Debug, Clone, Serialize)]
pub(crate) struct TaskRow {
    name: String,
    state: String,
    algorithm: Algorithm,
    template: PathBuf,
    roi: Option<[u32; 4]>,
    threshold: f32,
    base: Option<String>,
    action: Option<Action>,
    next: Option<Vec<String>>,
}

impl TaskRow {
    pub(crate) fn from_def(def: &TaskDef, template: PathBuf) -> Self {
        Self {
            name: def.name.clone(),
            state: def.state.clone(),
            algorithm: def.algorithm,
            template,
            roi: def.roi,
            threshold: def.threshold,
            base: def.base.clone(),
            action: def.action.clone(),
            next: def.next.clone(),
        }
    }
}

pub(crate) fn default_threshold() -> f32 {
    MatchConfidence::DEFAULT_THRESHOLD.0
}

#[derive(Debug, thiserror::Error)]
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

pub const PIPELINE_MANIFEST_FILENAME: &str = "pipeline.toml";

pub(crate) fn is_pipeline_manifest_path(path: &std::path::Path) -> bool {
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
