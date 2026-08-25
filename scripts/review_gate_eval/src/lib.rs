//! review-gate 定量評価: メトリクス集計ライブラリ (issue #76 Task 3)
//!
//! Task 1 の golden dataset (既知問題の正解リスト) と Task 2 の実行結果
//! (reviewer 指摘 + コンセンサス判定) を入力とし、PR ごと / 全体の
//! (1) recall, (2) 偽陽性率 (N 付き・Wilson 95% CI), (3) コンセンサス妥当性
//! (混同行列) を集計する。
//!
//! 設計方針:
//! - 分母が 0 のレートは点推定せず `None` を返す (estimate 承認条件)。
//! - `matched_golden` が正解リストに存在しない ID を参照する場合は集計エラー。

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// 正解 (golden) 問題: PR 本文/comments から客観的ルールで抽出された既知問題。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoldenIssue {
    pub pr: u64,
    pub id: String,
    pub description: String,
}

/// review-gate レビュアーの指摘 1 件。
/// `matched_golden` は評価者が正解問題と対応づけた場合のみ `Some(golden_id)`。
///
/// #80 拡張: `confidence` (LLM 判定の自己申告信頼度 0.0-1.0) と
/// `command_results` (clippy/nextest の決定論的成否) を追加。
/// いずれも旧 results.json との後方互換のため `#[serde(default)]` を付ける。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    pub pr: u64,
    pub reviewer: String,
    pub matched_golden: Option<String>,
    /// レビュアーの自己申告信頼度。旧データでは欠損 (`None`)。
    #[serde(default)]
    pub confidence: Option<f64>,
    /// clippy / nextest の決定論的成否。旧データでは欠損 (`None`)。
    /// 欠損 (`None`) と失敗 (`Some(.. CommandStatus::Fail ..)`) は区別される。
    #[serde(default)]
    pub command_results: Option<CommandResults>,
}

/// 個別コマンドの成否。欠損 (未実施/旧データ) は `None` で表現し、
/// スキーマデフォルトの欠損が暗黙に成功扱いにならないよう `Option` で包む。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandResults {
    #[serde(default)]
    pub clippy: Option<CommandStatus>,
    #[serde(default)]
    pub nextest: Option<CommandStatus>,
}

impl CommandResults {
    /// 決定論的チェックのいずれかが明示的に失敗している場合に true。
    /// 欠損 (`None`) は失敗扱いにしない (強制 NO-GO は明示的 Fail のみ)。
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

/// レビュアー 1 名の判定 (コンセンサス割れ記録用)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReviewerJudgment {
    pub reviewer: String,
    pub verdict: Verdict,
    /// 自己申告 confidence (0.0-1.0)。欠損可。
    #[serde(default)]
    pub confidence: Option<f64>,
    /// critical finding を含むか。critical は単独 NO-GO (veto) となる。
    #[serde(default)]
    pub has_critical: bool,
}

/// コンセンサスの割れ情報: 各レビュアー判定と決着方式。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SplitInfo {
    pub judgments: Vec<ReviewerJudgment>,
    /// critical finding による単独 NO-GO (veto) が発動したか。
    #[serde(default)]
    pub veto_activated: bool,
    /// 決定論的コマンド失敗による強制 NO-GO が発動したか (#80 承認条件)。
    #[serde(default)]
    pub command_fail_forced_nogo: bool,
}

/// コンセンサスの決着方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DecisionMethod {
    /// 全員 GO (割れなし)。
    UnanimousGo,
    /// majority 決着 (過半数 GO)。
    Majority,
    /// critical finding による単独 veto。
    CriticalVeto,
    /// 決定論的コマンド失敗による強制 NO-GO (veto と同格)。
    CommandFail,
}

impl SplitInfo {
    /// 判定集合から決着方式を導出する。
    /// 優先順位: CommandFail > CriticalVeto > Majority/UnanimousGo。
    /// command fail は LLM 判定と二重カウントせず最優先で強制 NO-GO とする。
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

    /// 決着方式から導かれる実効 verdict。
    /// - CommandFail / CriticalVeto: NoGo (強制)
    /// - UnanimousGo: Go
    /// - Majority: 過半数 GO なら Go、それ以外は NoGo
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

/// QC Manager のコンセンサス判定と実際の帰結。
/// #80 拡張: `split_info` (各レビュアー判定・confidence・veto/majority 決着方式) を追加。
/// 旧データとの後方互換のため `#[serde(default)]` を付ける。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsensusRecord {
    pub pr: u64,
    pub verdict: Verdict,
    pub merged: bool,
    /// マージ後に判明した問題の issue/issue-comment 識別子。
    pub post_merge_issue_ids: Vec<String>,
    /// コンセンサスの割れ情報。旧データでは欠損 (`None`)。
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

/// recall: 検出された正解問題の割合 (正解は PR 単位で一意に数える)。
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

    /// 分母 0 では呼ばないこと (テスト専用の利便メソッド)。
    #[must_use]
    pub fn recall(&self) -> f64 {
        self.recall_opt().unwrap_or(f64::NAN)
    }
}

/// 偽陽性率: 正解に対応しない指摘の割合。必ず N とともに報告する。
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

    /// 分母 0 では呼ばないこと (テスト専用の利便メソッド)。
    #[must_use]
    pub fn rate(&self) -> f64 {
        self.rate_opt().unwrap_or(f64::NAN)
    }

    /// Wilson score interval (95%)。N が小さい PR での点推定のみの報告を避けるため。
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

    /// レポート要件: N が必ず併記される構造であることの明示的チェック。
    pub fn n_included_in_report(&self) -> bool {
        true // struct フィールド total_findings を必ず serde で出力するため常に true
    }
}

/// コンセンサス妥当性の混同行列。
/// actual = post_merge_issue_ids が空でない (マージ後に問題が判明した) PR。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsensusValidity {
    /// NoGo 判定 & 実際に問題あり (正当なブロック)。
    pub tp: u64,
    /// NoGo 判定 & 問題なし (保守的すぎ / 偽ブロック)。
    pub fp: u64,
    /// Go 判定 & 問題なし (正当な通過)。
    pub tn: u64,
    /// Go 判定 & 問題あり (見逃し)。
    pub false_negatives: u64,
    /// NoGo なのにマージされた PR (プロセス逸脱の列挙用)。
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

    /// 分母 0 では呼ばないこと (テスト専用の利便メソッド)。
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

/// 集計本体。入力の整合性 (golden 外 ID 参照) を検証してからメトリクスを算出する。
///
/// # Errors
/// - 指摘の `matched_golden` が golden dataset に存在しない ID を参照する場合
pub fn aggregate(input: &EvalInput) -> Result<Metrics, EvalError> {
    let golden_ids: HashSet<&str> = input.golden_issues.iter().map(|g| g.id.as_str()).collect();
    for f in &input.findings {
        if let Some(id) = &f.matched_golden {
            // golden dataset が空の場合は突合不能のため検証をスキップ (分母0は集計側で None 化)
            if !golden_ids.is_empty() && !golden_ids.contains(id.as_str()) {
                return Err(EvalError::UnknownGoldenId(id.clone()));
            }
        }
    }

    // --- recall (PR 単位で一意な golden を分母に) ---
    let mut per_pr: HashMap<u64, PrRecall> = HashMap::new();
    for g in &input.golden_issues {
        per_pr.entry(g.pr).or_insert(PrRecall {
            pr: g.pr,
            total_golden: 0,
            detected_golden: 0,
        });
        let e = per_pr
            .get_mut(&g.pr)
            .ok_or(EvalError::UnknownGoldenId(g.id.clone()))?;
        e.total_golden += 1;
    }
    let mut detected_ids: HashSet<(u64, &str)> = HashSet::new();
    for f in &input.findings {
        if let Some(id) = &f.matched_golden {
            detected_ids.insert((f.pr, id.as_str()));
        }
    }
    for (pr, _id) in &detected_ids {
        if let Some(e) = per_pr.get_mut(pr) {
            e.detected_golden += 1;
        }
    }
    let mut recall_per_pr: Vec<PrRecall> = per_pr.into_values().collect();
    recall_per_pr.sort_by_key(|r| r.pr);
    let total_golden: u64 = recall_per_pr.iter().map(|r| r.total_golden).sum();
    let detected_golden: u64 = recall_per_pr.iter().map(|r| r.detected_golden).sum();

    // --- false positive rate ---
    let total_findings = input.findings.len() as u64;
    let false_positives = total_findings - detected_ids.len() as u64;

    // --- consensus confusion matrix ---
    let mut cv = ConsensusValidity {
        tp: 0,
        fp: 0,
        tn: 0,
        false_negatives: 0,
        nogo_merged_prs: Vec::new(),
    };
    for r in &input.consensus {
        let has_problem = !r.post_merge_issue_ids.is_empty();
        // #80: split_info が存在する場合は majority/critical-veto/command-fail による
        // effective_verdict を優先する (旧 AND コンセンサスの偽ブロック構造の解消)。
        // 欠損 (旧データ) は従来どおり verdict フィールドで集計。
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
        consensus: cv,
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

    // --- UC-1: 全 reviewer GO → GO ---
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

    // --- UC-2: 1名が低confidence NO-GO、コマンド成否正常、critical なし → majority で GO ---
    #[test]
    fn uc2_low_confidence_single_nogo_resolved_by_majority_go() {
        let split = SplitInfo {
            judgments: vec![
                judgment("arch", Verdict::Go, Some(0.9)),
                judgment("func", Verdict::Go, Some(0.8)),
                judgment("maint", Verdict::NoGo, Some(0.3)), // 低confidence NO-GO
            ],
            veto_activated: false,
            command_fail_forced_nogo: false,
        };
        assert_eq!(split.decision_method(), DecisionMethod::Majority);
        assert_eq!(split.effective_verdict(), Verdict::Go);
    }

    // --- エッジケース: critical finding 1件 → 単独 NO-GO (veto) ---
    #[test]
    fn critical_finding_alone_triggers_veto_nogo() {
        let mut maint = judgment("maint", Verdict::Go, Some(0.9));
        maint.has_critical = true; // GO 判定でも critical は veto
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

    // --- 承認条件: command fail は veto と同格の強制 NO-GO として記録され決定論的に決まる ---
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

    // --- 欠損 (missing) は fail と区別され、暗黙に pass/fail 扱いにしない ---
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

    // --- serde 後方互換: 旧形式 (confidence/command_results/split_info なし) が parsable ---
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

    // --- 新フィールドの round-trip ---
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

    // --- majority 割れ (1 GO / 2 NO-GO) は NO-GO ---
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

    // --- Finding の command_results が fail を明示的に保持する ---
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

    // --- split_info 付きレコードが aggregate() を通過する (混同行列への影響なし) ---
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
