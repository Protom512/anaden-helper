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
                confidence: None,
                command_results: None,
            },
            Finding {
                pr: 58,
                reviewer: "functional".into(),
                matched_golden: Some("g58-1".into()),
                confidence: None,
                command_results: None,
            },
            Finding {
                pr: 58,
                reviewer: "maintainability".into(),
                matched_golden: None,
                confidence: None,
                command_results: None,
            },
            Finding {
                pr: 58,
                reviewer: "maintainability".into(),
                matched_golden: None,
                confidence: None,
                command_results: None,
            },
            // PR62: golden g62-1 を検出せず、偽陽性2件
            Finding {
                pr: 62,
                reviewer: "architecture".into(),
                matched_golden: None,
                confidence: None,
                command_results: None,
            },
            Finding {
                pr: 62,
                reviewer: "functional".into(),
                matched_golden: None,
                confidence: None,
                command_results: None,
            },
            // PR72: 両golden検出、偽陽性0
            Finding {
                pr: 72,
                reviewer: "architecture".into(),
                matched_golden: Some("g72-1".into()),
                confidence: None,
                command_results: None,
            },
            Finding {
                pr: 72,
                reviewer: "functional".into(),
                matched_golden: Some("g72-2".into()),
                confidence: None,
                command_results: None,
            },
        ];
        let consensus = vec![
            // NoGo + マージされた + 事後問題なし => 判定は保守的すぎ (FP)
            ConsensusRecord {
                pr: 58,
                verdict: Verdict::NoGo,
                merged: true,
                post_merge_issue_ids: vec![],
                split_info: None,
            },
            // Go + マージ + 事後問題なし => 正しいTN
            ConsensusRecord {
                pr: 62,
                verdict: Verdict::Go,
                merged: true,
                post_merge_issue_ids: vec![],
                split_info: None,
            },
            // Go + マージ + 事後問題1件 => 見逃し (FN)
            ConsensusRecord {
                pr: 72,
                verdict: Verdict::Go,
                merged: true,
                post_merge_issue_ids: vec!["post-72-1".into()],
                split_info: None,
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
            confidence: None,
            command_results: None,
        });
        assert!(aggregate(&f).is_err()); // 正解リスト外の match は集計エラー
    }

    // --- #80 新スキーマ: confidence / command_results ---

    #[test]
    fn finding_parses_confidence_and_command_results() {
        let js = r#"{
            "pr": 58,
            "reviewer": "architecture",
            "matched_golden": null,
            "confidence": 0.3,
            "command_results": {"clippy": "PASS", "nextest": "FAIL"}
        }"#;
        let f: Finding = serde_json::from_str(js).unwrap();
        assert!((f.confidence.unwrap() - 0.3).abs() < 1e-9);
        let cmd = f.command_results.unwrap();
        assert_eq!(cmd.clippy, Some(CommandStatus::Pass));
        assert_eq!(cmd.nextest, Some(CommandStatus::Fail));
        assert!(cmd.any_fail());
    }

    #[test]
    fn legacy_finding_without_new_fields_defaults_to_none() {
        // 旧 results.json の findings には confidence / command_results がない
        let js = r#"{"pr": 58, "reviewer": "functional", "matched_golden": null}"#;
        let f: Finding = serde_json::from_str(js).unwrap();
        assert!(f.confidence.is_none());
        assert!(f.command_results.is_none()); // 欠損は fail 判定に寄与しない
    }

    #[test]
    fn command_missing_is_distinct_from_fail() {
        let missing = CommandResults {
            clippy: None,
            nextest: None,
        };
        assert!(!missing.any_fail(), "欠損 (None) は強制 NO-GO にしない");
        let pass = CommandResults {
            clippy: Some(CommandStatus::Pass),
            nextest: Some(CommandStatus::Pass),
        };
        assert!(!pass.any_fail());
        let fail = CommandResults {
            clippy: Some(CommandStatus::Pass),
            nextest: Some(CommandStatus::Fail),
        };
        assert!(fail.any_fail());
    }

    // --- #80: split_info による再集計で偽ブロック (fp) が構造的に低下 ---

    fn judgment(reviewer: &str, verdict: Verdict, confidence: Option<f64>) -> ReviewerJudgment {
        ReviewerJudgment {
            reviewer: reviewer.into(),
            verdict,
            confidence,
            has_critical: false,
        }
    }

    /// PR58 と同一の帰結 (マージ済み・事後問題なし) で、レビュアー判定が
    /// 「2名 GO + 1名 低confidence NO-GO」だった場合の split_info。
    /// 旧 AND コンセンサスなら NO-GO (fp) だが、majority なら GO (tn) になる。
    fn pr58_majority_split() -> SplitInfo {
        SplitInfo {
            judgments: vec![
                judgment("architecture", Verdict::Go, Some(0.9)),
                judgment("functional", Verdict::Go, Some(0.85)),
                judgment("maintainability", Verdict::NoGo, Some(0.3)), // 低confidence
            ],
            veto_activated: false,
            command_fail_forced_nogo: false,
        }
    }

    #[test]
    fn aggregate_prefers_split_info_effective_verdict() {
        let mut f = fixture();
        let pr58 = f
            .consensus
            .iter_mut()
            .find(|r| r.pr == 58)
            .expect("pr58 record");
        pr58.split_info = Some(pr58_majority_split());
        let m = aggregate(&f).unwrap();
        // verdict フィールドは旧 AND ロジックの NO-GO のままだが、
        // effective_verdict (majority GO) が混同行列に反映される → fp が低下
        assert_eq!(m.consensus.fp, 0);
        assert_eq!(m.consensus.tn, 2); // PR62 + PR58(majority GO)
    }

    #[test]
    fn aggregate_without_split_info_keeps_legacy_verdict() {
        // split_info 欠損 (旧データ) は従来どおり verdict フィールドで集計
        let m = aggregate(&fixture()).unwrap();
        assert_eq!(m.consensus.fp, 1);
        assert_eq!(m.consensus.tn, 1);
    }

    #[test]
    fn command_fail_split_info_forces_nogo_in_aggregation() {
        // 全員 GO でも command_fail_forced_nogo 付き split_info は NO-GO として集計
        let mut f = fixture();
        let pr62 = f
            .consensus
            .iter_mut()
            .find(|r| r.pr == 62)
            .expect("pr62 record");
        pr62.verdict = Verdict::Go;
        pr62.post_merge_issue_ids = vec!["cmd-fail".into()]; // 問題あり → NoGo なら TP
        pr62.split_info = Some(SplitInfo {
            judgments: vec![
                judgment("architecture", Verdict::Go, Some(0.9)),
                judgment("functional", Verdict::Go, Some(0.9)),
                judgment("maintainability", Verdict::Go, Some(0.9)),
            ],
            veto_activated: false,
            command_fail_forced_nogo: true,
        });
        let m = aggregate(&f).unwrap();
        assert_eq!(m.consensus.tp, 1); // 強制 NO-GO & 実際に問題あり
        assert_eq!(m.consensus.tn, 0);
    }

    #[test]
    fn critical_veto_split_info_forces_nogo_even_with_majority_go() {
        let mut f = fixture();
        let pr62 = f
            .consensus
            .iter_mut()
            .find(|r| r.pr == 62)
            .expect("pr62 record");
        pr62.post_merge_issue_ids = vec!["critical-bug".into()];
        let mut maint = judgment("maintainability", Verdict::Go, Some(0.9));
        maint.has_critical = true;
        pr62.split_info = Some(SplitInfo {
            judgments: vec![
                judgment("architecture", Verdict::Go, Some(0.9)),
                judgment("functional", Verdict::Go, Some(0.9)),
                maint,
            ],
            veto_activated: true,
            command_fail_forced_nogo: false,
        });
        let m = aggregate(&f).unwrap();
        assert_eq!(m.consensus.tp, 1); // veto NO-GO & 問題あり
    }

    /// 偽ブロック率の構造比較: reviewer 1名あたりの偽 NO-GO 確率 p のとき
    /// 3 reviewer AND は 1-(1-p)^3 に増幅、majority (2/3 GO で GO) は
    /// 3p^2(1-p)+p^3 に低下する (#80 主張の固定)。
    #[test]
    fn majority_consensus_structurally_reduces_false_block_rate() {
        for p in [0.05_f64, 0.1, 0.2, 0.3, 0.5] {
            let and_fp = 1.0 - (1.0 - p).powi(3);
            let majority_fp = 3.0 * p * p * (1.0 - p) + p * p * p;
            assert!(
                majority_fp < and_fp,
                "p={p}: majority({majority_fp:.4}) must be < AND({and_fp:.4})"
            );
        }
        // p=0.1 の具体値: AND 27.1% vs majority 2.8% (README 記載値と整合)
        let p = 0.1_f64;
        let and_fp = 1.0 - (1.0 - p).powi(3);
        let majority_fp = 3.0 * p * p * 0.9 + p * p * p;
        assert!((and_fp - 0.271).abs() < 1e-3);
        assert!((majority_fp - 0.028).abs() < 1e-3);
    }

    #[test]
    fn consensus_record_serializes_split_info() {
        let mut f = fixture();
        let pr58 = f.consensus.iter_mut().find(|r| r.pr == 58).unwrap();
        pr58.split_info = Some(pr58_majority_split());
        let js = serde_json::to_string(&f.consensus).unwrap();
        assert!(js.contains("\"split_info\""));
        assert!(js.contains("judgments"));
    }
}
