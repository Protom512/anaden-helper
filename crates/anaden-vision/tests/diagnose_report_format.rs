//! format_diagnose_report の整形仕様テスト（Issue #71 Task 2）。
//!
//! diagnose.rs の 500 行制限を守るため、レポート整形系のテストはここに分離した。

#![allow(clippy::unwrap_used)]
#![allow(clippy::panic)]
#![allow(clippy::expect_used)]

use anaden_core::ScreenRegion;
use anaden_vision::{Algorithm, DiagnoseEntry, format_diagnose_report};

mod report_tests {
    use super::*;
    fn report_entry(
        task: &str,
        conf: Option<f32>,
        scaled_roi: Option<ScreenRegion>,
    ) -> DiagnoseEntry {
        DiagnoseEntry {
            task: task.to_string(),
            template_path: std::path::PathBuf::from("t.png"),
            algorithm: Algorithm::Ccoeff,
            threshold: 0.95,
            best_confidence: conf,
            best_region: conf.map(|_| ScreenRegion::new(10, 20, 30, 40)),
            scaled_roi,
            scale_factors: (1.017, 1.017),
            needle_scaled_size: Some((40, 40)),
            error: None,
        }
    }

    #[test]
    fn format_report_contains_header_lines() {
        let report = format_diagnose_report("top.png", (1280, 720), &[]);
        assert!(report.contains("# NoMatch Diagnose Report"));
        assert!(report.contains("title: top.png"));
        assert!(report.contains("haystack: 1280x720"));
        assert!(report.contains("templates: 0"));
    }

    #[test]
    fn format_report_contains_template_rows_roi_and_scale() {
        let entries = vec![
            report_entry("Low", Some(0.10), Some(ScreenRegion::new(0, 0, 200, 100))),
            report_entry("High", Some(0.99), None),
        ];
        let report = format_diagnose_report("top.png", (1280, 720), &entries);
        assert!(report.contains("Low"));
        assert!(report.contains("High"));
        // ROI 内訳（scaled_roi の座標・寸法）
        assert!(report.contains("(0,0) 200x100"));
        // ROI なし = full
        assert!(report.contains("full"));
        // スケール倍率内訳
        assert!(report.contains("(1.017,1.017)"));
        // needle 寸法
        assert!(report.contains("40x40"));
        // 領域
        assert!(report.contains("(10,20) 30x40"));
    }

    #[test]
    fn format_report_rows_are_conf_descending() {
        let entries = vec![
            report_entry("Mid", Some(0.50), None),
            report_entry("Low", Some(0.10), None),
            report_entry("High", Some(0.99), None),
        ];
        let report = format_diagnose_report("top.png", (1280, 720), &entries);
        let pos = |n: &str| report.find(n).expect("task present");
        assert!(pos("High") < pos("Mid") && pos("Mid") < pos("Low"));
        assert!(report.contains("| 1 | High |"));
        assert!(report.contains("| 3 | Low |"));
    }

    #[test]
    fn format_report_threshold_diff_column() {
        let entries = vec![report_entry("T", Some(0.93), None)];
        let report = format_diagnose_report("top.png", (1280, 720), &entries);
        assert!(report.contains("0.9300"), "conf");
        assert!(report.contains("0.9500"), "threshold");
        assert!(report.contains("-0.0200"), "diff = 0.93 - 0.95");
    }

    #[test]
    fn format_report_skipped_entry_uses_dash_and_note() {
        let mut e = report_entry("Big", None, None);
        e.error = Some("needle larger than haystack (or empty scan space)".to_string());
        e.needle_scaled_size = Some((2000, 2000));
        let report = format_diagnose_report("top.png", (1280, 720), &[e]);
        assert!(report.contains("| 1 | Big | - |"));
        assert!(report.contains("needle larger than haystack"));
    }

    #[test]
    fn format_report_sorts_none_confidence_last() {
        let entries = vec![
            report_entry("Skipped", None, None),
            report_entry("Matched", Some(0.1), None),
        ];
        let report = format_diagnose_report("top.png", (1280, 720), &entries);
        assert!(report.contains("| 1 | Matched |"));
        assert!(report.contains("| 2 | Skipped |"));
    }
}
