//! golden dataset (data/input.json) の構造検証テスト (Task 3)。
//!
//! README 記載の客観的抽出基準に基づいて機械的に検証できる範囲:
//! - JSON が EvalInput スキーマとしてパースできる
//! - golden id はグローバル一意
//! - 対象 PR はマージ済み PR (golden: #54/#55/#56/#58/#62/#72, consensus 追加: #64/#65/#75) のみ
//! - findings の matched_golden は存在する golden id のみ参照する
//! - consensus の PR は golden または findings に登場する PR を網羅する

#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use review_gate_eval::{EvalInput, Verdict};
    use std::collections::HashSet;

    const RAW: &str = include_str!("../data/input.json");

    fn load() -> EvalInput {
        serde_json::from_str(RAW).expect("input.json must parse as EvalInput")
    }

    #[test]
    fn parses_as_eval_input_schema() {
        let input = load();
        assert!(
            !input.golden_issues.is_empty(),
            "golden_issues must not be empty"
        );
    }

    #[test]
    fn golden_ids_are_globally_unique() {
        let input = load();
        let ids: HashSet<&str> = input.golden_issues.iter().map(|g| g.id.as_str()).collect();
        assert_eq!(ids.len(), input.golden_issues.len(), "duplicate golden ids");
    }

    #[test]
    fn golden_prs_are_within_known_merged_prs() {
        let input = load();
        let known: HashSet<u64> = [54, 55, 56, 58, 62, 72, 64, 65, 75].into_iter().collect();
        for g in &input.golden_issues {
            assert!(
                known.contains(&g.pr),
                "PR {} is not in the known merged PR set",
                g.pr
            );
        }
    }

    #[test]
    fn findings_reference_only_existing_golden_ids() {
        let input = load();
        let ids: HashSet<&str> = input.golden_issues.iter().map(|g| g.id.as_str()).collect();
        for f in &input.findings {
            if let Some(m) = &f.matched_golden {
                assert!(
                    ids.contains(m.as_str()),
                    "finding references unknown golden id {m}"
                );
            }
        }
    }

    #[test]
    fn consensus_verdicts_are_valid_and_prs_cover_dataset() {
        let input = load();
        let dataset_prs: HashSet<u64> = input
            .golden_issues
            .iter()
            .map(|g| g.pr)
            .chain(input.findings.iter().map(|f| f.pr))
            .collect();
        for c in &input.consensus {
            assert!(matches!(c.verdict, Verdict::Go | Verdict::NoGo));
        }
        for pr in dataset_prs {
            assert!(
                input.consensus.iter().any(|c| c.pr == pr),
                "PR {pr} appears in dataset but has no consensus record"
            );
        }
    }

    #[test]
    fn every_golden_has_nonempty_description() {
        let input = load();
        for g in &input.golden_issues {
            assert!(
                !g.description.trim().is_empty(),
                "golden {} lacks description",
                g.id
            );
        }
    }
}
