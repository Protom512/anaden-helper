//! テンプレート照合の唯一の実装。
//!
//! `TaskDef::detect`、`Task::detect`、`Pipeline::inspect` はすべてここを通る。
//! ROI の正規化・クロップ・アルゴリズム分岐・領域のオフセットを 1 箇所に閉じる。

use std::path::{Path, PathBuf};

use anaden_core::{MatchConfidence, ScreenRegion};
use image::DynamicImage;

use crate::ccoeff::CcoeffVisionEngine;
use crate::engine::{SseVisionEngine, VisionEngine};
use crate::matcher::{MatchResult, TemplateMatcher};

use super::cache::prepared_template;
use super::schema::{Algorithm, TaskDef, TaskDefError};

/// 1 テンプレートの照合結果。`Pipeline::inspect` が診断情報を組み立てるのに使う。
pub(crate) struct Probe {
    pub(crate) matched: Option<MatchResult>,
    pub(crate) needle_size: Option<(u32, u32)>,
}

/// テンプレート 1 枚を 1 フレームに対して照合する。
///
/// `template` は解決済みのパス（絶対、または呼び出し側が決めた基準からの相対）。
pub(crate) fn probe_template(
    template: &Path,
    roi: Option<[u32; 4]>,
    algorithm: Algorithm,
    threshold: f32,
    screenshot: &DynamicImage,
) -> Result<Probe, TaskDefError> {
    let norm_w = screenshot.width();
    let norm_h = screenshot.height();
    let prepared = prepared_template(template, norm_w, norm_h)?;

    let scaled_roi = roi_to_region(roi).map(|r| {
        let s = crate::scale::roi_to_normalized([r.x, r.y, r.width, r.height], norm_w, norm_h);
        ScreenRegion::new(s[0], s[1], s[2], s[3])
    });
    let work = match &scaled_roi {
        Some(r) => crop_imm(screenshot, *r),
        None => screenshot.clone(),
    };

    let thr = MatchConfidence::new(threshold);
    let result = match algorithm {
        Algorithm::Sse => {
            let m = TemplateMatcher::threshold_only(thr);
            SseVisionEngine::new(m).match_template(&work, &prepared.needle)
        }
        Algorithm::Ccoeff => {
            CcoeffVisionEngine::threshold_only(thr).match_template_prepared(&work, &prepared.ccoeff)
        }
    };

    Ok(Probe {
        matched: result.map(|mut mr| {
            if let Some(r) = scaled_roi {
                mr.region = ScreenRegion::new(
                    mr.region.x + r.x,
                    mr.region.y + r.y,
                    mr.region.width,
                    mr.region.height,
                );
            }
            mr
        }),
        needle_size: Some((prepared.needle.width(), prepared.needle.height())),
    })
}

pub(crate) fn roi_to_region(roi: Option<[u32; 4]>) -> Option<ScreenRegion> {
    let [x, y, w, h] = roi?;
    if w == 0 || h == 0 {
        return None;
    }
    Some(ScreenRegion::new(x, y, w, h))
}

pub(crate) fn crop_imm(img: &DynamicImage, r: ScreenRegion) -> DynamicImage {
    let x = r.x.min(img.width().saturating_sub(1));
    let y = r.y.min(img.height().saturating_sub(1));
    let w = r.width.min(img.width().saturating_sub(x));
    let h = r.height.min(img.height().saturating_sub(y));
    img.crop_imm(x, y, w, h)
}

/// `root` が空ならテンプレートの相対パスをそのまま返す（現行挙動）。
pub(crate) fn resolve_template(template: &Path, root: &Path) -> PathBuf {
    if template.is_absolute() {
        template.to_path_buf()
    } else {
        root.join(template)
    }
}

impl TaskDef {
    /// ワイヤ行のまま照合する。相対 `template` は `template_root` から解決する。
    ///
    /// 既存の呼び出し側（テスト・診断）との互換のために残している。
    /// 新しいコードは [`super::runner::Pipeline::step`] を使う。
    pub fn detect(
        &self,
        screenshot: &DynamicImage,
        template_root: &Path,
    ) -> Result<Option<MatchResult>, TaskDefError> {
        let tpl = resolve_template(&self.template, template_root);
        Ok(probe_template(&tpl, self.roi, self.algorithm, self.threshold, screenshot)?.matched)
    }
}
