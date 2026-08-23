//! テンプレート別 conf 内訳の診断モード（Issue #71）。
//!
//! [`crate::pipeline::TaskDef::detect`] と同じ ROI/needle スケール経路
//! （[`crate::scale::roi_to_normalized`] / [`crate::scale::needle_to_normalized`]）を通り、
//! threshold=0.0 のエンジンで best conf を閾値によらず取得する。
//! NoMatch 原因特定をデータドリブン化するための純粋関数層（ファイル IO なし）。

use std::path::PathBuf;

use image::DynamicImage;

use anaden_core::{MatchConfidence, ScreenRegion};

use crate::ccoeff::CcoeffVisionEngine;
use crate::engine::{SseVisionEngine, VisionEngine};
use crate::matcher::TemplateMatcher;
use crate::pipeline::{Algorithm, TaskDef};
use crate::scale::{
    PC_CLIENT_HEIGHT_MEASURED, PC_CLIENT_WIDTH_MEASURED, needle_to_normalized, roi_to_normalized,
};

/// 1タスク分の診断結果。
///
/// `best_confidence` / `best_region` は「マッチ不能（needle が画面より大きい等）」の
/// skipped 表現として `None` を取る。閾値は `TaskDef::threshold` のまま報告するが、
/// 取得自体は threshold=0.0 で行うため閾値に切り捨てられない。
#[derive(Debug, Clone)]
pub struct DiagnoseEntry {
    /// タスク名。
    pub task: String,
    /// テンプレート画像パス。
    pub template_path: PathBuf,
    /// 認識アルゴリズム。
    pub algorithm: Algorithm,
    /// TaskDef 宣言の閾値（報告用。取得には使われない）。
    pub threshold: f32,
    /// 閾値 0.0 で取得した best conf。マッチ不能時は `None`。
    pub best_confidence: Option<f32>,
    /// best conf の領域（screenshot と同じ座標空間・ROI オフセット戻し済み）。
    pub best_region: Option<ScreenRegion>,
    /// 正規化後空間へスケールした ROI。`roi = None`（全面）時は `None`。
    pub scaled_roi: Option<ScreenRegion>,
    /// X/Y 別スケール倍率 `(sx, sy)`（norm/1258, norm/708）。
    pub scale_factors: (f32, f32),
    /// スケール後の needle 寸法 `(w, h)`。テンプレ読込失敗時は `None`。
    pub needle_scaled_size: Option<(u32, u32)>,
    /// マッチ不能・読込失敗の理由。正常時は `None`。
    pub error: Option<String>,
}

/// 1タスクを診断する。テンプレート読込失敗や needle 過大もエントリとして返す（Result ではない）。
pub fn diagnose_task(task: &TaskDef, screenshot: &DynamicImage) -> DiagnoseEntry {
    let norm_w = screenshot.width();
    let norm_h = screenshot.height();
    let sx = if PC_CLIENT_WIDTH_MEASURED == 0 {
        1.0
    } else {
        norm_w as f32 / PC_CLIENT_WIDTH_MEASURED as f32
    };
    let sy = if PC_CLIENT_HEIGHT_MEASURED == 0 {
        1.0
    } else {
        norm_h as f32 / PC_CLIENT_HEIGHT_MEASURED as f32
    };

    let scaled_roi = task.roi.and_then(|r| {
        let (w, h) = (r[2], r[3]);
        if w == 0 || h == 0 {
            return None;
        }
        let s = roi_to_normalized(r, norm_w, norm_h);
        Some(ScreenRegion::new(s[0], s[1], s[2], s[3]))
    });

    let mut entry = DiagnoseEntry {
        task: task.name.clone(),
        template_path: task.template.clone(),
        algorithm: task.algorithm,
        threshold: task.threshold,
        best_confidence: None,
        best_region: None,
        scaled_roi,
        scale_factors: (sx, sy),
        needle_scaled_size: None,
        error: None,
    };

    let needle = match image::open(&task.template) {
        Ok(n) => n,
        Err(e) => {
            entry.error = Some(format!("template load failed: {e}"));
            return entry;
        }
    };
    let needle = needle_to_normalized(&needle, norm_w, norm_h);
    entry.needle_scaled_size = Some((needle.width(), needle.height()));

    let work = match entry.scaled_roi {
        Some(r) => crop_clamped(screenshot, r),
        None => screenshot.clone(),
    };

    let thr = MatchConfidence::new(0.0);
    let result = match task.algorithm {
        Algorithm::Sse => SseVisionEngine::new(TemplateMatcher::threshold_only(thr))
            .match_template(&work, &needle),
        Algorithm::Ccoeff => CcoeffVisionEngine::threshold_only(thr).match_template(&work, &needle),
    };

    match result {
        Some(mut m) => {
            if let Some(r) = entry.scaled_roi {
                m.region = ScreenRegion::new(
                    m.region.x + r.x,
                    m.region.y + r.y,
                    m.region.width,
                    m.region.height,
                );
            }
            entry.best_confidence = Some(m.confidence.0);
            entry.best_region = Some(m.region);
        }
        None => {
            // threshold=0.0 で None = エンジンが走査不能（needle が画面/ROI より大きい等）。
            entry.error = Some("needle larger than haystack (or empty scan space)".to_string());
        }
    }

    entry
}

/// 全タスクを診断し、best_confidence 降順（`None` は末尾）で返す。
pub fn diagnose_all(tasks: &[TaskDef], screenshot: &DynamicImage) -> Vec<DiagnoseEntry> {
    let mut entries: Vec<DiagnoseEntry> =
        tasks.iter().map(|t| diagnose_task(t, screenshot)).collect();
    entries.sort_by(|a, b| match (a.best_confidence, b.best_confidence) {
        (Some(x), Some(y)) => y.partial_cmp(&x).unwrap_or(std::cmp::Ordering::Equal),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });
    entries
}

/// ROI cropping ヘルパ（pipeline::crop_imm と同じ clamp 挙動）。
fn crop_clamped(img: &DynamicImage, r: ScreenRegion) -> DynamicImage {
    let x = r.x.min(img.width().saturating_sub(1));
    let y = r.y.min(img.height().saturating_sub(1));
    let w = r.width.min(img.width().saturating_sub(x));
    let h = r.height.min(img.height().saturating_sub(y));
    img.crop_imm(x, y, w, h)
}

/// エントリの conf と threshold の差分（conf - threshold）。閾値未達は負値。
/// skipped（`None`）時は `None`。
fn threshold_diff(entry: &DiagnoseEntry) -> Option<f32> {
    entry.best_confidence.map(|c| c - entry.threshold)
}

/// conf 降順（`None` は末尾）の比較関数。[`diagnose_all`] と同一順序。
fn compare_conf_desc(a: &DiagnoseEntry, b: &DiagnoseEntry) -> std::cmp::Ordering {
    match (a.best_confidence, b.best_confidence) {
        (Some(x), Some(y)) => y.partial_cmp(&x).unwrap_or(std::cmp::Ordering::Equal),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    }
}

/// 診断レポート全体を markdown 文字列として生成する純粋関数（ファイル IO なし）。
///
/// 含まれる要素:
/// - ヘッダ（title / haystack サイズ / エントリ数）
/// - conf 降順テーブル（未ソート入力でも降順に行を並べ替える）
/// - 各行: conf / threshold / 差分 / scaled ROI / スケール倍率 / needle 寸法 / 領域 / 備考
pub fn format_diagnose_report(
    title: &str,
    haystack_size: (u32, u32),
    entries: &[DiagnoseEntry],
) -> String {
    let mut sorted: Vec<&DiagnoseEntry> = entries.iter().collect();
    sorted.sort_by(|a, b| compare_conf_desc(a, b));

    let mut out = String::new();
    out.push_str("# NoMatch Diagnose Report\n\n");
    out.push_str(&format!("- title: {title}\n"));
    out.push_str(&format!(
        "- haystack: {}x{}\n",
        haystack_size.0, haystack_size.1
    ));
    out.push_str(&format!("- templates: {}\n\n", entries.len()));
    out.push_str(
        "| # | task | conf | threshold | diff | roi (scaled) | scale (x,y) | needle | best region | note |\n",
    );
    out.push_str(
        "|---|------|------|-----------|------|--------------|-------------|--------|-------------|------|\n",
    );
    for (i, e) in sorted.iter().enumerate() {
        let conf = match e.best_confidence {
            Some(c) => format!("{c:.4}"),
            None => "-".to_string(),
        };
        let diff = match threshold_diff(e) {
            Some(d) => format!("{d:+.4}"),
            None => "-".to_string(),
        };
        let roi = match e.scaled_roi {
            Some(r) => format!("({},{}) {}x{}", r.x, r.y, r.width, r.height),
            None => "full".to_string(),
        };
        let needle = match e.needle_scaled_size {
            Some((w, h)) => format!("{w}x{h}"),
            None => "-".to_string(),
        };
        let region = match &e.best_region {
            Some(r) => format!("({},{}) {}x{}", r.x, r.y, r.width, r.height),
            None => "-".to_string(),
        };
        out.push_str(&format!(
            "| {} | {} | {} | {:.4} | {} | {} | ({:.3},{:.3}) | {} | {} | {} |\n",
            i + 1,
            e.task,
            conf,
            e.threshold,
            diff,
            roi,
            e.scale_factors.0,
            e.scale_factors.1,
            needle,
            region,
            e.error.as_deref().unwrap_or("")
        ));
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use image::{GrayImage, Luma};

    /// (x+y) mod 64 の勾配パターン（denomT≠0 を保証。pipeline.rs テスト準拠）。
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

    fn embed(w: u32, h: u32, needle: &GrayImage, ox: u32, oy: u32, bg: u8) -> GrayImage {
        let mut img = GrayImage::from_pixel(w, h, Luma([bg]));
        for y in 0..needle.height() {
            for x in 0..needle.width() {
                img.put_pixel(ox + x, oy + y, Luma([needle.get_pixel(x, y)[0]]));
            }
        }
        img
    }

    fn write_template(
        dir: &std::path::Path,
        filename: &str,
        needle: &GrayImage,
    ) -> std::path::PathBuf {
        let p = dir.join(filename);
        needle.save(&p).expect("save png");
        p
    }

    fn task(
        name: &str,
        template: std::path::PathBuf,
        algorithm: Algorithm,
        roi: Option<[u32; 4]>,
    ) -> TaskDef {
        TaskDef {
            name: name.into(),
            state: name.into(),
            algorithm,
            template,
            roi,
            threshold: 0.99,
            base: None,
            action: None,
            next: None,
        }
    }

    #[test]
    fn diagnose_task_reports_scaled_roi_and_high_conf_for_embedded_needle() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let needle = gradient_needle(40, 40);
        // screenshot 1258x708 → roi/needle スケールが恒等 (sx=sy=1.0)
        let screenshot = DynamicImage::ImageLuma8(embed(1258, 708, &needle, 600, 340, 128));
        let tpl = write_template(tmp.path(), "n1.png", &needle);
        let t = task("T", tpl, Algorithm::Ccoeff, Some([520, 320, 240, 80]));

        let e = diagnose_task(&t, &screenshot);

        assert_eq!(e.task, "T");
        assert_eq!(e.algorithm, Algorithm::Ccoeff);
        assert!((e.threshold - 0.99).abs() < 1e-6);
        assert!((e.scale_factors.0 - 1.0).abs() < 1e-6);
        assert!((e.scale_factors.1 - 1.0).abs() < 1e-6);
        assert_eq!(e.scaled_roi, Some(ScreenRegion::new(520, 320, 240, 80)));
        assert_eq!(e.needle_scaled_size, Some((40, 40)));
        assert!(e.error.is_none());
        let conf = e.best_confidence.expect("embedded needle must have conf");
        assert!(conf > 0.9, "embedded needle conf={conf}");
        let region = e.best_region.expect("region");
        assert!((region.x as i32 - 600).abs() <= 5);
        assert!((region.y as i32 - 340).abs() <= 5);
    }

    #[test]
    fn diagnose_task_with_1280x720_reports_scale_factors_and_scaled_roi() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let needle = gradient_needle(40, 40);
        let screenshot = DynamicImage::ImageLuma8(embed(1280, 720, &needle, 610, 345, 128));
        let tpl = write_template(tmp.path(), "n2.png", &needle);
        let t = task("T2", tpl, Algorithm::Sse, Some([520, 320, 240, 80]));

        let e = diagnose_task(&t, &screenshot);

        // sx = 1280/1258, sy = 720/708
        assert!((e.scale_factors.0 - 1280.0 / 1258.0).abs() < 1e-6);
        assert!((e.scale_factors.1 - 720.0 / 708.0).abs() < 1e-6);
        // ROI: 520*1280/1258=529.1→529, 320*720/708=325.4→325, w 244, h 244
        assert_eq!(e.scaled_roi, Some(ScreenRegion::new(529, 325, 244, 81)));
        // needle 40x40 → 40*1280/1258=40.7→41 x 40*720/708=40.7→41
        assert_eq!(e.needle_scaled_size, Some((41, 41)));
        // needle は 41x41 へリスケールされるが screenshot 内の埋込は 40x40 のまま
        // （サブピクセル差）のため、conf の高さは保証しない。取得できること自体を検証。
        assert!(
            e.best_confidence.is_some(),
            "threshold=0.0 で必ず conf を取得"
        );
    }

    #[test]
    fn diagnose_task_needle_larger_than_screen_is_skipped() {
        let tmp = tempfile::tempdir().expect("tempdir");
        // needle がスケール後も画面より大きい → skipped。
        // needle_to_normalized は raw-1258 基準で縮小するため、2000x2000 でも
        // 100x100 画面には収まらない寸法を使う。
        let big = gradient_needle(2000, 2000);
        let screenshot =
            DynamicImage::ImageLuma8(embed(100, 100, &gradient_needle(10, 10), 0, 0, 200));
        let tpl = write_template(tmp.path(), "big.png", &big);
        let t = task("Big", tpl, Algorithm::Ccoeff, None);

        let e = diagnose_task(&t, &screenshot);

        assert!(e.best_confidence.is_none(), "skipped 表現は None");
        assert!(e.best_region.is_none());
        assert!(e.error.is_some(), "skip 理由を error に保持");
        assert_eq!(e.scaled_roi, None);
    }

    #[test]
    fn diagnose_task_missing_template_reports_error_not_panic() {
        let t = task(
            "Missing",
            std::path::PathBuf::from("nonexistent.png"),
            Algorithm::Sse,
            None,
        );
        let screenshot = DynamicImage::ImageLuma8(GrayImage::new(100, 100));
        let e = diagnose_task(&t, &screenshot);
        assert!(e.best_confidence.is_none());
        assert!(e.error.is_some());
        assert!(e.error.as_deref().unwrap().contains("template load failed"));
    }

    #[test]
    fn diagnose_all_sorts_descending_by_confidence() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let good = gradient_needle(30, 30);
        // good は埋め込む、bad は埋め込まない（低 conf）
        let screenshot = DynamicImage::ImageLuma8(embed(1258, 708, &good, 100, 100, 128));
        let tpl_good = write_template(tmp.path(), "good.png", &good);
        // bad は反転勾配（低 conf）にする
        let mut inverted = GrayImage::new(30, 30);
        for y in 0..30 {
            for x in 0..30 {
                inverted.put_pixel(x, y, Luma([255 - ((x + y) % 64) as u8]));
            }
        }
        let tpl_bad = write_template(tmp.path(), "bad.png", &inverted);

        let tasks = vec![
            task("Bad", tpl_bad, Algorithm::Ccoeff, None),
            task("Good", tpl_good, Algorithm::Ccoeff, None),
        ];
        let out = diagnose_all(&tasks, &screenshot);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].task, "Good");
        assert_eq!(out[1].task, "Bad");
        let g = out[0].best_confidence.expect("good conf");
        let b = out[1].best_confidence.expect("bad conf");
        assert!(g >= b, "降順: {g} >= {b}");
        assert!(g > 0.9);
    }

    #[test]
    fn diagnose_all_empty_tasks_returns_empty_vec() {
        let screenshot = DynamicImage::ImageLuma8(GrayImage::new(100, 100));
        assert!(diagnose_all(&[], &screenshot).is_empty());
    }

    #[test]
    fn diagnose_all_places_none_confidence_entries_last() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let good = gradient_needle(30, 30);
        let screenshot = DynamicImage::ImageLuma8(embed(1258, 708, &good, 100, 100, 128));
        let tpl_good = write_template(tmp.path(), "good.png", &good);
        let big = gradient_needle(2000, 2000);
        let tpl_big = write_template(tmp.path(), "big.png", &big);

        let tasks = vec![
            task("Big", tpl_big, Algorithm::Ccoeff, None),
            task("Good", tpl_good, Algorithm::Ccoeff, None),
        ];
        let out = diagnose_all(&tasks, &screenshot);
        assert_eq!(out[0].task, "Good");
        assert!(out[0].best_confidence.is_some());
        assert_eq!(out[1].task, "Big");
        assert!(out[1].best_confidence.is_none());
    }
}
