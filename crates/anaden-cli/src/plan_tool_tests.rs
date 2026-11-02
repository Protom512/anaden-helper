//! plan_tool の振る舞いテスト (公開関数経由・実データ込み)。
//! plan_tool.rs から `#[path]` で読み込まれる子モジュール。

use super::*;
use crate::plan_data::{DifficultyTable, RewardSlots};

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

fn real_tenmei() -> TenmeiDoc {
    load_tenmei(&workspace_root().join("data").join("ad_tenmei.toml")).unwrap()
}

fn real_plan() -> PlanDoc {
    load_plan(
        &workspace_root()
            .join("data")
            .join("ticket_digest_plan.toml"),
    )
    .unwrap()
}

fn chars(names: &[&str]) -> Vec<String> {
    names.iter().map(|s| s.to_string()).collect()
}

fn alignment(h: SlotAlignment, vh: SlotAlignment) -> DifficultyTable<SlotAlignment> {
    DifficultyTable {
        hard: Some(h),
        very_hard: Some(vh),
    }
}

fn keys(h: &str, vh: &str) -> DifficultyTable<String> {
    DifficultyTable {
        hard: Some(h.to_string()),
        very_hard: Some(vh.to_string()),
    }
}

/// 実データに依存しない邁縁ケース用の最小 tenmei doc。
/// ダンジョン: 城 (スキップ可・天) / 秘境 (スキップ不可)。
/// キャラ: ア (城+秘境+表外) / ベ (城のみ) / 該当なし (秘境のみ)。
fn synthetic_tenmei() -> TenmeiDoc {
    TenmeiDoc {
        version: 1,
        source: "synthetic".to_string(),
        reward_slots: RewardSlots {
            vh: vec![24],
            hard: vec![0],
            iseki_vh: vec![48],
            iseki_hard: vec![0],
            gallerea: vec![24],
        },
        dungeons: vec![
            TenmeiDungeon {
                name: "城".to_string(),
                category: "main".to_string(),
                skip_target: true,
                key: keys("green", "red"),
                alignment: alignment(SlotAlignment::Ten, SlotAlignment::Ten),
            },
            TenmeiDungeon {
                name: "秘境".to_string(),
                category: "hiseki".to_string(),
                skip_target: false,
                key: DifficultyTable {
                    hard: None,
                    very_hard: None,
                },
                alignment: DifficultyTable {
                    hard: None,
                    very_hard: Some(SlotAlignment::AllDist),
                },
            },
        ],
        chars: vec![
            TenmeiChar {
                name: "ア".to_string(),
                alignment: SlotAlignment::Ten,
                dungeons: vec!["城".to_string(), "秘境".to_string(), "表外".to_string()],
            },
            TenmeiChar {
                name: "ベ".to_string(),
                alignment: SlotAlignment::Mei,
                dungeons: vec!["城".to_string()],
            },
            TenmeiChar {
                name: "該当なし".to_string(),
                alignment: SlotAlignment::Mei,
                dungeons: vec!["秘境".to_string()],
            },
        ],
    }
}

fn plan_of(entries: Vec<PlanEntry>) -> PlanDoc {
    PlanDoc {
        version: 1,
        on_ticket_empty: OnTicketEmpty::Stop,
        auto_buy_key_with_stone: false,
        entries,
    }
}

fn entry(dungeon: &str, difficulty: Difficulty, priority: u32) -> PlanEntry {
    PlanEntry {
        dungeon: dungeon.to_string(),
        difficulty,
        priority,
        note: String::new(),
    }
}

// ---- generate_plan (逆引き) ----

#[test]
fn generates_plan_for_aldo_from_real_data() {
    let tenmei = real_tenmei();
    let (plan, notes) = generate_plan(&tenmei, &chars(&["アルド"]), Difficulty::VeryHard).unwrap();
    assert_eq!(
        plan.entries.len(),
        1,
        "蝕時ユニガンは [[dungeon]] 表外で除外: {notes:?}"
    );
    let e = plan.entries.first().unwrap();
    assert_eq!(e.dungeon, "ミグランス城");
    assert_eq!(e.difficulty, Difficulty::VeryHard);
    assert_eq!(e.priority, 1);
    assert_eq!(e.note, "アルド天 / VH報酬枠=天");
    assert!(
        notes.iter().any(|n| n.contains("蝕時ユニガン")),
        "{notes:?}"
    );
}

#[test]
fn assigns_priority_in_char_order() {
    let (plan, _) = generate_plan(
        &real_tenmei(),
        &chars(&["アルド", "リィカ"]),
        Difficulty::VeryHard,
    )
    .unwrap();
    let names: Vec<&str> = plan.entries.iter().map(|e| e.dungeon.as_str()).collect();
    assert_eq!(names, ["ミグランス城", "ゼノ・ドメイン"]);
    assert_eq!(plan.entries[0].priority, 1);
    assert_eq!(plan.entries[1].priority, 2);
}

#[test]
fn unknown_char_fails_closed_with_name() {
    let err = generate_plan(
        &real_tenmei(),
        &chars(&["存在しないキャラ"]),
        Difficulty::VeryHard,
    )
    .expect_err("unknown char must fail");
    assert!(format!("{err}").contains("存在しないキャラ"), "{err}");
}

#[test]
fn excludes_non_skip_and_unknown_dungeons_with_notes() {
    let (plan, notes) =
        generate_plan(&synthetic_tenmei(), &chars(&["ア"]), Difficulty::VeryHard).unwrap();
    assert_eq!(plan.entries.len(), 1);
    assert_eq!(plan.entries[0].dungeon, "城");
    assert!(
        notes
            .iter()
            .any(|n| n.contains("秘境") && n.contains("スキップ対象外")),
        "{notes:?}"
    );
    assert!(notes.iter().any(|n| n.contains("表外")), "{notes:?}");
}

#[test]
fn dedups_dungeon_shared_by_two_chars() {
    let (plan, notes) = generate_plan(
        &synthetic_tenmei(),
        &chars(&["ア", "ベ"]),
        Difficulty::VeryHard,
    )
    .unwrap();
    assert_eq!(
        plan.entries.len(),
        1,
        "城 is shared by ア and ベ: {notes:?}"
    );
    assert_eq!(plan.entries[0].note, "ア天 / VH報酬枠=天");
    assert!(
        notes.iter().any(|n| n.contains("重複") && n.contains("ベ")),
        "{notes:?}"
    );
}

#[test]
fn all_excluded_char_is_error() {
    let err = generate_plan(
        &synthetic_tenmei(),
        &chars(&["該当なし"]),
        Difficulty::VeryHard,
    )
    .expect_err("must not produce empty plan");
    assert!(format!("{err}").contains("0 件"), "{err}");
}

// ---- expand_plan (静的展開) ----

#[test]
fn expands_real_plan_into_engine_validatable_routine() {
    let def = expand_plan(
        &real_plan(),
        &real_tenmei(),
        "templates/pipelines/ticket_digest",
        "AdSelectDetectPc",
    )
    .unwrap();
    assert_eq!(def.name, "ticket_digest");
    assert_eq!(def.steps.len(), 3);
    def.validate(&workspace_root())
        .expect("generated routine must validate against real workspace");
    let first = def.steps.first().unwrap();
    assert_eq!(first.name, "digest_01_ミグランス城_VH");
    assert_eq!(first.start_task, "AdSelectDetectPc");
    assert_eq!(first.pipeline_dir, "templates/pipelines/ticket_digest");
    assert_eq!(first.interval, STEP_INTERVAL_SECS);
    assert_eq!(first.max_iters, STEP_MAX_ITERS);
}

#[test]
fn expansion_sorts_entries_by_priority() {
    let plan = plan_of(vec![
        entry("城", Difficulty::VeryHard, 2),
        entry("城", Difficulty::Hard, 1),
    ]);
    let def = expand_plan(&plan, &synthetic_tenmei(), "p", "T").unwrap();
    let names: Vec<&str> = def.steps.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["digest_01_城_H", "digest_02_城_VH"]);
}

#[test]
fn expansion_fails_closed_on_unknown_not_skippable_and_duplicate() {
    let unknown = plan_of(vec![entry("存在しないダンジョン", Difficulty::VeryHard, 1)]);
    let err = expand_plan(&unknown, &synthetic_tenmei(), "p", "T").expect_err("must fail");
    assert!(format!("{err}").contains("存在しないダンジョン"), "{err}");

    let not_skippable = plan_of(vec![entry("秘境", Difficulty::VeryHard, 1)]);
    let err = expand_plan(&not_skippable, &synthetic_tenmei(), "p", "T").expect_err("must fail");
    assert!(format!("{err}").contains("skip_target"), "{err}");

    let dup = plan_of(vec![
        entry("城", Difficulty::VeryHard, 1),
        entry("城", Difficulty::VeryHard, 2),
    ]);
    let err = expand_plan(&dup, &synthetic_tenmei(), "p", "T").expect_err("must fail");
    assert!(format!("{err}").contains("重複"), "{err}");

    let empty = plan_of(vec![]);
    let err = expand_plan(&empty, &synthetic_tenmei(), "p", "T").expect_err("must fail");
    assert!(format!("{err}").contains("[[entry]]"), "{err}");
}

// ---- コマンド本体 (ファイル入出力込み) ----

#[test]
fn run_plan_tenmei_writes_parseable_plan_file() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("nested").join("plan.toml");
    run_plan_tenmei(
        &chars(&["アルド"]),
        &workspace_root().join("data").join("ad_tenmei.toml"),
        Difficulty::VeryHard,
        Some(&out),
    )
    .unwrap();
    let plan = load_plan(&out).unwrap();
    assert_eq!(plan.entries.len(), 1);
    assert_eq!(plan.entries[0].dungeon, "ミグランス城");
}

#[test]
fn run_plan_expand_writes_engine_loadable_routine() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("routine.toml");
    run_plan_expand(
        &workspace_root()
            .join("data")
            .join("ticket_digest_plan.toml"),
        &workspace_root().join("data").join("ad_tenmei.toml"),
        "templates/pipelines/ticket_digest",
        Some(&out),
    )
    .unwrap();
    let def = anaden_engine::load_routine(&out).unwrap();
    assert_eq!(def.name, "ticket_digest");
    assert_eq!(def.steps.len(), 3);
    def.validate(&workspace_root()).unwrap();
}
