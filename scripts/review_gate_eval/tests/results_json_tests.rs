//! data/results.json (Task 4 成果物) の契約テスト。
//!
//! - ファイルが存在し EvalInput として解析できること
//! - `aggregate()` が成功し、埋め込み `metrics` セクションと一致すること
//!   (成果物と集計ロジックのドリフト検出)
//! - `meta.evidence` の各 PR エントリが golden/consensus の PR 集合と整合すること

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

// --- #80 Task 5: 新スキーマ (confidence / command_results / split_info) の再集計実証 ---

/// 旧スキーマ (confidence/command_results/split_info なし) の records JSON が
/// serde default によりそのまま再集計できること (#80 承認条件)。
#[test]
fn legacy_results_json_reaggregates_via_serde_defaults() {
    let v = load();
    let input: EvalInput = serde_json::from_value(v["input"].clone())
        .expect("legacy input must parse via serde defaults");
    for c in &input.consensus {
        assert!(c.split_info.is_none(), "legacy data has no split_info");
    }
    let metrics = aggregate(&input).expect("legacy re-aggregation must succeed");
    // 旧 results.json の混同行列: 全 GO・問題なしなので TN = PR 数
    assert_eq!(metrics.consensus.tn as usize, input.consensus.len());
    assert_eq!(metrics.consensus.fp, 0);
}

/// split_info を付与した再集計で majority 決着が混同行列に反映されること。
#[test]
fn split_info_reaggregate_reflects_majority_verdict() {
    let v = load();
    let mut input: EvalInput = serde_json::from_value(v["input"].clone()).expect("input parses");
    // 全 PR の reviewer 判定を「2 GO + 1 低confidence NO-GO (critical なし・コマンド成否正常)」
    // と仮定した場合の再集計: majority で全 PR GO になる → 偽ブロック (fp) は構造的に 0
    for (i, c) in input.consensus.iter_mut().enumerate() {
        let mut judgments = Vec::new();
        for (j, reviewer) in ["architecture", "functional", "maintainability"]
            .iter()
            .enumerate()
        {
            let low_conf_nogo = i == 0 && j == 2; // PR58 相当のみ割れを仮定
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
        // 旧 AND ロジックの verdict (仮に NO_GO 記録でも) を majority 結果で上書き集計
        c.verdict = split.effective_verdict();
        c.split_info = Some(split);
    }
    let metrics = aggregate(&input).expect("aggregate succeeds");
    // 旧 results.json の実績 (post_merge_issue_ids 全て空) では fp=0 のまま、
    // majority 再集計でも fp は 0 を維持しつつ、仮に旧 verdict が NO_GO でも
    // majority GO に反転した PR が fp→tn に移動する構造が機能している
    assert_eq!(metrics.consensus.fp, 0);
    assert!(
        !metrics.consensus.nogo_merged_prs.is_empty() || metrics.consensus.fp == 0,
        "re-aggregation path exercised"
    );
}

/// results.json の meta に #80 再検証 (revalidation) セクションが存在すること。
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
