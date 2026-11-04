use std::path::Path;

use image::DynamicImage;

use anaden_vision::{TaskDef, diagnose_all, format_diagnose_report};

pub fn diag_report_dir() -> Option<std::path::PathBuf> {
    if let Some(dir) = std::env::var_os("ANADEN_SNAPSHOT_DIR") {
        return Some(std::path::PathBuf::from(dir));
    }
    if let Some(v) = std::env::var_os("ANADEN_DIAG_REPORT") {
        if v == "1" || v == "true" {
            return Some(std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")));
        }
        return Some(std::path::PathBuf::from(v));
    }
    None
}

pub fn save_diagnose_report(
    dir: &Path,
    task: &str,
    counter: u64,
    frame: &DynamicImage,
    tasks: &[TaskDef],
    dump_frame: bool,
) {
    if let Err(e) = std::fs::create_dir_all(dir) {
        tracing::warn!("diag report create_dir failed ({}): {e}", dir.display());
        return;
    }
    if dump_frame {
        let png_path = dir.join(format!("diag_{task}_{counter}.png"));
        match frame.save(&png_path) {
            Ok(()) => tracing::debug!("diag frame dumped: {}", png_path.display()),
            Err(e) => tracing::warn!("diag frame dump failed ({}): {e}", png_path.display()),
        }
    }

    let entries = diagnose_all(tasks, frame);
    let report = format_diagnose_report(
        &format!("{task}_{counter}"),
        (frame.width(), frame.height()),
        &entries,
    );
    let md_path = dir.join(format!("diag_{task}_{counter}.md"));
    match std::fs::write(&md_path, report) {
        Ok(()) => tracing::debug!("diag report saved: {}", md_path.display()),
        Err(e) => tracing::warn!("diag report write failed ({}): {e}", md_path.display()),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use anaden_vision::Algorithm;
    use image::{GrayImage, Luma};

    fn task_def(name: &str, template: std::path::PathBuf) -> TaskDef {
        TaskDef {
            name: name.into(),
            state: name.into(),
            algorithm: Algorithm::Ccoeff,
            template,
            roi: None,
            threshold: 0.9,
            base: None,
            action: None,
            next: None,
        }
    }

    #[test]
    fn save_diagnose_report_writes_png_and_md_pair() {
        let tmp = tempfile::tempdir().expect("tempdir");

        let mut needle = GrayImage::new(24, 24);
        for y in 0..24 {
            for x in 0..24 {
                needle.put_pixel(x, y, Luma([((x + y) % 64) as u8]));
            }
        }
        let tpl = tmp.path().join("n.png");
        needle.save(&tpl).expect("save tpl");

        let mut frame = GrayImage::from_pixel(1258, 708, Luma([128]));
        for y in 0..24 {
            for x in 0..24 {
                frame.put_pixel(600 + x, 300 + y, *needle.get_pixel(x, y));
            }
        }
        let frame = DynamicImage::ImageLuma8(frame);
        let tasks = vec![task_def("T", tpl)];

        save_diagnose_report(tmp.path(), "Title", 1, &frame, &tasks, true);

        let png = tmp.path().join("diag_Title_1.png");
        let md = tmp.path().join("diag_Title_1.md");
        assert!(png.exists(), "frame dump must exist: {}", png.display());
        assert!(md.exists(), "report must exist: {}", md.display());
        assert!(!md.as_os_str().is_empty(), "report path must be non-empty");

        let body = std::fs::read_to_string(&md).expect("read md");
        assert!(body.contains("# NoMatch Diagnose Report"));
        assert!(body.contains("T"), "report must contain task row");
    }

    #[test]
    fn save_diagnose_report_skips_broken_templates_without_panic() {
        let tmp = tempfile::tempdir().expect("tempdir");

        let empty = tmp.path().join("empty.png");
        std::fs::write(&empty, b"").expect("write empty");

        let frame = DynamicImage::ImageLuma8(GrayImage::from_pixel(1258, 708, Luma([100])));
        let tasks = vec![task_def("Broken", empty)];

        save_diagnose_report(tmp.path(), "Title", 7, &frame, &tasks, true);

        let md = std::fs::read_to_string(tmp.path().join("diag_Title_7.md")).expect("read md");
        assert!(md.contains("Broken"), "skipped row must exist");
        assert!(
            md.contains("template load failed"),
            "skip reason must be reported"
        );
    }

    #[test]
    fn save_diagnose_report_without_frame_dump_skips_png() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let empty = tmp.path().join("empty.png");
        std::fs::write(&empty, b"").expect("write empty");
        let frame = DynamicImage::ImageLuma8(GrayImage::from_pixel(1258, 708, Luma([100])));
        let tasks = vec![task_def("T", empty)];

        save_diagnose_report(tmp.path(), "Title", 2, &frame, &tasks, false);

        assert!(
            tmp.path().join("diag_Title_2.md").exists(),
            "report md must exist"
        );
        assert!(
            !tmp.path().join("diag_Title_2.png").exists(),
            "frame dump must be skipped when dump_frame=false"
        );
    }

    #[test]
    fn diag_report_dir_none_when_env_unset() {
        unsafe {
            std::env::remove_var("ANADEN_SNAPSHOT_DIR");
            std::env::remove_var("ANADEN_DIAG_REPORT");
        }
        assert!(diag_report_dir().is_none());
    }

    #[test]
    fn diag_report_dir_cwd_when_diag_report_enabled() {
        unsafe {
            std::env::remove_var("ANADEN_SNAPSHOT_DIR");
            std::env::set_var("ANADEN_DIAG_REPORT", "1");
        }
        struct EnvUnset;
        impl Drop for EnvUnset {
            fn drop(&mut self) {
                unsafe {
                    std::env::remove_var("ANADEN_DIAG_REPORT");
                }
            }
        }
        let _unset = EnvUnset;
        assert!(diag_report_dir().is_some());
    }

    #[test]
    fn diag_report_dir_prefers_snapshot_dir() {
        let tmp = tempfile::tempdir().expect("tempdir");
        unsafe {
            std::env::set_var("ANADEN_SNAPSHOT_DIR", tmp.path());
        }
        struct EnvUnset;
        impl Drop for EnvUnset {
            fn drop(&mut self) {
                unsafe {
                    std::env::remove_var("ANADEN_SNAPSHOT_DIR");
                }
            }
        }
        let _unset = EnvUnset;
        assert_eq!(diag_report_dir(), Some(tmp.path().to_path_buf()));
    }
}
