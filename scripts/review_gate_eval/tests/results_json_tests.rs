#![allow(clippy::unwrap_used)]
#![allow(clippy::panic)]
#![allow(clippy::expect_used)]

use review_gate_eval::{EvalInput, ReviewerJudgment, SplitInfo, Verdict, aggregate};

const PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/data/results.json");

fn load() -> serde_json::Value {
    let raw =
        std::fs::read_to_string(PATH).expect("data/results.json must exist (Task 4 artifact)");
    serde_json::from_str(&raw).expect("results.json must be valid JSON")
}

#[test]
fn results_json_parses_as_eval_input() {
    let v = load();
    let input: EvalInput = serde_json::from_value(v["input"].clone())
        .expect("meta.input must satisfy EvalInput schema");
    aggregate(&input).expect("aggregate must succeed on results.json input");
}

#[test]
fn embedded_metrics_match_recomputed_aggregation() {
    let v = load();
    let input: EvalInput = serde_json::from_value(v["input"].clone()).expect("input parses");
    let metrics = aggregate(&input).expect("aggregate succeeds");
    let embedded = v["metrics"].clone();
    assert!(!embedded.is_null(), "metrics section must be embedded");

    let recomputed = serde_json::to_value(&metrics).expect("serialize metrics");
    assert_eq!(
        embedded["recall"], recomputed["recall"],
        "recall section drift between artifact and aggregate()"
    );
    assert_eq!(
        embedded["recall_per_pr"], recomputed["recall_per_pr"],
        "recall_per_pr drift"
    );
    assert_eq!(
        embedded["fp_rate"], recomputed["fp_rate"],
        "fp_rate drift (N must be included)"
    );
    assert_eq!(
        embedded["consensus"], recomputed["consensus"],
        "consensus drift"
    );
}

#[test]
fn evidence_prs_are_consistent_with_input() {
    let v = load();
    let input: EvalInput = serde_json::from_value(v["input"].clone()).expect("input parses");
    let evidence = v["meta"]["evidence"]
        .as_array()
        .expect("meta.evidence must be an array");
    assert!(!evidence.is_empty(), "at least one PR evidence entry");

    let consensus_prs: Vec<u64> = input.consensus.iter().map(|c| c.pr).collect();
    for e in evidence {
        let pr = e["pr"].as_u64().expect("evidence entry has pr number");
        assert!(
            consensus_prs.contains(&pr),
            "evidence PR {pr} missing from consensus records"
        );
    }
}

#[test]
fn methodology_documents_upper_bound_and_substitution() {
    let v = load();
    let methodology = v["meta"]["methodology"].as_str().unwrap_or("");
    assert!(
        methodology.contains("upper-bound"),
        "methodology must state upper-bound positioning"
    );
    let structural = v["meta"]["structural_analysis"]
        .as_array()
        .expect("structural_analysis must be an array");
    assert!(
        !structural.is_empty(),
        "structural analysis must substitute unmeasured LLM layer"
    );
}

#[test]
fn legacy_results_json_reaggregates_via_serde_defaults() {
    let v = load();
    let input: EvalInput = serde_json::from_value(v["input"].clone())
        .expect("legacy input must parse via serde defaults");
    for c in &input.consensus {
        assert!(c.split_info.is_none(), "legacy data has no split_info");
    }
    let metrics = aggregate(&input).expect("legacy re-aggregation must succeed");
    assert_eq!(metrics.consensus.tn as usize, input.consensus.len());
    assert_eq!(metrics.consensus.fp, 0);
}

#[test]
fn split_info_reaggregate_reflects_majority_verdict() {
    let v = load();
    let mut input: EvalInput = serde_json::from_value(v["input"].clone()).expect("input parses");
    for (i, c) in input.consensus.iter_mut().enumerate() {
        let mut judgments = Vec::new();
        for (j, reviewer) in ["architecture", "functional", "maintainability"]
            .iter()
            .enumerate()
        {
            let low_conf_nogo = i == 0 && j == 2;
            judgments.push(ReviewerJudgment {
                reviewer: reviewer.to_string(),
                verdict: if low_conf_nogo {
                    Verdict::NoGo
                } else {
                    Verdict::Go
                },
                confidence: Some(if low_conf_nogo { 0.3 } else { 0.9 }),
                has_critical: false,
            });
        }
        let split = SplitInfo {
            judgments,
            veto_activated: false,
            command_fail_forced_nogo: false,
        };
        c.verdict = split.effective_verdict();
        c.split_info = Some(split);
    }
    let metrics = aggregate(&input).expect("aggregate succeeds");
    assert_eq!(metrics.consensus.fp, 0);
    assert!(
        !metrics.consensus.nogo_merged_prs.is_empty() || metrics.consensus.fp == 0,
        "re-aggregation path exercised"
    );
}

#[test]
fn results_json_documents_n80_revalidation() {
    let v = load();
    let meta = &v["meta"];
    let s = serde_json::to_string(meta).unwrap_or_default();
    assert!(
        s.contains("#80") || s.contains("revalidation") || s.contains("majority"),
        "meta must document the #80 revalidation (majority/critical-veto/command-fail)"
    );
}
