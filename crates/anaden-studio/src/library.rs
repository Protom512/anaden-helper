use std::path::{Path, PathBuf};

use image::DynamicImage;
use serde::{Deserialize, Serialize};

use anaden_core::ScreenRegion;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TemplateSpec {
    pub name: String,
    pub state: String,
    pub roi: ScreenRegion,
    pub threshold: f32,
    pub method: String,
}

pub fn save_template(
    base_dir: &Path,
    spec: &TemplateSpec,
    image: &DynamicImage,
) -> std::io::Result<PathBuf> {
    let dir = base_dir.join(&spec.state);
    std::fs::create_dir_all(&dir)?;
    let png_path = dir.join(format!("{}.png", spec.name));
    image.save(&png_path).map_err(std::io::Error::other)?;
    let toml_path = dir.join(format!("{}.toml", spec.name));
    let toml_str = toml::to_string(spec).map_err(std::io::Error::other)?;
    std::fs::write(&toml_path, toml_str)?;
    Ok(png_path)
}

pub(crate) fn template_structure_warning(image: &DynamicImage) -> Option<String> {
    if anaden_vision::template_is_structured(image) {
        return None;
    }
    let stddev = anaden_vision::template_luma_stddev(image);
    Some(format!(
        "警告: テンプレート画像がほぼ単色です (輝度 stddev {stddev:.1} < 閾値 {:.1}) — \
         テンプレートマッチで認識不能 (恒久 NoMatch) の恐れがあります",
        anaden_vision::TEMPLATE_MIN_LUMA_STDDEV
    ))
}

pub(crate) fn needle_roi_warning(needle: (u32, u32), roi: Option<[u32; 4]>) -> Option<String> {
    if anaden_vision::needle_fits_roi(needle, roi) {
        return None;
    }
    let [_, _, rw, rh] = roi?;
    let (nw, nh) = needle;
    Some(format!(
        "警告: テンプレート ({nw}x{nh}) が ROI ({rw}x{rh}) より大きい — ROI 内に needle が\
         収まらず恒久 NoMatch になります (テンプレート差し替え時は ROI を対で更新してください)"
    ))
}

#[allow(dead_code)]
pub fn load_library(base_dir: &Path) -> Vec<TemplateSpec> {
    let mut out = Vec::new();
    let Ok(state_dirs) = std::fs::read_dir(base_dir) else {
        return out;
    };
    for state_dir in state_dirs.flatten() {
        if !state_dir.path().is_dir() {
            continue;
        }
        let Ok(files) = std::fs::read_dir(state_dir.path()) else {
            continue;
        };
        for f in files.flatten() {
            let p = f.path();
            if p.extension().and_then(|e| e.to_str()) == Some("toml")
                && let Ok(content) = std::fs::read_to_string(&p)
                && let Ok(spec) = toml::from_str::<TemplateSpec>(&content)
            {
                out.push(spec);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn spec(name: &str, state: &str) -> TemplateSpec {
        TemplateSpec {
            name: name.to_string(),
            state: state.to_string(),
            roi: ScreenRegion::new(10, 20, 100, 50),
            threshold: 0.95,
            method: "sse".to_string(),
        }
    }

    #[test]
    fn save_then_load_roundtrip() {
        let dir = tempdir().unwrap();
        let img = DynamicImage::ImageLuma8(image::GrayImage::new(100, 50));
        let s = spec("logo", "title");
        let png = save_template(dir.path(), &s, &img).unwrap();
        assert!(png.exists());

        let loaded = load_library(dir.path());
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0], s);
    }

    #[test]
    fn load_empty_dir_returns_empty() {
        let dir = tempdir().unwrap();
        assert!(load_library(dir.path()).is_empty());
    }

    #[test]
    fn multiple_states_separate_dirs() {
        let dir = tempdir().unwrap();
        let img = DynamicImage::ImageLuma8(image::GrayImage::new(10, 10));
        save_template(dir.path(), &spec("a", "title"), &img).unwrap();
        save_template(dir.path(), &spec("b", "battle"), &img).unwrap();

        let loaded = load_library(dir.path());
        assert_eq!(loaded.len(), 2);
    }

    #[test]
    fn sidecar_toml_is_readable_text() {
        let dir = tempdir().unwrap();
        let img = DynamicImage::ImageLuma8(image::GrayImage::new(100, 50));
        save_template(dir.path(), &spec("logo", "title"), &img).unwrap();

        let toml_path = dir.path().join("title").join("logo.toml");
        let content = std::fs::read_to_string(&toml_path).unwrap();
        assert!(content.contains("name = \"logo\""));
        assert!(content.contains("state = \"title\""));
        assert!(content.contains("threshold ="));
    }
}
