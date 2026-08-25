//! reviewer 定義 (issue #80 Task 1) のスキーマ契約テスト。
//!
//! 3つの reviewer Markdown テンプレートに以下が必須であること:
//! - (a) confidence: high/medium/low フィールド
//! - (b) コマンド成否セクション: clippy / nextest の pass/fail を実行結果貼付で報告
//! - (c) 決定論的判定 (コマンド成否) と LLM 判定 (所見ベース GO/NO-GO) の分離

#![allow(clippy::unwrap_used)]
#![allow(clippy::panic)]
#![allow(clippy::expect_used)]

const REVIEWERS: &[(&str, &str)] = &[
    (
        "architecture",
        "../../.claude/agents/review/reviewer-architecture.md",
    ),
    (
        "functional",
        "../../.claude/agents/review/reviewer-functional.md",
    ),
    (
        "maintainability",
        "../../.claude/agents/review/reviewer-maintainability.md",
    ),
];

fn load(name: &str) -> String {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../.claude/agents/review/");
    let full = format!("{path}{name}");
    std::fs::read_to_string(&full).unwrap_or_else(|e| panic!("failed to read {full}: {e}"))
}

#[test]
fn all_reviewer_templates_define_confidence_field() {
    for (name, _) in REVIEWERS {
        let md = load(&format!("reviewer-{name}.md"));
        assert!(
            md.contains("confidence")
                && md.contains("high")
                && md.contains("medium")
                && md.contains("low"),
            "{name}: confidence (high/medium/low) が定義されていない"
        );
    }
}

#[test]
fn all_reviewer_templates_define_command_results_section() {
    for (name, _) in REVIEWERS {
        let md = load(&format!("reviewer-{name}.md"));
        let lower = md.to_lowercase();
        assert!(
            lower.contains("コマンド成否") || lower.contains("command results"),
            "{name}: コマンド成否セクションがない"
        );
        assert!(
            lower.contains("clippy") && lower.contains("nextest"),
            "{name}: clippy/nextest の成否報告がない"
        );
        assert!(
            lower.contains("pass") && lower.contains("fail"),
            "{name}: pass/fail の明示がない"
        );
        assert!(
            lower.contains("実行結果") && (lower.contains("貼付") || lower.contains("貼り付け")),
            "{name}: 実行結果の貼付必須と明記されていない"
        );
    }
}

#[test]
fn all_reviewer_templates_separate_deterministic_and_llm_judgments() {
    for (name, _) in REVIEWERS {
        let md = load(&format!("reviewer-{name}.md"));
        assert!(md.contains("決定論的"), "{name}: 決定論的判定の記述がない");
        assert!(md.contains("LLM"), "{name}: LLM 判定の記述がない");
    }
}
