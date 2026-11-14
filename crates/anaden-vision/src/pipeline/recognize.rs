use std::path::{Path, PathBuf};

use anaden_core::{MatchConfidence, ScreenRegion};
use image::DynamicImage;

use crate::ccoeff::CcoeffVisionEngine;
use crate::engine::{SseVisionEngine, VisionEngine};
use crate::matcher::{MatchResult, TemplateMatcher};

use super::cache::prepared_template;
use super::schema::{Algorithm, TaskDef, TaskDefError};

pub(crate) struct Probe {
    pub(crate) matched: Option<MatchResult>,
    pub(crate) needle_size: Option<(u32, u32)>,
}

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

pub(crate) fn resolve_template(template: &Path, root: &Path) -> PathBuf {
    if template.is_absolute() {
        template.to_path_buf()
    } else {
        root.join(template)
    }
}

impl TaskDef {
    pub fn detect(
        &self,
        screenshot: &DynamicImage,
        template_root: &Path,
    ) -> Result<Option<MatchResult>, TaskDefError> {
        let tpl = resolve_template(&self.template, template_root);
        Ok(probe_template(&tpl, self.roi, self.algorithm, self.threshold, screenshot)?.matched)
    }
}
