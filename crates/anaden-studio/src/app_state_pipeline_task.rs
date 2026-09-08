//! pipeline task の構築・保存 (Issue #175: app_state.rs 分割)。
//!
//! 作成タブ (Authoring) の入力 (ROI/スコア/アクション) から pipeline task
//! (anaden_vision::TaskDef) を構築する ([`PipelineActionKind`] /
//! [`pipeline_task_spec`]) と、TOML + テンプレート PNG としてディレクトリへ
//! 保存する ([`save_pipeline_task`]) を定義する。StudioApp 本体は
//! [`crate::app_state_core`]、呼び出し元互換の re-export は
//! [`crate::app_state`] (facade)。

use std::path::{Path, PathBuf};

use image::DynamicImage;

use anaden_core::ScreenRegion;
use anaden_vision::{Action, Algorithm};

/// pipeline task の認識成功時アクション種別 (UI コンボ選択用)。
/// anaden_vision::Action の作成タブで扱う部分集合。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelineActionKind {
    /// マッチ位置をクリック (`click_self`)。
    ClickSelf,
    /// 何もしない (`do_nothing`)。
    DoNothing,
    /// 停止 (`stop`)。
    Stop,
}

impl PipelineActionKind {
    /// UI コンボ表示ラベル (グリフ確認済み・豆腐なし)。
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::ClickSelf => "click_self (マッチ位置をタップ)",
            Self::DoNothing => "do_nothing (何もしない)",
            Self::Stop => "stop (停止)",
        }
    }

    /// ラベル → 種別。UI の選択状態復元用。未知ラベルは None (fail-closed)。
    #[must_use]
    pub fn from_label(label: &str) -> Option<Self> {
        match label {
            l if l == Self::ClickSelf.label() => Some(Self::ClickSelf),
            l if l == Self::DoNothing.label() => Some(Self::DoNothing),
            l if l == Self::Stop.label() => Some(Self::Stop),
            _ => None,
        }
    }

    /// anaden_vision::Action へ変換。
    fn to_action(self) -> Action {
        match self {
            Self::ClickSelf => Action::ClickSelf,
            Self::DoNothing => Action::DoNothing,
            Self::Stop => Action::Stop,
        }
    }
}

/// 作成タブの入力 (ROI/スコア) から pipeline task (anaden_vision::TaskDef) を構築する。
///
/// `method` は engine_kind.method_str ("sse"/"ccoeff") を想定。未知文字列は
/// None (fail-closed。黙って既定方式へフォールバックしない)。
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

/// pipeline task を TOML + テンプレート PNG としてディレクトリへ保存する。
///
/// 出力: `<dir>/<name>.toml` + `<dir>/<name>.png`。既存 `load_pipeline`
/// (anaden-vision) でそのまま読み込める形式 (`templates/pipelines/<pipeline>/` 互換)。
pub fn save_pipeline_task(
    dir: &Path,
    spec: &anaden_vision::TaskDef,
    template: &DynamicImage,
) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let png_path = dir.join(format!("{}.png", spec.name));
    template.save(&png_path).map_err(std::io::Error::other)?;
    let toml_path = dir.join(format!("{}.toml", spec.name));
    let toml_str = toml::to_string(spec).map_err(std::io::Error::other)?;
    std::fs::write(&toml_path, toml_str)?;
    Ok(toml_path)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use image::Luma;

    // ---- Issue #139 T5: UC-3 作成タブ → pipeline task TOML 保存 ----

    /// pipeline_task_spec は有効な方式文字列から TaskDef を構築する。
    /// engine_kind.method_str ("sse"/"ccoeff") がそのまま使える。
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

    /// 未知の方式文字列は None (fail-closed。黙って sse にフォールバックしない)。
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

    /// save_pipeline_task が書いた TOML は既存 load_pipeline で読み込める (roundtrip)。
    /// 作成タブで保存した task が実行パイプライン (anaden-cli) からそのまま
    /// 使えることの結合保証。
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

    /// action 種別の label ラウンドトリップ (UI コンボ用)。
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
}
