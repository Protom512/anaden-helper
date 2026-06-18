//! テンプレートライブラリの保存/読込（純関数・テスト対象）。
//!
//! 各テンプレートは PNG 画像と sidecar TOML メタデータの対で保存される:
//!   `<base_dir>/<state>/<name>.png`   ← テンプレート画像（既存 TemplateStore 互換）
//!   `<base_dir>/<state>/<name>.toml`  ← メタデータ（ROI/閾値/方式/状態）
//!
//! PNG の配置は既存 `TemplateStore::load_from_directory` と互換（ディレクトリ名=状態）。
//! TOML 形式は Wiki [[Declarative-Tasks-Design]] に準拠する。

use std::path::{Path, PathBuf};

use image::DynamicImage;
use serde::{Deserialize, Serialize};

use anaden_core::ScreenRegion;

/// テンプレート1件のメタデータ仕様。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TemplateSpec {
    /// テンプレート識別名（ファイル名の stem）。
    pub name: String,
    /// 状態キー（ディレクトリ名兼 GameState ラベル。例: "title", "battle"）。
    pub state: String,
    /// ROI。720p 基準座標を想定（M6 で統合）。
    pub roi: ScreenRegion,
    /// マッチ判定閾値。
    pub threshold: f32,
    /// 認識方式（"sse"。将来 "ccoeff"）。
    pub method: String,
}

/// テンプレートを PNG + sidecar TOML として保存する。
pub fn save_template(
    base_dir: &Path,
    spec: &TemplateSpec,
    image: &DynamicImage,
) -> std::io::Result<PathBuf> {
    let dir = base_dir.join(&spec.state);
    std::fs::create_dir_all(&dir)?;
    let png_path = dir.join(format!("{}.png", spec.name));
    image
        .save(&png_path)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    let toml_path = dir.join(format!("{}.toml", spec.name));
    let toml_str =
        toml::to_string(spec).map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    std::fs::write(&toml_path, toml_str)?;
    Ok(png_path)
}

/// ベースディレクトリ下の全 sidecar TOML を読み込み、仕様一覧を返す。
/// PNG の存在は確認しない（TOML のみ基準）。
#[allow(dead_code)] // M4 バッチ混同行列で使用
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
            if p.extension().and_then(|e| e.to_str()) == Some("toml") {
                if let Ok(content) = std::fs::read_to_string(&p) {
                    if let Ok(spec) = toml::from_str::<TemplateSpec>(&content) {
                        out.push(spec);
                    }
                }
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
        // f32 0.95 は TOML 上で 0.9499999... と展開されるため、行の存在のみ検証
        assert!(content.contains("threshold ="));
    }
}
