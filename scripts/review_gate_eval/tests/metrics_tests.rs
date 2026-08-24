#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use review_gate_eval::*;

    fn fixture() -> EvalInput {
        let golden = vec![
            GoldenIssue {
                pr: 58,
                id: "g58-1".into(),
                description: "ROIが実測と乖離".into(),
            },
            GoldenIssue {
                pr: 62,
                id: "g62-1".into(),
                description: "テンプレ再生成漏れ".into(),
            },
            GoldenIssue {
                pr: 72,
                id: "g72-1".into(),
                description: "dump時のパス扱い".into(),
            },
            GoldenIssue {
                pr: 72,
                id: "g72-2".into(),
                description: "conf報告の閾値未明示".into(),
            },
        ];
        let findings = vec![
            // PR58: golden g58-1 を検出 (match) + 検出 = recall 1.0, FP 0.5
            Finding {
                pr: 58,
                reviewer: "architecture".into(),
                matched_golden: Some("g58-1".into()),
            },
            Finding {
                pr: 58,
                reviewer: "functional".into(),
                matched_golden: Some("g58-1".into()),
            },
            Finding {
                pr: 58,
                reviewer: "maintainability".into(),
                matched_golden: None,
            },
            Finding {
                pr: 58,
                reviewer: "maintainability".into(),
                matched_golden: None,
            },
            // PR62: golden g62-1 を検出せず、偽陽性2件
            Finding {
                pr: 62,
                reviewer: "architecture".into(),
                matched_golden: None,
            },
            Finding {
                pr: 62,
                reviewer: "functional".into(),
                matched_golden: None,
            },
            // PR72: 両golden検出、偽陽性0
            Finding {
                pr: 72,
                reviewer: "architecture".into(),
                matched_golden: Some("g72-1".into()),
            },
            Finding {
                pr: 72,
                reviewer: "functional".into(),
                matched_golden: Some("g72-2".into()),
            },
        ];
        let consensus = vec![
            // NoGo + マージされた + 事後問題なし => 判定は保守的すぎ (FP)
            ConsensusRecord {
                pr: 58,
                verdict: Verdict::NoGo,
                merged: true,
                post_merge_issue_ids: vec![],
            },
            // Go + マージ + 事後問題なし => 正しいTN
            ConsensusRecord {
                pr: 62,
                verdict: Verdict::Go,
                merged: true,
                post_merge_issue_ids: vec![],
            },
            // Go + マージ + 事後問題1件 => 見逃し (FN)
            ConsensusRecord {
                pr: 72,
                verdict: Verdict::Go,
                merged: true,
                post_merge_issue_ids: vec!["post-72-1".into()],
            },
        ];
        EvalInput {
            golden_issues: golden,
            findings,
            consensus,
        }
    }

    // --- recall ---

    #[test]
    fn recall_overall_counts_unique_golden_detected() {
        let m = aggregate(&fixture()).unwrap();
        assert_eq!(m.recall.total_golden, 4);
        assert_eq!(m.recall.detected_golden, 3); // g58-1, g72-1, g72-2
        assert!((m.recall.recall() - 0.75).abs() < 1e-9);
    }

    #[test]
    fn recall_per_pr_reports_n() {
        let m = aggregate(&fixture()).unwrap();
        let pr62 = m.recall_per_pr.iter().find(|r| r.pr == 62).unwrap();
        assert_eq!(pr62.total_golden, 1);
        assert_eq!(pr62.detected_golden, 0);
        assert_eq!(pr62.recall(), 0.0);
    }

    #[test]
    fn recall_empty_golden_is_none_not_zero() {
        let mut f = fixture();
        f.golden_issues.clear();
        let m = aggregate(&f).unwrap();
        assert_eq!(m.recall.total_golden, 0);
        assert!(m.recall.recall_opt().is_none()); // 分母0は点推定しない
    }

    // --- false positive rate ---

    #[test]
    fn fp_rate_overall_with_n() {
        let m = aggregate(&fixture()).unwrap();
        assert_eq!(m.fp_rate.total_findings, 8);
        assert_eq!(m.fp_rate.false_positives, 5); // 8 - 3 matched
        assert!((m.fp_rate.rate() - 5.0 / 8.0).abs() < 1e-9);
        assert!(m.fp_rate.n_included_in_report());
    }

    #[test]
    fn fp_rate_zero_findings_is_none() {
        let mut f = fixture();
        f.findings.clear();
        let m = aggregate(&f).unwrap();
        assert_eq!(m.fp_rate.total_findings, 0);
        assert!(m.fp_rate.rate_opt().is_none());
    }

    #[test]
    fn fp_rate_wilson_ci_brackets_point_estimate() {
        let m = aggregate(&fixture()).unwrap();
        let Some((lo, hi)) = m.fp_rate.wilson_95().unwrap() else {
            panic!("no CI")
        };
        let p = m.fp_rate.rate();
        assert!(lo < p && p < hi, "lo={lo} p={p} hi={hi}");
        assert!(lo >= 0.0 && hi <= 1.0);
    }

    #[test]
    fn wilson_ci_extremes() {
        // 0/10 と 10/10 でも有限区間を返す
        let r = FpRate {
            total_findings: 10,
            false_positives: 0,
        };
        let Some((lo, hi)) = r.wilson_95().unwrap() else {
            panic!("no CI")
        };
        assert_eq!(lo, 0.0);
        assert!(hi > 0.0 && hi < 0.5);
        let r = FpRate {
            total_findings: 10,
            false_positives: 10,
        };
        let Some((lo, hi)) = r.wilson_95().unwrap() else {
            panic!("no CI")
        };
        assert!(lo < 1.0 && lo > 0.5);
        assert_eq!(hi, 1.0);
    }

    #[test]
    fn fp_rate_false_positives_cannot_exceed_total() {
        let r = FpRate {
            total_findings: 3,
            false_positives: 4,
        };
        assert!(r.validate().is_err());
    }

    // --- consensus validity ---

    #[test]
    fn consensus_confusion_matrix() {
        let m = aggregate(&fixture()).unwrap();
        let c = &m.consensus;
        // actual_problem = post_merge issues あり
        assert_eq!(c.tp, 0); // NoGo & 問題あり
        assert_eq!(c.fp, 1); // NoGo & 問題なし (PR58: 保守的すぎ)
        assert_eq!(c.tn, 1); // Go & 問題なし (PR62)
        assert_eq!(c.false_negatives, 1); // Go & 問題あり (PR72: 見逃し)
        assert!((c.accuracy() - 1.0 / 3.0).abs() < 1e-9); // (tp+tn)/total = 1/3
    }

    #[test]
    fn consensus_nogo_but_merged_flagged() {
        let m = aggregate(&fixture()).unwrap();
        let flagged = m
            .consensus
            .nogo_merged_prs
            .iter()
            .map(|r| r.pr)
            .collect::<Vec<_>>();
        assert_eq!(flagged, vec![58]); // NoGo なのにマージされた PR を列挙
    }

    #[test]
    fn consensus_empty_records_accuracy_is_none() {
        let mut f = fixture();
        f.consensus.clear();
        let m = aggregate(&f).unwrap();
        assert!(m.consensus.accuracy_opt().is_none());
    }

    // --- serialization for report ---

    #[test]
    fn metrics_serialize_to_json() {
        let m = aggregate(&fixture()).unwrap();
        let js = serde_json::to_string(&m).unwrap();
        assert!(js.contains("\"recall\""));
        assert!(js.contains("\"fp_rate\""));
        assert!(js.contains("\"consensus\""));
        assert!(js.contains("\"n\":8")); // N がレポートに含まれる
    }

    // --- input parsing ---

    #[test]
    fn parse_input_from_json() {
        let js = r#"{
            "golden_issues": [
                {"pr": 58, "id": "g1", "description": "x"}
            ],
            "findings": [
                {"pr": 58, "reviewer": "architecture", "matched_golden": "g1"},
                {"pr": 58, "reviewer": "functional", "matched_golden": null}
            ],
            "consensus": [
                {"pr": 58, "verdict": "GO", "merged": true, "post_merge_issue_ids": []}
            ]
        }"#;
        let input: EvalInput = serde_json::from_str(js).unwrap();
        let m = aggregate(&input).unwrap();
        assert!((m.recall.recall() - 1.0).abs() < 1e-9);
        assert!((m.fp_rate.rate() - 0.5).abs() < 1e-9);
    }

    #[test]
    fn matched_golden_id_not_in_dataset_is_error() {
        let mut f = fixture();
        f.findings.push(Finding {
            pr: 58,
            reviewer: "architecture".into(),
            matched_golden: Some("g-nonexistent".into()),
        });
        assert!(aggregate(&f).is_err()); // 正解リスト外の match は集計エラー
    }
}
