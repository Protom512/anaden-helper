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
    image.save(&png_path).map_err(std::io::Error::other)?;
    let toml_path = dir.join(format!("{}.toml", spec.name));
    let toml_str = toml::to_string(spec).map_err(std::io::Error::other)?;
    std::fs::write(&toml_path, toml_str)?;
    Ok(png_path)
}

/// テンプレート画像が無構造 (ほぼ単色) の場合に GUI status 表示用の警告文を返す
/// (Issue #184・fail-visible)。
///
/// stddev 計算・閾値判定は anaden-vision の単一実装
/// ([`anaden_vision::template_is_structured`]) に委譲する — GUI 側での再実装は
/// しない (Issue #184 受入基準「stddev 計算ロジックが単一実装」)。
///
/// - `Some(warning)`: stddev が [`anaden_vision::TEMPLATE_MIN_LUMA_STDDEV`] 未満。
///   警告文は「認識不能 (恒久 NoMatch) の恐れ」を明示する
///   (Issue #182: 無構造テンプレ実測 stddev 3.76 → 実機 65 iters 発火 0)。
/// - `None`: 構造あり (警告なし = 偽陽性ゼロ)。
///
/// 呼出側 (テンプレート保存・pipeline task 保存・シナリオ保存) はこの警告を
/// status へ表示するのみで、**保存自体はブロックしない** (ユーザーが意図的に
/// 単色テンプレートを保存するケースを拒否しない方針)。
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

/// needle (テンプレート PNG) が TaskDef の ROI に収まらない場合の警告文を返す
/// (Issue #187・fail-visible)。
///
/// 収容判定は anaden-vision の単一実装 ([`anaden_vision::needle_fits_roi`]) に委譲する
/// (Issue #184 の stddev 検証と同じ「GUI 側で再実装しない」原則)。
///
/// - `Some(warning)`: needle の幅/高さが roi の幅/高さを超えている。ROI cropping 後の
///   haystack に needle が置けず恒久 NoMatch になる (Issue #182: 再生成テンプレ 138px 幅
///   vs 旧 roi 幅 121px で発火しなかった実例)。
/// - `None`: 収まる、または roi = 全面 (省略)。
///
/// 呼出側 (シナリオ保存) はこの警告を status へ表示するのみで、**保存自体は
/// ブロックしない** (テンプレート差し替えと ROI 更新は対で行うべきだが、警告で
/// 促すに留める — [`template_structure_warning`] と同じ方針)。
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
        // f32 0.95 は TOML 上で 0.9499999... と展開されるため、行の存在のみ検証
        assert!(content.contains("threshold ="));
    }
}
