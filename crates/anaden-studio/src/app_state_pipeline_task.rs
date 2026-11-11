use std::path::{Path, PathBuf};

use image::DynamicImage;

use anaden_core::ScreenRegion;
use anaden_vision::{Action, Algorithm};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelineActionKind {
    ClickSelf,
    DoNothing,
    Stop,
}

impl PipelineActionKind {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::ClickSelf => "click_self (マッチ位置をタップ)",
            Self::DoNothing => "do_nothing (何もしない)",
            Self::Stop => "stop (停止)",
        }
    }

    #[must_use]
    pub fn from_label(label: &str) -> Option<Self> {
        match label {
            l if l == Self::ClickSelf.label() => Some(Self::ClickSelf),
            l if l == Self::DoNothing.label() => Some(Self::DoNothing),
            l if l == Self::Stop.label() => Some(Self::Stop),
            _ => None,
        }
    }

    fn to_action(self) -> Action {
        match self {
            Self::ClickSelf => Action::ClickSelf,
            Self::DoNothing => Action::DoNothing,
            Self::Stop => Action::Stop,
        }
    }
}

#[must_use]
pub fn pipeline_task_spec(
    name: &str,
    state: &str,
    method: &str,
    roi: ScreenRegion,
    threshold: f32,
    action: PipelineActionKind,
) -> Option<anaden_vision::TaskDef> {
    let algorithm = match method {
        "sse" => Algorithm::Sse,
        "ccoeff" => Algorithm::Ccoeff,
        _ => return None,
    };
    Some(anaden_vision::TaskDef {
        name: name.to_string(),
        state: state.to_string(),
        algorithm,
        template: PathBuf::from(format!("{name}.png")),
        roi: Some([roi.x, roi.y, roi.width, roi.height]),
        threshold,
        base: None,
        action: Some(action.to_action()),
        next: Some(vec![]),
    })
}

pub fn save_pipeline_task(
    dir: &Path,
    spec: &anaden_vision::TaskDef,
    template: &DynamicImage,
) -> std::io::Result<PathBuf> {
    if let Some(issue) = crate::library::save_name_issue(&spec.name) {
        return Err(std::io::Error::other(format!(
            "pipeline task name invalid ({issue}): {:?}",
            spec.name
        )));
    }
    let png_path = dir.join(format!("{}.png", spec.name));
    let toml_path = dir.join(format!("{}.toml", spec.name));
    for target in [&png_path, &toml_path] {
        if target.exists() {
            return Err(std::io::Error::other(format!(
                "pipeline task file collision: {} (既存タスクの無警告上書きは禁止。ロードして編集するか別名を指定してください)",
                target.display()
            )));
        }
    }
    std::fs::create_dir_all(dir)?;
    template.save(&png_path).map_err(std::io::Error::other)?;
    anaden_vision::save_task_def(spec, &toml_path).map_err(std::io::Error::other)?;
    Ok(toml_path)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use image::Luma;

    #[test]
    fn pipeline_task_spec_builds_from_method_str() {
        let roi = ScreenRegion::new(10, 20, 30, 40);
        let spec = pipeline_task_spec(
            "my_task",
            "field",
            "ccoeff",
            roi,
            0.85,
            PipelineActionKind::ClickSelf,
        )
        .unwrap();
        assert_eq!(spec.name, "my_task");
        assert_eq!(spec.state, "field");
        assert_eq!(spec.algorithm, anaden_vision::Algorithm::Ccoeff);
        assert_eq!(spec.roi, Some([10, 20, 30, 40]));
        assert_eq!(spec.threshold, 0.85);
        assert_eq!(spec.action, Some(anaden_vision::Action::ClickSelf));
        assert_eq!(spec.next, Some(vec![]));

        let sse = pipeline_task_spec(
            "t2",
            "title",
            "sse",
            roi,
            0.9,
            PipelineActionKind::DoNothing,
        )
        .unwrap();
        assert_eq!(sse.algorithm, anaden_vision::Algorithm::Sse);
        assert_eq!(sse.action, Some(anaden_vision::Action::DoNothing));
    }

    #[test]
    fn pipeline_task_spec_rejects_unknown_method() {
        assert!(
            pipeline_task_spec(
                "x",
                "field",
                "orb",
                ScreenRegion::new(0, 0, 1, 1),
                0.9,
                PipelineActionKind::ClickSelf
            )
            .is_none()
        );
    }

    #[test]
    fn save_pipeline_task_roundtrips_through_load_pipeline() {
        let dir = tempfile::tempdir().unwrap();
        let spec = pipeline_task_spec(
            "tap_logo",
            "title",
            "ccoeff",
            ScreenRegion::new(10, 20, 100, 50),
            0.82,
            PipelineActionKind::ClickSelf,
        )
        .unwrap();
        let img = DynamicImage::ImageLuma8(image::GrayImage::from_pixel(100, 50, Luma([128])));
        let toml_path = save_pipeline_task(dir.path(), &spec, &img).unwrap();
        assert!(toml_path.exists());
        assert!(dir.path().join("tap_logo.png").exists());

        let tasks = anaden_vision::load_pipeline(dir.path()).unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].name, "tap_logo");
        assert_eq!(tasks[0].state, "title");
        assert_eq!(tasks[0].algorithm, anaden_vision::Algorithm::Ccoeff);
        assert_eq!(tasks[0].roi, Some([10, 20, 100, 50]));
        assert_eq!(tasks[0].action, Some(anaden_vision::Action::ClickSelf));
    }

    #[test]
    fn pipeline_action_kind_labels_roundtrip() {
        for k in [
            PipelineActionKind::ClickSelf,
            PipelineActionKind::DoNothing,
            PipelineActionKind::Stop,
        ] {
            assert_eq!(PipelineActionKind::from_label(k.label()), Some(k));
        }
        assert_eq!(PipelineActionKind::from_label("bogus"), None);
    }

    #[test]
    fn save_pipeline_task_refuses_overwrite_and_preserves_existing_toml() {
        let dir = tempfile::tempdir().unwrap();
        let spec = pipeline_task_spec(
            "TapLogo",
            "title",
            "ccoeff",
            ScreenRegion::new(10, 20, 100, 50),
            0.82,
            PipelineActionKind::ClickSelf,
        )
        .unwrap();
        let img = DynamicImage::ImageLuma8(image::GrayImage::from_pixel(100, 50, Luma([128])));
        save_pipeline_task(dir.path(), &spec, &img).unwrap();
        let toml_path = dir.path().join("TapLogo.toml");
        let before = std::fs::read(&toml_path).unwrap();

        let err = save_pipeline_task(dir.path(), &spec, &img).unwrap_err();
        assert!(err.to_string().contains("collision"), "err: {err}");
        assert_eq!(std::fs::read(&toml_path).unwrap(), before);
    }

    #[test]
    fn save_pipeline_task_rejects_path_escape_name() {
        let dir = tempfile::tempdir().unwrap();
        let spec = pipeline_task_spec(
            "../escape",
            "title",
            "ccoeff",
            ScreenRegion::new(10, 20, 100, 50),
            0.82,
            PipelineActionKind::ClickSelf,
        )
        .unwrap();
        let img = DynamicImage::ImageLuma8(image::GrayImage::from_pixel(100, 50, Luma([128])));
        let err = save_pipeline_task(dir.path(), &spec, &img).unwrap_err();
        assert!(err.to_string().contains("invalid"), "err: {err}");
        assert!(!dir.path().join("escape.png").exists());
    }
}
