use std::path::PathBuf;

use image::DynamicImage;

use anaden_core::ScreenRegion;

use crate::pipeline::{
    Algorithm, TaskDef, cmp_confidence_desc, diagnose_def, sort_by_confidence_desc,
};

#[derive(Debug, Clone)]
pub struct DiagnoseEntry {
    pub task: String,
    pub template_path: PathBuf,
    pub algorithm: Algorithm,
    pub threshold: f32,
    pub best_confidence: Option<f32>,
    pub best_region: Option<ScreenRegion>,
    pub scaled_roi: Option<ScreenRegion>,
    pub scale_factors: (f32, f32),
    pub needle_scaled_size: Option<(u32, u32)>,
    pub error: Option<String>,
}

pub fn diagnose_task(task: &TaskDef, screenshot: &DynamicImage) -> DiagnoseEntry {
    diagnose_def(task, &task.template, screenshot)
}

pub fn diagnose_all(tasks: &[TaskDef], screenshot: &DynamicImage) -> Vec<DiagnoseEntry> {
    let mut entries: Vec<DiagnoseEntry> =
        tasks.iter().map(|t| diagnose_task(t, screenshot)).collect();
    sort_by_confidence_desc(&mut entries);
    entries
}

fn threshold_diff(entry: &DiagnoseEntry) -> Option<f32> {
    entry.best_confidence.map(|c| c - entry.threshold)
}

pub fn format_diagnose_report(
    title: &str,
    haystack_size: (u32, u32),
    entries: &[DiagnoseEntry],
) -> String {
    let mut sorted: Vec<&DiagnoseEntry> = entries.iter().collect();
    sorted.sort_by(|a, b| cmp_confidence_desc(a, b));

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

        assert!((e.scale_factors.0 - 1280.0 / 1258.0).abs() < 1e-6);
        assert!((e.scale_factors.1 - 720.0 / 708.0).abs() < 1e-6);
        assert_eq!(e.scaled_roi, Some(ScreenRegion::new(529, 325, 244, 81)));
        assert_eq!(e.needle_scaled_size, Some((41, 41)));
        assert!(
            e.best_confidence.is_some(),
            "threshold=0.0 で必ず conf を取得"
        );
    }

    #[test]
    fn diagnose_task_needle_larger_than_screen_is_skipped() {
        let tmp = tempfile::tempdir().expect("tempdir");
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
        let screenshot = DynamicImage::ImageLuma8(embed(1258, 708, &good, 100, 100, 128));
        let tpl_good = write_template(tmp.path(), "good.png", &good);
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
