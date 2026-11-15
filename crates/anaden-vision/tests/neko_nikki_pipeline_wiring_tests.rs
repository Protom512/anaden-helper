#![allow(clippy::unwrap_used)]
#![allow(clippy::expect_used)]
#![allow(clippy::panic)]

use std::collections::HashMap;
use std::path::PathBuf;

use anaden_core::StopCondition;
use anaden_vision::{Action, TaskDef, load_pipeline, load_pipeline_manifest};

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

fn pipeline_dir() -> PathBuf {
    workspace_root()
        .join("templates")
        .join("pipelines")
        .join("neko_nikki")
}

fn scenes_root() -> PathBuf {
    workspace_root().join("templates").join("scenes")
}

fn chain_defs() -> Vec<TaskDef> {
    load_pipeline(&pipeline_dir()).expect("neko_nikki pipeline dir must load")
}

fn task_index(defs: &[TaskDef]) -> HashMap<&str, &TaskDef> {
    let mut map = HashMap::new();
    for d in defs {
        let fresh = map.insert(d.name.as_str(), d).is_none();
        assert!(
            fresh,
            "duplicate task name {} in neko_nikki pipeline",
            d.name
        );
    }
    map
}

fn collect_template_refs(stop: &StopCondition, out: &mut Vec<String>) {
    match stop {
        StopCondition::TemplateMatch { task, .. } => out.push(task.clone()),
        StopCondition::All { conditions } | StopCondition::Any { conditions } => {
            for c in conditions {
                collect_template_refs(c, out);
            }
        }
        StopCondition::LoopCount { .. } | StopCondition::Timeout { .. } => {}
    }
}

const EXPECTED_TEMPLATES: &[(&str, &str)] = &[
    ("NekoFieldHudProbePc", "field_pc/hud_top.png"),
    ("NekoOpenMenuPc", "neko_pc/menu_btn.png"),
    ("NekoOpenDiaryPc", "neko_pc/diary_tab.png"),
    ("NekoDiaryHintDispatchPc", "neko_pc/hint_text_miparu.png"),
    ("NekoHintMiparuRoutePc", "neko_pc/hint_text_miparu.png"),
];

const T6_PENDING_ANCHORS: &[(&str, &str)] = &[
    ("NekoCatSpriteAPc", "neko_pc/cat_sprite_a.png"),
    ("NekoCatSpriteBPc", "neko_pc/cat_sprite_b.png"),
    ("NekoCatSpriteCPc", "neko_pc/cat_sprite_c.png"),
    ("NekoFeatherMarkerPc", "neko_pc/feather_marker.png"),
    ("NekoRewardOkPc", "neko_pc/reward_ok.png"),
    ("NekoPawmarkSlotPc", "neko_pc/pawmark_done.png"),
    ("NekoDiarySlotFramePc", "neko_pc/diary_slot_frame.png"),
    ("NekoDiaryAllPawmarkPc", "neko_pc/diary_all_pawmark.png"),
    ("NekoBattleEndPc", "common_pc/battle_end.png"),
    ("NekoLvlupPopupDismissPc", "common_pc/levelup_popup.png"),
    ("NekoStonePopupDismissPc", "common_pc/stone_popup.png"),
    ("NekoDiarySkipPc", "common_pc/close_btn.png"),
];

#[test]
fn neko_nikki_pipeline_manifest_loads_start_task_and_design_section7_goal_shape() {
    let manifest = load_pipeline_manifest(&pipeline_dir())
        .expect("neko_nikki pipeline.toml must load as manifest");
    assert_eq!(manifest.start_task, "NekoFieldHudProbePc");
    let chain = chain_defs();
    assert!(
        chain.iter().any(|d| d.name == manifest.start_task),
        "manifest.start_task must exist as a chain TaskDef"
    );

    assert_eq!(
        manifest.goals.len(),
        1,
        "exactly one goal per design section 7"
    );
    let goal = &manifest.goals[0];
    assert_eq!(goal.name, "neko_nikki_all_slots_done");
    let expected = StopCondition::Any {
        conditions: vec![
            StopCondition::All {
                conditions: vec![StopCondition::TemplateMatch {
                    task: "NekoDiaryAllPawmarkPc".to_string(),
                    confidence: 0.80,
                }],
            },
            StopCondition::Timeout { secs: 1800 },
        ],
    };
    assert_eq!(
        goal.stop, expected,
        "goal must be Any{{All{{TemplateMatch(diary all-pawmark anchor)}}, Timeout(1800)}} per design section 7 (3 pawmarked slots = done)"
    );
    goal.validate().expect("goal must validate");
}

#[test]
fn neko_nikki_goal_pawmark_condition_is_fail_closed_until_anchor_collection() {
    let manifest = load_pipeline_manifest(&pipeline_dir()).expect("manifest loads");
    let mut refs = Vec::new();
    collect_template_refs(&manifest.goals[0].stop, &mut refs);
    assert_eq!(
        refs,
        vec!["NekoDiaryAllPawmarkPc".to_string()],
        "the goal TemplateMatch must name the diary all-pawmark anchor task"
    );

    let chain = chain_defs();
    let index = task_index(&chain);
    let png = scenes_root().join("neko_pc").join("diary_all_pawmark.png");
    if let Some(def) = index.get("NekoDiaryAllPawmarkPc") {
        assert!(
            png.exists(),
            "goal anchor task exists but its collected png is missing at {}",
            png.display()
        );
        assert_eq!(
            def.template,
            pipeline_dir().join("../../scenes/neko_pc/diary_all_pawmark.png"),
            "goal anchor task must reference the neko_pc collection path"
        );
    } else {
        assert!(
            !png.exists(),
            "the all-pawmark anchor png exists but no goal anchor TaskDef references it: \
             wire the task into the pipeline so the goal can actually fire"
        );
    }
}

#[test]
fn neko_nikki_chain_references_resolve_and_templates_exist_on_disk() {
    let defs = chain_defs();
    let index = task_index(&defs);
    for d in &defs {
        for target in d.next.as_deref().unwrap_or(&[]) {
            assert!(
                index.contains_key(target.as_str()),
                "{}: next target {target} must resolve",
                d.name
            );
        }
        for target in d.candidates.as_deref().unwrap_or(&[]) {
            assert!(
                index.contains_key(target.as_str()),
                "{}: candidate {target} must resolve",
                d.name
            );
        }
        assert!(
            d.template.is_file(),
            "{}: template {} must exist (template_bank_audit enforces this for every \
             referenced pipeline template)",
            d.name,
            d.template.display()
        );
    }

    let mut table: HashMap<&str, &str> = HashMap::new();
    for (name, path) in EXPECTED_TEMPLATES {
        assert!(
            table.insert(name, path).is_none(),
            "template table must not duplicate task {name}"
        );
    }
    assert_eq!(
        table.len(),
        defs.len(),
        "the template table must cover every chain task exactly once (found {:?})",
        defs.iter().map(|d| d.name.as_str()).collect::<Vec<_>>()
    );
    for d in &defs {
        let expected = table.get(d.name.as_str()).expect("task covered by table");
        assert_eq!(
            d.template,
            pipeline_dir().join(format!("../../scenes/{expected}")),
            "{} must reference the scene-collection path {expected}",
            d.name
        );
    }
    for (name, path) in EXPECTED_TEMPLATES {
        let png = scenes_root().join(path.replace('/', "\\"));
        assert!(png.exists(), "{name} png must exist at {}", png.display());
    }
}

#[test]
fn neko_nikki_diary_dispatch_matches_collected_hint_candidates_in_order() {
    let defs = chain_defs();
    let index = task_index(&defs);
    let entry_chain = [
        ("NekoFieldHudProbePc", "NekoOpenMenuPc"),
        ("NekoOpenMenuPc", "NekoOpenDiaryPc"),
        ("NekoOpenDiaryPc", "NekoDiaryHintDispatchPc"),
    ];
    for (task, next) in entry_chain {
        let def = index.get(task).expect("entry chain task must exist");
        assert_eq!(
            def.next.as_deref(),
            Some(&[next.to_string()][..]),
            "{task} must chain into {next} (field HUD -> menu -> diary -> hint dispatch)"
        );
    }

    let dispatch = index
        .get("NekoDiaryHintDispatchPc")
        .copied()
        .expect("hint dispatcher must exist");
    let candidates = dispatch
        .candidates
        .as_deref()
        .expect("hint dispatcher must declare candidates");
    assert!(
        !candidates.is_empty(),
        "hint dispatcher must list collected hint candidates"
    );
    assert!(
        candidates.contains(&"NekoHintMiparuRoutePc".to_string()),
        "the one collected hint anchor (miparu) must be a dispatch candidate"
    );
    let miparu = index
        .get("NekoHintMiparuRoutePc")
        .expect("miparu route task must exist");
    assert!(
        miparu.template.is_file(),
        "miparu route candidate must reference its collected png"
    );
    assert_eq!(
        miparu.action,
        Some(Action::DoNothing),
        "hint identification is detection-only until the route chain is wired in"
    );
}

#[test]
fn neko_nikki_rois_stay_within_the_raw_pc_capture_space() {
    let defs = chain_defs();
    for d in &defs {
        if let Some([x, y, w, h]) = d.roi {
            assert!(w > 0 && h > 0, "{}: roi dims must be positive", d.name);
            assert!(
                x + w <= 1258 && y + h <= 708,
                "{}: roi [{x},{y},{w},{h}] exceeds RAW 1258x708 PC capture space",
                d.name
            );
        }
        if let Some(Action::ClickRect { roi }) = &d.action {
            assert!(
                roi.x + roi.width <= 1258 && roi.y + roi.height <= 708,
                "{}: click_rect roi exceeds RAW 1258x708 PC capture space",
                d.name
            );
        }
    }
}

#[test]
fn neko_nikki_walk_reward_watch_layers_stay_unwired_until_anchors_are_collected() {
    let defs = chain_defs();
    for (task_name, png_rel) in T6_PENDING_ANCHORS {
        let png = scenes_root().join(png_rel.replace('/', "\\"));
        if png.exists() {
            continue;
        }
        for d in &defs {
            assert_ne!(
                d.template,
                png,
                "{} references {} which no live collection has produced: the walk, \
                 reward and watch layers must stay unwired until the t6 anchors exist \
                 (template_bank_audit enforces referenced files, and a fabricated or \
                 mismatched anchor would mis-tap the live screen)",
                d.name,
                png.display()
            );
            assert_ne!(
                d.name, *task_name,
                "task {task_name} must not exist before its anchor png is collected"
            );
        }
    }

    let manifest = load_pipeline_manifest(&pipeline_dir()).expect("manifest loads");
    let watch_pngs_missing = T6_PENDING_ANCHORS
        .iter()
        .filter(|(name, _)| {
            matches!(
                name,
                &"NekoBattleEndPc" | &"NekoLvlupPopupDismissPc" | &"NekoStonePopupDismissPc"
            )
        })
        .all(|(_, rel)| !scenes_root().join(rel.replace('/', "\\")).exists());
    if watch_pngs_missing {
        assert!(
            manifest.watch.is_empty(),
            "watch layer must stay empty until the battle/popup anchors are collected (t6)"
        );
    }
}
