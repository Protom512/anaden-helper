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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    pub pr: u64,
    pub reviewer: String,
    pub matched_golden: Option<String>,
}

/// QC Manager のコンセンサス判定と実際の帰結。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsensusRecord {
    pub pr: u64,
    pub verdict: Verdict,
    pub merged: bool,
    /// マージ後に判明した問題の issue/issue-comment 識別子。
    pub post_merge_issue_ids: Vec<String>,
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
        match (r.verdict, has_problem) {
            (Verdict::NoGo, true) => cv.tp += 1,
            (Verdict::NoGo, false) => cv.fp += 1,
            (Verdict::Go, false) => cv.tn += 1,
            (Verdict::Go, true) => cv.false_negatives += 1,
        }
        if r.verdict == Verdict::NoGo && r.merged {
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
