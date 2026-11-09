use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoldenIssue {
    pub pr: u64,
    pub id: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    pub pr: u64,
    pub reviewer: String,
    pub matched_golden: Option<String>,
    #[serde(default)]
    pub confidence: Option<f64>,
    #[serde(default)]
    pub command_results: Option<CommandResults>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandResults {
    #[serde(default)]
    pub clippy: Option<CommandStatus>,
    #[serde(default)]
    pub nextest: Option<CommandStatus>,
}

impl CommandResults {
    #[must_use]
    pub fn any_fail(&self) -> bool {
        self.clippy == Some(CommandStatus::Fail) || self.nextest == Some(CommandStatus::Fail)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CommandStatus {
    Pass,
    Fail,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReviewerJudgment {
    pub reviewer: String,
    pub verdict: Verdict,
    #[serde(default)]
    pub confidence: Option<f64>,
    #[serde(default)]
    pub has_critical: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SplitInfo {
    pub judgments: Vec<ReviewerJudgment>,
    #[serde(default)]
    pub veto_activated: bool,
    #[serde(default)]
    pub command_fail_forced_nogo: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DecisionMethod {
    UnanimousGo,
    Majority,
    CriticalVeto,
    CommandFail,
}

impl SplitInfo {
    #[must_use]
    pub fn decision_method(&self) -> DecisionMethod {
        if self.command_fail_forced_nogo {
            return DecisionMethod::CommandFail;
        }
        if self.veto_activated || self.judgments.iter().any(|j| j.has_critical) {
            return DecisionMethod::CriticalVeto;
        }
        let go = self
            .judgments
            .iter()
            .filter(|j| j.verdict == Verdict::Go)
            .count();
        if go == self.judgments.len() && !self.judgments.is_empty() {
            DecisionMethod::UnanimousGo
        } else {
            DecisionMethod::Majority
        }
    }

    #[must_use]
    pub fn effective_verdict(&self) -> Verdict {
        match self.decision_method() {
            DecisionMethod::CommandFail | DecisionMethod::CriticalVeto => Verdict::NoGo,
            DecisionMethod::UnanimousGo => Verdict::Go,
            DecisionMethod::Majority => {
                let total = self.judgments.len();
                let go = self
                    .judgments
                    .iter()
                    .filter(|j| j.verdict == Verdict::Go)
                    .count();
                if total > 0 && go * 2 > total {
                    Verdict::Go
                } else {
                    Verdict::NoGo
                }
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsensusRecord {
    pub pr: u64,
    pub verdict: Verdict,
    pub merged: bool,
    pub post_merge_issue_ids: Vec<String>,
    #[serde(default)]
    pub split_info: Option<SplitInfo>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Verdict {
    Go,
    NoGo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalInput {
    pub golden_issues: Vec<GoldenIssue>,
    pub findings: Vec<Finding>,
    pub consensus: Vec<ConsensusRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recall {
    pub total_golden: u64,
    pub detected_golden: u64,
}

impl Recall {
    pub fn recall_opt(&self) -> Option<f64> {
        if self.total_golden == 0 {
            None
        } else {
            Some(self.detected_golden as f64 / self.total_golden as f64)
        }
    }

    #[must_use]
    pub fn recall(&self) -> f64 {
        self.recall_opt().unwrap_or(f64::NAN)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FpRate {
    #[serde(rename = "n")]
    pub total_findings: u64,
    pub false_positives: u64,
}

const Z95: f64 = 1.96;

impl FpRate {
    pub fn validate(&self) -> Result<(), EvalError> {
        if self.false_positives > self.total_findings {
            Err(EvalError::InvalidCounts {
                total: self.total_findings,
                fps: self.false_positives,
            })
        } else {
            Ok(())
        }
    }

    pub fn rate_opt(&self) -> Option<f64> {
        if self.total_findings == 0 {
            None
        } else {
            Some(self.false_positives as f64 / self.total_findings as f64)
        }
    }

    #[must_use]
    pub fn rate(&self) -> f64 {
        self.rate_opt().unwrap_or(f64::NAN)
    }

    pub fn wilson_95(&self) -> Result<Option<(f64, f64)>, EvalError> {
        self.validate()?;
        let n = self.total_findings;
        if n == 0 {
            return Ok(None);
        }
        let p = self.rate();
        let z2 = Z95 * Z95;
        let denom = 1.0 + z2 / n as f64;
        let center = (p + z2 / (2.0 * n as f64)) / denom;
        let half =
            Z95 * (p * (1.0 - p) / n as f64 + z2 / (4.0 * (n as f64) * (n as f64))).sqrt() / denom;
        let lo = (center - half).max(0.0);
        let hi = (center + half).min(1.0);
        Ok(Some((lo, hi)))
    }

    pub fn n_included_in_report(&self) -> bool {
        true
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsensusValidity {
    pub tp: u64,
    pub fp: u64,
    pub tn: u64,
    pub false_negatives: u64,
    pub nogo_merged_prs: Vec<ConsensusRecord>,
}

impl ConsensusValidity {
    pub fn accuracy_opt(&self) -> Option<f64> {
        let total = self.tp + self.fp + self.tn + self.false_negatives;
        if total == 0 {
            None
        } else {
            Some((self.tp + self.tn) as f64 / total as f64)
        }
    }

    #[must_use]
    pub fn accuracy(&self) -> f64 {
        self.accuracy_opt().unwrap_or(f64::NAN)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrRecall {
    pub pr: u64,
    pub total_golden: u64,
    pub detected_golden: u64,
}

impl PrRecall {
    pub fn recall_opt(&self) -> Option<f64> {
        if self.total_golden == 0 {
            None
        } else {
            Some(self.detected_golden as f64 / self.total_golden as f64)
        }
    }

    #[must_use]
    pub fn recall(&self) -> f64 {
        self.recall_opt().unwrap_or(f64::NAN)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Metrics {
    pub recall: Recall,
    pub recall_per_pr: Vec<PrRecall>,
    pub fp_rate: FpRate,
    pub consensus: ConsensusValidity,
}

#[derive(Debug, thiserror::Error)]
#[error("invalid counts: total={total}, false positives={fps}")]
pub enum EvalError {
    #[error("matched golden id not in dataset: {0}")]
    UnknownGoldenId(String),
    InvalidCounts {
        total: u64,
        fps: u64,
    },
}

fn validate_golden_matches(
    findings: &[Finding],
    golden_ids: &HashSet<&str>,
) -> Result<(), EvalError> {
    for f in findings {
        if let Some(id) = &f.matched_golden
            && !golden_ids.is_empty()
            && !golden_ids.contains(id.as_str())
        {
            return Err(EvalError::UnknownGoldenId(id.clone()));
        }
    }
    Ok(())
}

fn build_recall_per_pr(golden_issues: &[GoldenIssue], findings: &[Finding]) -> Vec<PrRecall> {
    let mut per_pr: HashMap<u64, PrRecall> = HashMap::new();
    for g in golden_issues {
        per_pr
            .entry(g.pr)
            .or_insert(PrRecall {
                pr: g.pr,
                total_golden: 0,
                detected_golden: 0,
            })
            .total_golden += 1;
    }
    let mut detected_ids: HashSet<(u64, &str)> = HashSet::new();
    for f in findings {
        if let Some(id) = &f.matched_golden {
            detected_ids.insert((f.pr, id.as_str()));
        }
    }
    for (pr, _id) in &detected_ids {
        if let Some(e) = per_pr.get_mut(pr) {
            e.detected_golden += 1;
        }
    }
    per_pr.into_values().collect()
}

fn build_consensus_validity(consensus: &[ConsensusRecord]) -> ConsensusValidity {
    let mut cv = ConsensusValidity {
        tp: 0,
        fp: 0,
        tn: 0,
        false_negatives: 0,
        nogo_merged_prs: Vec::new(),
    };
    for r in consensus {
        let has_problem = !r.post_merge_issue_ids.is_empty();
        let effective = r
            .split_info
            .as_ref()
            .map_or(r.verdict, SplitInfo::effective_verdict);
        match (effective, has_problem) {
            (Verdict::NoGo, true) => cv.tp += 1,
            (Verdict::NoGo, false) => cv.fp += 1,
            (Verdict::Go, false) => cv.tn += 1,
            (Verdict::Go, true) => cv.false_negatives += 1,
        }
        if effective == Verdict::NoGo && r.merged {
            cv.nogo_merged_prs.push(r.clone());
        }
    }
    cv
}

pub fn aggregate(input: &EvalInput) -> Result<Metrics, EvalError> {
    let golden_ids: HashSet<&str> = input.golden_issues.iter().map(|g| g.id.as_str()).collect();
    validate_golden_matches(&input.findings, &golden_ids)?;

    let mut recall_per_pr = build_recall_per_pr(&input.golden_issues, &input.findings);
    recall_per_pr.sort_by_key(|r| r.pr);
    let total_golden: u64 = recall_per_pr.iter().map(|r| r.total_golden).sum();
    let detected_golden: u64 = recall_per_pr.iter().map(|r| r.detected_golden).sum();

    let total_findings = input.findings.len() as u64;
    let false_positives = input
        .findings
        .iter()
        .filter(|f| f.matched_golden.is_none())
        .count() as u64;

    Ok(Metrics {
        recall: Recall {
            total_golden,
            detected_golden,
        },
        recall_per_pr,
        fp_rate: FpRate {
            total_findings,
            false_positives,
        },
        consensus: build_consensus_validity(&input.consensus),
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    fn judgment(reviewer: &str, verdict: Verdict, confidence: Option<f64>) -> ReviewerJudgment {
        ReviewerJudgment {
            reviewer: reviewer.to_string(),
            verdict,
            confidence,
            has_critical: false,
        }
    }

    #[test]
    fn uc1_unanimous_go_yields_go() {
        let split = SplitInfo {
            judgments: vec![
                judgment("arch", Verdict::Go, Some(0.9)),
                judgment("func", Verdict::Go, Some(0.85)),
                judgment("maint", Verdict::Go, Some(0.95)),
            ],
            veto_activated: false,
            command_fail_forced_nogo: false,
        };
        assert_eq!(split.decision_method(), DecisionMethod::UnanimousGo);
        assert_eq!(split.effective_verdict(), Verdict::Go);
    }

    #[test]
    fn uc2_low_confidence_single_nogo_resolved_by_majority_go() {
        let split = SplitInfo {
            judgments: vec![
                judgment("arch", Verdict::Go, Some(0.9)),
                judgment("func", Verdict::Go, Some(0.8)),
                judgment("maint", Verdict::NoGo, Some(0.3)),
            ],
            veto_activated: false,
            command_fail_forced_nogo: false,
        };
        assert_eq!(split.decision_method(), DecisionMethod::Majority);
        assert_eq!(split.effective_verdict(), Verdict::Go);
    }

    #[test]
    fn critical_finding_alone_triggers_veto_nogo() {
        let mut maint = judgment("maint", Verdict::Go, Some(0.9));
        maint.has_critical = true;
        let split = SplitInfo {
            judgments: vec![
                judgment("arch", Verdict::Go, Some(0.9)),
                judgment("func", Verdict::Go, Some(0.9)),
                maint,
            ],
            veto_activated: true,
            command_fail_forced_nogo: false,
        };
        assert_eq!(split.decision_method(), DecisionMethod::CriticalVeto);
        assert_eq!(split.effective_verdict(), Verdict::NoGo);
    }

    #[test]
    fn command_fail_forces_nogo_even_with_unanimous_go() {
        let split = SplitInfo {
            judgments: vec![
                judgment("arch", Verdict::Go, Some(0.9)),
                judgment("func", Verdict::Go, Some(0.9)),
                judgment("maint", Verdict::Go, Some(0.9)),
            ],
            veto_activated: false,
            command_fail_forced_nogo: true,
        };
        assert_eq!(split.decision_method(), DecisionMethod::CommandFail);
        assert_eq!(split.effective_verdict(), Verdict::NoGo);
    }

    #[test]
    fn missing_command_results_do_not_count_as_fail_or_pass() {
        let missing = CommandResults {
            clippy: None,
            nextest: None,
        };
        assert!(!missing.any_fail(), "missing must not count as fail");

        let failed = CommandResults {
            clippy: Some(CommandStatus::Pass),
            nextest: Some(CommandStatus::Fail),
        };
        assert!(failed.any_fail());

        let all_pass = CommandResults {
            clippy: Some(CommandStatus::Pass),
            nextest: Some(CommandStatus::Pass),
        };
        assert!(!all_pass.any_fail());
    }

    #[test]
    fn old_schema_json_still_parses_with_defaults() {
        let old_finding = r#"{"pr":1,"reviewer":"arch","matched_golden":null}"#;
        let f: Finding = serde_json::from_str(old_finding).expect("old Finding json parses");
        assert_eq!(f.confidence, None);
        assert_eq!(f.command_results, None);

        let old_record = r#"{"pr":1,"verdict":"GO","merged":true,"post_merge_issue_ids":[]}"#;
        let r: ConsensusRecord =
            serde_json::from_str(old_record).expect("old ConsensusRecord json parses");
        assert_eq!(r.split_info, None);
        assert_eq!(r.verdict, Verdict::Go);
    }

    #[test]
    fn new_schema_round_trips() {
        let record = ConsensusRecord {
            pr: 42,
            verdict: Verdict::NoGo,
            merged: false,
            post_merge_issue_ids: vec![],
            split_info: Some(SplitInfo {
                judgments: vec![judgment("arch", Verdict::Go, Some(0.9))],
                veto_activated: true,
                command_fail_forced_nogo: false,
            }),
        };
        let json = serde_json::to_string(&record).expect("serialize");
        let back: ConsensusRecord = serde_json::from_str(&json).expect("deserialize");
        let split = back.split_info.as_ref().unwrap();
        assert!(split.veto_activated);
        assert_eq!(split.judgments[0].confidence, Some(0.9));
        assert_eq!(back.verdict, Verdict::NoGo);
    }

    #[test]
    fn majority_minority_go_is_nogo() {
        let split = SplitInfo {
            judgments: vec![
                judgment("arch", Verdict::Go, Some(0.9)),
                judgment("func", Verdict::NoGo, Some(0.7)),
                judgment("maint", Verdict::NoGo, Some(0.8)),
            ],
            veto_activated: false,
            command_fail_forced_nogo: false,
        };
        assert_eq!(split.effective_verdict(), Verdict::NoGo);
    }

    #[test]
    fn finding_command_results_fail_is_distinct_from_missing() {
        let f: Finding = serde_json::from_str(
            r#"{"pr":1,"reviewer":"func","matched_golden":null,"confidence":0.5,
                "command_results":{"clippy":"FAIL","nextest":"PASS"}}"#,
        )
        .expect("finding with command_results parses");
        let cmds = f.command_results.expect("command_results present");
        assert_eq!(cmds.clippy, Some(CommandStatus::Fail));
        assert!(cmds.any_fail());
        assert_eq!(f.confidence, Some(0.5));
    }

    #[test]
    fn aggregate_accepts_records_with_split_info() {
        let input = EvalInput {
            golden_issues: vec![GoldenIssue {
                pr: 1,
                id: "G1".to_string(),
                description: "d".to_string(),
            }],
            findings: vec![Finding {
                pr: 1,
                reviewer: "arch".to_string(),
                matched_golden: Some("G1".to_string()),
                confidence: Some(0.9),
                command_results: Some(CommandResults {
                    clippy: Some(CommandStatus::Pass),
                    nextest: Some(CommandStatus::Pass),
                }),
            }],
            consensus: vec![ConsensusRecord {
                pr: 1,
                verdict: Verdict::Go,
                merged: true,
                post_merge_issue_ids: vec![],
                split_info: None,
            }],
        };
        let m = aggregate(&input).expect("aggregate succeeds with new fields");
        assert_eq!(m.consensus.tn, 1);
    }
}
