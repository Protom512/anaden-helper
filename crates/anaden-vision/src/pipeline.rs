//! 宣言的TOMLパイプライン。1つのTOMLファイル = 1つの認識タスク。
//!
//! TASK-008 の最初の縦スライス。テンプレート画像・ROI・閾値・アルゴリズム・状態キーを
//! TOMLで宣言し、コード変更なしで認識タスクを定義・実行できる。
//! Wiki [[Declarative-Tasks-Design]] の具装化。action/next/base は前方互換の Option で保持する。
//!
//! 本スライスの範囲: TOML1ファイル = 平坦な [`TaskDef`]。1ファイル複数タスク（サブテーブル）や
//! action 実行・next 状態機械・base 継承解決は将来スライス。

use std::path::{Path, PathBuf};

use image::DynamicImage;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::debug;

use anaden_core::{MatchConfidence, ScreenRegion};

use crate::ccoeff::CcoeffVisionEngine;
use crate::engine::{SseVisionEngine, VisionEngine};
use crate::matcher::{MatchResult, TemplateMatcher};

/// 認識アルゴリズム。TOML の `algorithm` 文字列（`sse`/`ccoeff`）から解決する。
///
/// Wiki 設計の `AlgorithmType`（match_template/ocr/feature/just_return）のうち、
/// 本スライスは既存2エンジンで実現可能な `sse`/`ccoeff` のみ。未知値はロード時エラー。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Algorithm {
    /// imageproc 正規化SSE（[`crate::engine::SseVisionEngine`]）。
    Sse,
    /// OpenCV 互換 TM_CCOEFF_NORMED（[`crate::ccoeff::CcoeffVisionEngine`]）。
    Ccoeff,
}

/// MAA `ProcessTaskAction`（AsstTypes.h:492-518）準拠の認識成功時アクション。
///
/// 本スライスでは「指示を値として表現」するまで。実際の tap/swipe 発火は範囲外
/// （[`crate`] 外のデバイス実行層で [`anaden_core`] の入力型へ変換して発火させる）。
///
/// TOML ではテーブル `[action]`（`type` キー必須）で記述する。MAA の `get_action_type` と同様、
/// 未知の `type` 文字列はロード時（[`load_pipeline`] の `toml::from_str`）エラーになる。
///
/// ```toml
/// [action]
/// type = "click_self"
/// ```
///
/// 引数なしバリアント（`click_self`/`do_nothing`/`stop`）は `type` だけのテーブル。
/// 引数付きバリアントは `roi`/`from`/`to`（[`ScreenRegion`] = `[x,y,width,height]` 整数配列）を持つ。
///
/// # `deny_unknown_fields` の実装上の注意
/// serde の内部タグ付き enum（`#[serde(tag = "type")]`）では `deny_unknown_fields` が
/// **未知フィールドを黙許する** 既知の制限がある（serde は `Content` バッファに取り込むため
/// フィールド名を検証できない）。そのため `Action` の [`Deserialize`] は手動実装とし、
/// `type` を取り出した残りテーブルを各バリアント構造体（`deny_unknown_fields` 付き）で
/// 厳密に検証する。これにより未知 `type`・必須フィールド欠落・未知フィールド過剰の
/// いずれもロード時 [`TaskDefError::ParseFailed`] となる。
///
/// 注: anaden-core の `InputAction` はデバイス実行用の別型（Tap/Swipe/LongPress 等）で、
/// MAA 宣言的 Action とは責務が違うため本 enum は pipeline 内に新設する。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum Action {
    /// マッチしたテンプレート位置（[`MatchResult::region`]）をクリック。
    ClickSelf,
    /// 静的に指定した矩形をクリック（MAA `specificRect` 相当）。
    ClickRect {
        /// クリック対象矩形。720p（幅1280）基準 `[x, y, width, height]`。
        roi: ScreenRegion,
    },
    /// 指定2点間をスワイプ（`special_params[0]`=duration は将来スライス）。
    Swipe {
        /// スワイプ開始点。720p 基準 `[x, y, width, height]`。
        from: ScreenRegion,
        /// スワイプ終点。720p 基準 `[x, y, width, height]`。
        to: ScreenRegion,
    },
    /// 何もしない（MAA では action 省略/空文字列と同等）。
    DoNothing,
    /// 現在のタスクフローを停止（将来: `NodeStatus::Interrupted` に伝播）。
    Stop,
}

// ---- Action の Deserialize 手動実装（deny_unknown_fields を確実に効かせる）----
//
// `type` キーでバリアントを分岐し、残りのキーを各バリアント構造体で検証する。
// 各構造体に `deny_unknown_fields` を付け、未知フィールドを拒否する。

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

/// 引数なしバリアント用。`deny_unknown_fields` で `type` 以外の過剰キーを拒否する。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BareActionDef {}

impl<'de> Deserialize<'de> for Action {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error;

        // 一旦 toml::Value 相当の汎用 map として受ける（`type` を先読みするため）。
        let raw = toml::Value::deserialize(deserializer)?;
        let table = raw
            .as_table()
            .ok_or_else(|| D::Error::custom("action must be a table with a `type` field"))?;

        let type_str = table
            .get("type")
            .and_then(|v| v.as_str())
            .ok_or_else(|| D::Error::custom("action requires a string `type` field"))?;

        // `type` 以外のキーだけを持つ部分テーブルを構築し、各バリアント構造体で検証。
        // （`type` を取り除かないと deny_unknown_fields が `type` を未知フィールドとして弾く）
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

/// 1つの宣言的認識タスク。
///
/// 1ファイル1タスク構成（[`load_pipeline`] はディレクトリ内 `*.toml` を各1タスクとして集約）。
/// 将来の action/next/base 継承拡張に備え、これらは `Option` で前方互換を持たせる。
/// 本スライスの [`TaskDef::detect`] は action/next/base を **使わない**（パースして保持するだけ）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskDef {
    /// タスク名（= 識別子）。
    pub name: String,
    /// 紐付ける状態キー（文字列）。GameState enum 直結は本スライス外。後段マッパが解決する。
    pub state: String,
    /// 認識アルゴリズム。
    pub algorithm: Algorithm,
    /// テンプレート画像パス。相対パスは TOML ファイルの親ディレクトリ基準
    /// （[`load_pipeline`] で絶対パスに解決される）。
    pub template: PathBuf,
    /// 720p（幅1280）基準ROI `[x, y, w, h]`。`None` なら全面。
    #[serde(default)]
    pub roi: Option<[u32; 4]>,
    /// マッチ判定閾値 0.0..=1.0。省略時は [`MatchConfidence::DEFAULT_THRESHOLD`]（0.95）。
    #[serde(default = "default_threshold")]
    pub threshold: f32,

    /// MAA baseTask 相当の継承。将来の resolve で使用。本スライスでは無視。
    #[serde(default)]
    pub base: Option<String>,
    /// 認識成功時アクション（[`Action`]）。省略時は [`Action::DoNothing`] 扱い
    /// （[`run_step`] で補充）。未知 `type` や必須フィールド欠落はロード時エラー。
    #[serde(default)]
    pub action: Option<Action>,
    /// next 遷移リスト。将来の状態機械で使用。本スライスでは無視。
    #[serde(default)]
    pub next: Option<Vec<String>>,
}

fn default_threshold() -> f32 {
    MatchConfidence::DEFAULT_THRESHOLD.0
}

impl TaskDef {
    /// テンプレート画像を読み込み、algorithm + roi + threshold で認識を実行する。
    ///
    /// 戻り値の [`MatchResult::region`] は **スクリーンショット元解像度の座標**
    /// （[`VisionEngine`] 仕様）。roi 指定時は cropping 後の局所座標 → 元座標へオフセット戻しする。
    ///
    /// # 720p基準ROIの前提
    /// `roi` は 720p（幅1280）座標系。呼出側は事前に [`crate::scale::ScreenScaler`] 等で
    /// screenshot を幅1280へ正規化済みであることを想定する。正規化された画面に対して
    /// 720p座標の ROI を直接画素座標として crop する（MAA ControlScaleProxy 準拠）。
    ///
    /// `template_root` は、`template` が相対パスの場合の解決基準（通常は TOML の親ディレクトリ）。
    /// [`load_pipeline`] で既に絶対化済みならこの引数は使われない。
    pub fn detect(
        &self,
        screenshot: &DynamicImage,
        template_root: &Path,
    ) -> Result<Option<MatchResult>, TaskDefError> {
        // 1. テンプレート読込
        let tpl_path = if self.template.is_absolute() {
            self.template.clone()
        } else {
            template_root.join(&self.template)
        };
        let needle = image::open(&tpl_path).map_err(|e| TaskDefError::TemplateLoadFailed {
            path: tpl_path.clone(),
            reason: e.to_string(),
        })?;

        // 2. ROI cropping（720p座標 → normalize 済み画面の画素座標として直接 crop）
        let work = match self.roi_to_region() {
            Some(r) => crop_imm(screenshot, r),
            None => screenshot.clone(),
        };

        // 3. algorithm 切替（既存エンジン再利用）
        let thr = MatchConfidence::new(self.threshold);
        let result = match self.algorithm {
            // SseVisionEngine::with_defaults() は閾値0.85固定のため、TaskDef.threshold を
            // 反映するには threshold_only で構築する。
            Algorithm::Sse => {
                let m = TemplateMatcher::threshold_only(thr);
                SseVisionEngine::new(m).match_template(&work, &needle)
            }
            // CCOEFF も downscale_factor=1（threshold_only 相当）で構築する。
            // 設計案では downscale=2 を想定していたが、Triangle 1/2縮小は周期パターンで
            // サブピクセル位相依存の相関低下を生じ、needle 埋込位置が奇数座標だと閾値未満に
            // なる（位置非依存の「埋め込めば必ず一致」性質が壊れる）。宣言的タスクでは
            // ユーザが「テンプレを埋めれば確実にマッチする」ことを期待するため、正確性を
            // 優先して downscale=1 とする。全面走査の性能は将来スライスで ROI/解像度前提を
            // 併用して最適化する。
            Algorithm::Ccoeff => {
                CcoeffVisionEngine::threshold_only(thr).match_template(&work, &needle)
            }
        };

        // 4. ROI オフセット戻し（cropping した場合、局所→元座標へ）
        Ok(result.map(|mut mr| {
            if let Some(r) = self.roi_to_region() {
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

    /// roi 配列 → [`ScreenRegion`]。`w`/`h` が 0 なら `None`（=全面扱い）。
    fn roi_to_region(&self) -> Option<ScreenRegion> {
        let [x, y, w, h] = self.roi?;
        if w == 0 || h == 0 {
            return None;
        }
        Some(ScreenRegion::new(x, y, w, h))
    }
}

/// パイプライン読込・テンプレート読込のエラー。
#[derive(Debug, Error)]
pub enum TaskDefError {
    /// テンプレート画像の読込失敗。
    #[error("template load failed {path}: {reason}")]
    TemplateLoadFailed { path: PathBuf, reason: String },

    /// TOML 解析失敗（構文エラー・未知キー・未知 algorithm・型不一致を含む）。
    #[error("TOML parse failed {path}: {reason}")]
    ParseFailed { path: PathBuf, reason: String },
}

/// ディレクトリ内の `*.toml` を走査し、各1ファイルを1 [`TaskDef`] として読み込む。
///
/// [`crate::template_store::TemplateStore::load_from_directory`] と同構造
/// （`read_dir` → 拡張子フィルタ → パース → 集約）。順序は保持する（`Vec<TaskDef>`）。
///
/// 相対 `template` パスは各 TOML ファイルの親ディレクトリ基準で絶対化する。
/// 画像は遅延読込（パスだけ保持し、[`TaskDef::detect`] 呼出時に `image::open`）。
/// 存在しないディレクトリ・空ディレクトリは空 `Vec` を返す。
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

        // *.toml のみ（template_store::is_image_file の TOML版）
        let is_toml = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("toml"))
            .unwrap_or(false);
        if !is_toml {
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

        // template 相対パスを TOMLファイルの親ディレクトリ基準で絶対化
        if !def.template.is_absolute() && path.parent().is_some() {
            let parent = path.parent().expect("parent exists");
            def.template = parent.join(&def.template);
        }

        debug!("Loaded pipeline task '{}' from {:?}", def.name, path);
        defs.push(def);
    }

    Ok(defs)
}

/// 1ステップの実行結果。action は省略時 [`Action::DoNothing`] を補充済み。
///
/// `next` は空の場合あり（終端タスク）。base 継承解決は範囲外のため `next` は
/// [`TaskDef::next`] の生の値をそのまま返す。
///
/// `matched_region` はマッチしたテンプレート位置（[`MatchResult::region`]）。caller はこれを
/// [`Action::ClickSelf`] のクリック座標計算などに使う（純粋層 [`crate`] 外の pipeline_runner）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepOutcome {
    /// マッチしたタスク名（= `run_step` の `current_name` と一致するはず）。
    pub matched_task: String,
    /// 実行すべきアクション（action 省略時は [`Action::DoNothing`]）。
    pub action: Action,
    /// next 遷移先リスト（空可）。
    pub next: Vec<String>,
    /// マッチしたテンプレート領域（スクリーンショット元解像度の座標）。
    pub matched_region: ScreenRegion,
}

/// `current_name` の [`TaskDef`] を探して1ステップ実行（detect）する。
///
/// 戻り値 [`Option<StepOutcome>`]:
/// - `current_name` が `tasks` に無い → `None`（未知タスク名）。
/// - `detect` がテンプレ読込失敗等の [`Err`] → `None`（`Result` を表面に出さず潰す）。
/// - `detect` が `Ok(None)`（閾値下・ROI 外）→ `None`。
/// - `detect` が `Ok(Some(_))` → action（省略時 [`Action::DoNothing`] 補充）+ next で [`StepOutcome`] 構築。
///
/// 本スライスは「1ステップ」のみ。next への自動遷移ループは caller 側で戻り値の `next` を
/// 見て次の `current_name` を決める形で実装する（本関数内では行わない）。
///
/// `template` は [`load_pipeline`] 経由で絶対化済みを想定する。`detect` 内部で
/// `self.template.is_absolute()` なら `template_root` を無視するため、空 [`Path`] を渡す。
pub fn run_step(
    tasks: &[TaskDef],
    screenshot: &DynamicImage,
    current_name: &str,
) -> Option<StepOutcome> {
    let task = tasks.iter().find(|t| t.name == current_name)?;
    match task.detect(screenshot, Path::new("")) {
        Ok(Some(matched)) => {
            let action = task.action.clone().unwrap_or(Action::DoNothing);
            let next = task.next.clone().unwrap_or_default();
            Some(StepOutcome {
                matched_task: task.name.clone(),
                action,
                next,
                matched_region: matched.region,
            })
        }
        Ok(None) => None, // 閾値下 / ROI 外
        Err(_e) => {
            // テンプレ欠落等。tracing::warn 推奨だが、本スライスでは None に潰す。
            None
        }
    }
}

/// ROI cropping ヘルパ。はみ出し時は画面サイズに clamp する。
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

    /// `(x+y) mod 64` の勾配パターン（ccoeff.rs テスト準拠。denomT≠0 を保証）。
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

    /// `(haystack_w, haystack_h)` の背景 `bg` の上に needle を `(ox, oy)` に埋め込んだ画像。
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

    /// `dir` に `filename` を作成し `body` を書き込む。
    fn write_toml(dir: &Path, filename: &str, body: &str) -> PathBuf {
        let path = dir.join(filename);
        fs::write(&path, body).expect("write toml");
        path
    }

    /// テンプレPNGを `dir/scenes/title/title_center.png` に保存。
    fn write_template(dir: &Path, needle: &GrayImage) -> PathBuf {
        let sub = dir.join("scenes").join("title");
        fs::create_dir_all(&sub).expect("mkdir");
        let p = sub.join("title_center.png");
        needle.save(&p).expect("save png");
        p
    }

    // ---- テスト1: TOML解析 ----

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
        // template が親ディレクトリ基準で絶対化されている
        assert!(d.template.is_absolute(), "template must be absolute");
        assert_eq!(d.template, tmp.path().join("scenes/title/title_center.png"));
    }

    #[test]
    fn load_pipeline_rejects_unknown_field() {
        let tmp = tempfile::tempdir().expect("tempdir");
        // typo: threshild
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

    // ---- テスト2: ROI指定 detect ----

    #[test]
    fn detect_with_roi_finds_embedded_position() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let needle = gradient_needle(40, 40);
        // 720p基準(1280x720)。ROI = [520,320,240,80] (x:520..760, y:320..400)。
        // needle(40x40) を ROI 内に完全に収まる (600,340) に埋める（x:600..640, y:340..380）。
        let screenshot = luma_dyn(embed(1280, 720, &needle, 600, 340, 128));
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
        // ROI オフセット戻し済み → 元座標。埋込 (600,340) 付近。
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
        // needle を ROI 外 (50,50) に埋める。
        let screenshot = luma_dyn(embed(1280, 720, &needle, 50, 50, 128));
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

    /// 全面走査テスト用の小キャンバス。CCOEFF は純Rust O(N·M) なので
    /// 1280x720 全面走査は遅い（CI 実行時間健全化）。16:9 縮小版で検証する。
    const FULL_W: u32 = 320;
    const FULL_H: u32 = 180;

    #[test]
    fn detect_without_roi_finds_position_fullscan() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let needle = gradient_needle(40, 40);
        // downscale=1 なので奇数座標でも完全一致埋込なら confidence=1.0。
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

    // ---- テスト3: algorithm 切替 ----

    #[test]
    fn detect_switches_sse_and_ccoeff() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let needle = gradient_needle(40, 40);
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

        // 両者とも埋込位置を発見
        for (label, x) in [("sse", sse_m.region.x), ("ccoeff", cc_m.region.x)] {
            assert!(
                (148..=152).contains(&x),
                "{label} region.x at embed: got {x}"
            );
        }
        for (label, y) in [("sse", sse_m.region.y), ("ccoeff", cc_m.region.y)] {
            assert!((73..=77).contains(&y), "{label} region.y at embed: got {y}");
        }
        // 両者とも閾値を超える（完全一致埋込）
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

    // ---- テスト4: 閾値境界 ----

    /// needle に定数オフセットを加えたテンプレ（完全一致ではないが構造は同じ）。
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
        let base = gradient_needle(40, 40);
        let screenshot = luma_dyn(embed(FULL_W, FULL_H, &base, 150, 75, 128));
        // 埋込画像は base。テンプレートは輝度シフト版（≠完全一致）。
        // SSE は絶対輝度差を測るため、輝度シフトで confidence が大きく低下する
        // （CCOEFF は平均除去で定数シフトを完全に吸収し 1.0 のままになるため、閾値境界の
        // 証明には SSE を使う）。
        let tpl_needle = shifted_needle(&base, 60);
        write_template(tmp.path(), &tpl_needle);

        // 低閾値では一致する（位置は解決できる）
        let low_task = TaskDef {
            name: "T".into(),
            state: "T".into(),
            algorithm: Algorithm::Sse,
            template: tmp.path().join("scenes/title/title_center.png"),
            roi: None,
            threshold: 0.1,
            base: None,
            action: None,
            next: None,
        };
        let low = low_task
            .detect(&screenshot, tmp.path())
            .expect("detect ok")
            .expect("should match at low threshold");
        // 輝度シフト版テンプレ → SSE confidence は 1.0 より十分低い
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

        // 到達可能信頼度より高い閾値 → None（閾値が結果を gating していることの証明）
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

    // ---- テスト5: テンプレート欠落 ----

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

    // ---- テスト6: Action の TOML 解析 ----

    /// action 付きの最小 TaskDef TOML 本体（共通フィールド + action ブロック）。
    /// `template` は存在しなくても load_pipeline のパース段階では読込しないため OK。
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

    // ---- テスト7: run_step ----

    /// run_step テスト共通セットアップ。gradient_needle(40,40) を FULL_W×FULL_H
    /// キャンバスに (150,75) で埋込んだ screenshot と、絶対パステンプレを返す。
    /// `with_needle=false` なら背景のみの screenshot を返す。
    /// tempdir を `.into_path()` で永続化し、drop によるテンプレ削除を回避する
    /// （OS 一時領域に残るが、テスト成否には影響しない）。
    fn run_step_setup(with_needle: bool) -> (DynamicImage, PathBuf) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let needle = gradient_needle(40, 40);
        let screenshot = if with_needle {
            luma_dyn(embed(FULL_W, FULL_H, &needle, 150, 75, 128))
        } else {
            luma_dyn(GrayImage::from_pixel(FULL_W, FULL_H, Luma([128u8])))
        };
        let tpl = write_template(tmp.path(), &needle);
        // tempdir を永続化（drop でテンプレが消えるのを防ぐ）。ディレクトリは OS 一時領域に残る。
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

        let out = run_step(&tasks, &screenshot, "Title").expect("should match");
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

        let out = run_step(&tasks, &screenshot, "Title").expect("should match");
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
        // 背景のみ（needle 無）→ detect は Ok(None)
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

        let out = run_step(&tasks, &screenshot, "Title");
        assert!(out.is_none(), "no needle in screenshot must yield None");

        let _ = std::fs::remove_file(&tpl);
    }

    #[test]
    fn run_step_below_threshold_returns_none() {
        // SSE + 輝度シフト版テンプレで「到達可能信頼度」を 1.0 未満に下げ、
        // それより高い閾値を設定 → detect が Ok(None) → run_step も None。
        // （MatchConfidence::new は [0,1] にクランプするため、閾値 1.1 では gating できない。
        //  detect_threshold_above_achievable_returns_none と同じ手法を採る）
        let tmp = tempfile::tempdir().expect("tempdir");
        let base = gradient_needle(40, 40);
        let screenshot = luma_dyn(embed(FULL_W, FULL_H, &base, 150, 75, 128));
        let tpl_needle = shifted_needle(&base, 60);
        write_template(tmp.path(), &tpl_needle);

        // 低閾値で到達可能信頼度を計測
        let probe = TaskDef {
            name: "Title".into(),
            state: "Title".into(),
            algorithm: Algorithm::Sse,
            template: tmp.path().join("scenes/title/title_center.png"),
            roi: None,
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

        // 到達可能信頼度より高い閾値 → run_step は None
        let tasks = vec![TaskDef {
            threshold: achievable + 0.05,
            ..probe
        }];
        let out = run_step(&tasks, &screenshot, "Title");
        assert!(
            out.is_none(),
            "threshold above achievable confidence ({achievable}) must yield None"
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

        // 存在しないタスク名 → 検索失敗経路
        let out = run_step(&tasks, &screenshot, "NoSuchTask");
        assert!(out.is_none(), "unknown task name must yield None");

        let _ = std::fs::remove_file(&tpl);
    }

    #[test]
    fn run_step_missing_template_returns_none() {
        let (screenshot, _tpl) = run_step_setup(true);
        // template を存在しないパスに → detect が Err → None に潰す
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
        assert!(
            out.is_none(),
            "missing template (Err path) must yield None, not propagate Err"
        );
    }

    // ---- テスト8: ドキュメント推奨 TOML 例との同期 ----
    //
    // Tool-Reference / Declarative-Tasks-Design の「実装済み範囲」に載せた
    // 推奨 TOML 例が `toml::from_str::<TaskDef>` で解析できることを検証する。
    // ドキュメント更新者が誤って `[task.x]` 表記等に戻した場合、このテストが即座に壊れ、
    // ドキュメントと実装スキーマのズレを CI で検知できる。

    /// 最小タスク（タイトル画面クリック）。インライン action + フラット next。
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

    /// click_rect インライン action（roi 含む）の解析検証。
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

    /// swipe インライン action の解析検証。
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

    /// 停止タスク（終端）。action=stop。
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

    /// 順序の罠の回帰テスト: インライン action の後に next を置いても
    /// action テーブルスコープに吸われず、正しく TaskDef 直下へマッピングされること。
    /// インライン表記の価値（順序非依存）を保証する。これが壊れる = 要再設計。
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

    /// ドキュメント「禁止パターン」の回帰テスト: `[task.X]` 名前空間表記は
    /// deny_unknown_fields で拒否されること（フラット構造の前提を固定）。
    #[test]
    fn doc_forbidden_namespace_table_rejected() {
        let toml_body = r#"
            [task.TitleScreen]
            algorithm = "ccoeff"
        "#;
        let err = toml::from_str::<TaskDef>(toml_body);
        assert!(err.is_err(), "namespace [task.X] must be rejected");
    }

    // ---- PC版(Windows, 16:9)パイプラインスライス(T5) ----
    //
    // 実機(20:9)テンプレ templates/pipelines/field_loop/*, nav_to_field/* を上書きせず、
    // PC版は pc-scoped 名前空間(field_loop_pc/, nav_to_field_pc/, scenes/field_pc/)へ隔離する
    // (T7 の 20:9→16:9 劣化検証は両者の共存を前提とするため)。
    // 全 ROI は RAW 1258x708 ピクセル空間(ScreenScaler は width<=1280 で非変換=RAW 通過)。

    /// workspace の `templates/` ルートを返す。
    /// anaden-vision クレート(crates/anaden-vision)からは `../../templates`。
    fn workspace_templates_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("templates")
    }

    /// ROI の全要素が 1258x708 キャンバスに収まり、かつ幅/高さが 0 でないことを検証。
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
        // tap_bottom + tap_hud_tr の 2 タスク。
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
        // field 到達 = 目標なので発火しない。テンプレは scenes/field_pc/ へ参照する。
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
    fn pc_pipeline_namespace_does_not_collide_with_20x9_field_loop() {
        // T7 の劣化検証が両者共存を前提とするため、PC 名前空間は 20:9 を上書きしない。
        let pc = workspace_templates_root()
            .join("pipelines")
            .join("field_loop_pc");
        let legacy = workspace_templates_root()
            .join("pipelines")
            .join("field_loop");
        assert!(pc.exists(), "field_loop_pc namespace must exist");
        assert!(
            legacy.exists(),
            "legacy field_loop must still exist (not clobbered)"
        );

        let pc_names: Vec<String> = load_pipeline(&pc)
            .expect("pc load")
            .into_iter()
            .map(|d| d.name)
            .collect();
        let legacy_names: Vec<String> = load_pipeline(&legacy)
            .expect("legacy load")
            .into_iter()
            .map(|d| d.name)
            .collect();
        // 名前空間が分離済み(同名タスクの衝突無し)。
        for n in &pc_names {
            assert!(
                !legacy_names.contains(n),
                "PC task '{n}' collides with legacy 20:9 namespace"
            );
        }
    }

    // ---- PC版 field_pc シーンテンプレスライス (Task#3 / Issue#5) ----
    //
    // templates/scenes/field_pc/ に PC版(16:9, RAW 1258x708) 参照テンプレ3つ
    // (template_01, hud_top, hud_topright) を新設する。
    // 20:9 既存 templates/scenes/field/* は PC の 16:9 フレーム上で劣化し(conf 0.6723 < 0.80)
    // field_loop が --target windows でマッチしない現状を、pc-scoped 名前空間で解除する。
    // 全 ROI は RAW 1258x708 空間。TOML は TaskDef スキーマ(algorithm+template+平坦 roi)。
    //
    // TaskDef::detect で capture_probe.png(T7 PrintWindow 実機フレーム) 上の
    // 各 PC テンプレが threshold(0.95) 以上でマッチすることを E2E 検証する。

    fn field_pc_dir() -> PathBuf {
        workspace_templates_root().join("scenes").join("field_pc")
    }

    /// field_pc/ の3 TOML が TaskDef スキーマで parse でき、PNG が存在し、
    /// ROI が RAW 1258x708 に収まることを検証。
    #[test]
    fn pc_field_pc_scene_templates_load_and_validate() {
        let dir = field_pc_dir();
        let defs = load_pipeline(&dir).expect("field_pc scene dir must load");
        // template_01 + hud_top + hud_topright の3タスク。
        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        assert!(
            names.len() >= 3,
            "field_pc must contain template_01 + hud_top + hud_topright, got {names:?}"
        );

        for d in &defs {
            // TaskDef スキーマ(legacy method+[roi] は deny_unknown_fields で弾かれる)。
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

    /// field_pc ネームスペースは 20:9 scenes/field と名前衝突しない
    /// (pc-scoped ACL 隔離。T7 劣化検証が共存を前提とする)。
    ///
    /// 注: legacy scenes/field/*.toml は旧 schema(`method=` + `[roi]` サブテーブル)で
    /// TaskDef の deny_unknown_fields と非互換(toml::from_str が ParseFailed)のため、
    /// ここでは legacy TOML をパースせず、TaskDef 名と legacy *ファイル名* の重複を
    /// 比較する(名前空間分離の本質は同名衝突の回避であり、schema 非互換とは無関係)。
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
        // legacy scenes/field の *.toml ファイル名(stem)を名前空間識別子として使う。
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
        // field_pc の 3 TaskDef 名が legacy stem(template_01/hud_top/hud_topright)と
        // 衝突しないことを具体的に保証(FieldPc* 接頭辞で分離済み)。
        assert!(
            !pc_names.iter().any(|n| legacy_stems.contains(n)),
            "no PC task name may equal a legacy scenes/field filename stem"
        );
    }

    /// field_pc 用キャプチャプローブのパスを解決する。
    /// 優先順位(menu_pc_probe_path と同じ規約):
    ///   1. `templates/captures/field_pc_probe.png`(規約位置・tracked)
    ///   2. workspace ルート直下 `capture_probe.png`(開発時の暫定プローブ)
    ///
    /// Why not 旧インライン resolver: 旧実装は workspace ルートの `capture_probe.png`
    /// (`.gitignore:17` で除外) のみを見ており、fresh clone では存在せず absence-skip で
    /// 偽 green になっていた。規約位置の tracked プローブを primary にすることで再現性を保証し、
    /// menu_pc と同じ primary/fallback 構造に揃えて drift を防ぐ(Issue #9)。
    ///
    /// 見つからなければ None(CI フォーク等では検証をスキップ)。
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

    /// E2E: field_pc_probe.png(T7 PrintWindow 実機 1258x708 field フレーム) 上で
    /// 各 PC field テンプレが threshold 以上の confidence でマッチする。
    /// これが --target windows で field_loop がアンブロックされる直接証明。
    #[test]
    fn pc_field_pc_templates_match_real_capture_above_threshold() {
        let dir = field_pc_dir();
        let probe_path = match field_pc_probe_path() {
            Some(p) => p,
            None => {
                // プローブ未整備の環境(CI フォーク等)では検証をスキップ。
                eprintln!(
                    "skip: field_pc_probe.png not found \
                     (neither templates/captures/ nor workspace-root capture_probe.png)"
                );
                return;
            }
        };
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

    /// field_loop_pc/ の各 TaskDef が field_pc_probe.png 上で threshold 以上でマッチする。
    /// `--target windows` + `field_loop_pc` パイプラインのオフライン/E2E 前提契約。
    #[test]
    fn pc_field_loop_pc_templates_match_real_capture_above_threshold() {
        let dir = workspace_templates_root()
            .join("pipelines")
            .join("field_loop_pc");
        let probe_path = match field_pc_probe_path() {
            Some(p) => p,
            None => {
                eprintln!(
                    "skip: field_pc_probe.png not found \
                     (neither templates/captures/ nor workspace-root capture_probe.png)"
                );
                return;
            }
        };
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

    // ---- PC版 menu_pc シーンテンプレスライス (Task#2 / Issue#6 / 親 Issue#5) ----
    //
    // templates/scenes/menu_pc/ に PC版(16:9, RAW 1258x708) 参照テンプレ7件
    // (bag/board/gacha/grasta/info/party/record) を新設する。20:9 既存 templates/scenes/menu/*
    // は PC の 16:9 フレーム上で劣化するため、pc-scoped 名前空間(menu_pc)で隔離する。
    // 全 ROI は RAW 1258x708 空間(ScreenScaler は width<=1280 で非変換=RAW 通過)。
    // TOML は TaskDef スキーマ(algorithm=sse + template + 平坦 roi)。20:9 legacy menu は
    // 旧 schema(method + [roi] サブテーブル)で TaskDef と非互換のため共存(非破壊)。

    fn menu_pc_dir() -> PathBuf {
        workspace_templates_root().join("scenes").join("menu_pc")
    }

    /// menu_pc/ の7 TOML が TaskDef スキーマで parse でき、PNG が存在し、
    /// ROI が RAW 1258x708 に収まり、全て SSE アルゴリズム・threshold>=0.95 であることを検証。
    #[test]
    fn pc_menu_pc_scene_templates_load_and_validate() {
        let dir = menu_pc_dir();
        let defs = load_pipeline(&dir).expect("menu_pc scene dir must load");
        // bag + board + gacha + grasta + info + party + record の7タスク。
        assert_eq!(
            defs.len(),
            7,
            "menu_pc must contain bag/board/gacha/grasta/info/party/record (7 tasks), got {}",
            defs.len()
        );

        for d in &defs {
            // TaskDef スキーマ(legacy method+[roi] は deny_unknown_fields で弾かれる)。
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

    /// menu_pc ネームスペースは 20:9 legacy scenes/field および scenes/menu と
    /// 名前衝突しない(pc-scoped ACL 隔離。非破壊共存の前提)。
    ///
    /// 注: legacy scenes/{field,menu}/*.toml は旧 schema(`method=` + `[roi]` サブテーブル)で
    /// TaskDef の deny_unknown_fields と非互換(toml::from_str が ParseFailed)のため、
    /// ここでは legacy TOML をパースせず、TaskDef 名と legacy *ファイル名* の重複を比較する
    /// (名前空間分離の本質は同名衝突の回避であり、schema 非互換とは無関係)。
    #[test]
    fn pc_menu_pc_namespace_does_not_collide_with_scenes_field_or_menu() {
        let pc_dir = menu_pc_dir();
        assert!(pc_dir.exists(), "scenes/menu_pc namespace must exist");

        let pc_names: Vec<String> = load_pipeline(&pc_dir)
            .expect("menu_pc load")
            .into_iter()
            .map(|d| d.name)
            .collect();

        // legacy scenes/field と scenes/menu の *.toml ファイル名(stem)を名前空間識別子として使う。
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
        // menu_pc の全 TaskDef 名が legacy field/menu stem と衝突しないことを保証。
        assert!(
            !pc_names.is_empty(),
            "menu_pc must define at least one task for namespace isolation check"
        );
    }

    /// menu_pc 用キャプチャプローブのパスを解決する。
    /// 優先順位:
    ///   1. `templates/captures/menu_pc_probe.png`(規約位置)
    ///   2. workspace ルート直下 `menu_pc_probe.png`
    ///
    /// 見つからなければ None(CI フォーク等では検証をスキップ)。
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

    /// E2E: menu_pc_probe.png(16:9 PC RAW 1258x708 メニューフレーム) 上で
    /// 各 menu_pc テンプレ 7 件(bag/board/gacha/grasta/info/party/record)が
    /// threshold(>=0.95) 以上の confidence でマッチし、かつ ROI が 1258x708 に
    /// 収まることを検証する。これが conf>=0.95 acceptance gate。
    ///
    /// `pc_field_pc_templates_match_real_capture_above_threshold`(pipeline.rs:1609)
    /// と同じ absence-skip パターンを採用し、プローブ or menu_pc ネームスペースが
    /// 未整備の環境(CI フォーク等)では検証をスキップしてビルドを壊さない。
    #[test]
    fn pc_menu_pc_templates_match_real_capture_above_threshold() {
        let dir = menu_pc_dir();
        if !dir.exists() {
            eprintln!("skip: menu_pc namespace not found at {:?}", dir);
            return;
        }
        let probe_path = match menu_pc_probe_path() {
            Some(p) => p,
            None => {
                eprintln!(
                    "skip: menu_pc_probe.png not found \
                     (neither templates/captures/ nor workspace root)"
                );
                return;
            }
        };

        let defs = load_pipeline(&dir).expect("menu_pc load");
        // menu 底部 7 アイコン(bag/board/gacha/grasta/info/party/record)。
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
            // acceptance gate: threshold 自体も >=0.95 を要求(acceptance 地盤)。
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
}
