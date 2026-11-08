#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use review_gate_eval::{DecisionMethod, EvalInput, Verdict, aggregate};

    const RAW: &str = include_str!("../data/input.json");

    fn load() -> EvalInput {
        serde_json::from_str(RAW).expect("input.json must parse as EvalInput")
    }

    #[test]
    fn golden_issues_for_pr82_exist() {
        let input = load();
        let g82: Vec<_> = input.golden_issues.iter().filter(|g| g.pr == 82).collect();
        assert_eq!(g82.len(), 3, "PR 82 must have exactly 3 golden issues");
        for g in &g82 {
            assert!(!g.description.trim().is_empty());
        }
    }

    #[test]
    fn findings_for_pr82_match_all_golden() {
        let input = load();
        let golden82: std::collections::HashSet<String> = input
            .golden_issues
            .iter()
            .filter(|g| g.pr == 82)
            .map(|g| g.id.clone())
            .collect();
        let f82: Vec<_> = input.findings.iter().filter(|f| f.pr == 82).collect();
        assert!(!f82.is_empty(), "PR 82 must have findings");
        for f in &f82 {
            let m = f
                .matched_golden
                .as_ref()
                .unwrap_or_else(|| panic!("PR82 finding must be matched, got None"));
            assert!(
                golden82.contains(m),
                "finding references non-PR82 golden {m}"
            );
        }
        let matched: std::collections::HashSet<&str> = f82
            .iter()
            .filter_map(|f| f.matched_golden.as_deref())
            .collect();
        for id in &golden82 {
            assert!(matched.contains(id.as_str()), "golden {id} not detected");
        }
    }

    #[test]
    fn consensus_for_pr82_has_split_info_with_decision_methods() {
        let input = load();
        let rec = input
            .consensus
            .iter()
            .find(|c| c.pr == 82)
            .expect("PR 82 consensus record must exist");
        assert_eq!(rec.verdict, Verdict::Go);
        assert!(rec.merged, "PR 82 is merged");
        assert!(rec.post_merge_issue_ids.is_empty());
        let split = rec
            .split_info
            .as_ref()
            .expect("PR 82 record must exercise the new split_info schema");
        assert!(
            matches!(
                split.decision_method(),
                DecisionMethod::UnanimousGo | DecisionMethod::Majority
            ),
            "effective decision must be GO-sided"
        );
        assert_eq!(split.effective_verdict(), Verdict::Go);
        assert!(
            !split.judgments.is_empty() && split.judgments.iter().all(|j| j.confidence.is_some()),
            "PR82 schema requires confidence on judgments"
        );
    }

    #[test]
    fn aggregate_includes_pr82_with_full_recall() {
        let input = load();
        let metrics = aggregate(&input).expect("aggregate must succeed");
        let pr82 = metrics
            .recall_per_pr
            .iter()
            .find(|r| r.pr == 82)
            .expect("PR 82 in recall_per_pr");
        assert_eq!(pr82.total_golden, 3);
        assert_eq!(pr82.detected_golden, 3);
        assert!((pr82.recall() - 1.0).abs() < 1e-9);
        assert!(metrics.consensus.tn >= 1);
    }
}
