//! data/results.json (Task 4 成果物) の契約テスト。
//!
//! - ファイルが存在し EvalInput として解析できること
//! - `aggregate()` が成功し、埋め込み `metrics` セクションと一致すること
//!   (成果物と集計ロジックのドリフト検出)
//! - `meta.evidence` の各 PR エントリが golden/consensus の PR 集合と整合すること

#![allow(clippy::unwrap_used)]
#![allow(clippy::panic)]
#![allow(clippy::expect_used)]

use review_gate_eval::{EvalInput, aggregate};

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
    // 集計がエラーなく通ること (golden 外 ID 参照などがない)
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
