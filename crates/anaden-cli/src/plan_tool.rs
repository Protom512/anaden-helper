use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anaden_engine::{OnFailure, RoutineDef, RoutineStep};

use crate::plan_data::{
    Difficulty, OnTicketEmpty, PlanDoc, PlanEntry, SlotAlignment, TenmeiChar, TenmeiDoc,
    TenmeiDungeon, load_plan, load_tenmei, render_plan_toml, write_text_file,
};

const ROUTINE_NAME: &str = "ticket_digest";
const STEP_INTERVAL_SECS: u64 = 2;
const STEP_MAX_ITERS: u64 = 300;

enum DungeonLookup<'a> {
    Include(&'a TenmeiDungeon),
    Unknown,
    NotSkippable,
    Duplicate,
}

fn classify_dungeon<'a>(
    tenmei: &'a TenmeiDoc,
    dungeon_name: &str,
    difficulty: Difficulty,
    seen: &HashSet<(String, Difficulty)>,
) -> DungeonLookup<'a> {
    let Some(row) = tenmei.dungeons.iter().find(|d| d.name == dungeon_name) else {
        return DungeonLookup::Unknown;
    };
    if !row.skip_target {
        return DungeonLookup::NotSkippable;
    }
    if seen.contains(&(dungeon_name.to_string(), difficulty)) {
        return DungeonLookup::Duplicate;
    }
    DungeonLookup::Include(row)
}

fn note_for(char_row: &TenmeiChar, dungeon: &TenmeiDungeon, difficulty: Difficulty) -> String {
    let slot = dungeon
        .alignment
        .get(difficulty)
        .map_or("不明", SlotAlignment::label);
    format!(
        "{}{} / {}報酬枠={}",
        char_row.name,
        char_row.alignment.label(),
        difficulty,
        slot
    )
}

pub fn generate_plan(
    tenmei: &TenmeiDoc,
    char_names: &[String],
    difficulty: Difficulty,
) -> anyhow::Result<(PlanDoc, Vec<String>)> {
    let mut notes = vec![format!(
        "ad_tenmei.toml 読込: dungeons={} chars={} reward_slots(vh={}, hard={}, iseki_vh={}, iseki_hard={}, gallerea={})",
        tenmei.dungeons.len(),
        tenmei.chars.len(),
        tenmei.reward_slots.vh.len(),
        tenmei.reward_slots.hard.len(),
        tenmei.reward_slots.iseki_vh.len(),
        tenmei.reward_slots.iseki_hard.len(),
        tenmei.reward_slots.gallerea.len(),
    )];
    let mut entries: Vec<PlanEntry> = Vec::new();
    let mut seen: HashSet<(String, Difficulty)> = HashSet::new();
    for char_name in char_names {
        let rows: Vec<&TenmeiChar> = tenmei
            .chars
            .iter()
            .filter(|c| c.name == *char_name)
            .collect();
        if rows.is_empty() {
            anyhow::bail!(
                "キャラ '{char_name}' は ad_tenmei.toml の [[char]] に存在しない \
                 (収録 {n} 名 — data/ad_tenmei.toml 参照)",
                n = tenmei.chars.len()
            );
        }
        for row in rows {
            for dungeon_name in &row.dungeons {
                match classify_dungeon(tenmei, dungeon_name, difficulty, &seen) {
                    DungeonLookup::Include(dungeon) => {
                        entries.push(PlanEntry {
                            dungeon: dungeon_name.clone(),
                            difficulty,
                            priority: entries.len() as u32 + 1,
                            note: note_for(row, dungeon, difficulty),
                        });
                        seen.insert((dungeon_name.clone(), difficulty));
                    }
                    DungeonLookup::Unknown => {
                        notes.push(format!(
                            "除外: {dungeon_name} ([[dungeon]] 表に無い — {char_name})"
                        ));
                    }
                    DungeonLookup::NotSkippable => {
                        notes.push(format!(
                            "除外: {dungeon_name} (スキップ対象外 — {char_name})"
                        ));
                    }
                    DungeonLookup::Duplicate => {
                        notes.push(format!(
                            "重複: {dungeon_name} {difficulty} は上位優先度で登録済み ({char_name})"
                        ));
                    }
                }
            }
        }
    }
    if entries.is_empty() {
        anyhow::bail!(
            "計画エントリが 0 件 (全ダンジョンが除外された — {difficulty} でスキップ対象が無い)"
        );
    }
    Ok((
        PlanDoc {
            version: 1,
            on_ticket_empty: OnTicketEmpty::Stop,
            auto_buy_key_with_stone: false,
            entries,
        },
        notes,
    ))
}

pub fn expand_plan(
    plan: &PlanDoc,
    tenmei: &TenmeiDoc,
    pipeline_dir: &str,
    start_task: &str,
) -> anyhow::Result<RoutineDef> {
    if plan.entries.is_empty() {
        anyhow::bail!("計画に [[entry]] がない (空の計画は展開できない)");
    }
    let mut seen: HashSet<(String, Difficulty)> = HashSet::new();
    for entry in &plan.entries {
        let Some(row) = tenmei.dungeons.iter().find(|d| d.name == entry.dungeon) else {
            anyhow::bail!(
                "計画エントリ '{d}' は ad_tenmei.toml の [[dungeon]] に存在しない (設計 §6 突合)",
                d = entry.dungeon
            );
        };
        if !row.skip_target {
            anyhow::bail!(
                "計画エントリ '{d}' は skip_target = false (特急券でスキップできない)",
                d = entry.dungeon
            );
        }
        if !seen.insert((entry.dungeon.clone(), entry.difficulty)) {
            anyhow::bail!(
                "計画エントリ '{d}' ({diff}) が重複 (キーを無駄に消費する)",
                d = entry.dungeon,
                diff = entry.difficulty
            );
        }
    }
    let mut sorted = plan.entries.clone();
    sorted.sort_by_key(|e| e.priority);
    let steps = sorted
        .iter()
        .map(|e| RoutineStep {
            name: format!(
                "digest_{:02}_{}_{}",
                e.priority,
                e.dungeon,
                e.difficulty.as_toml_str()
            ),
            pipeline_dir: pipeline_dir.to_string(),
            start_task: start_task.to_string(),
            max_iters: STEP_MAX_ITERS,
            interval: STEP_INTERVAL_SECS,
            on_failure: OnFailure::Stop,
        })
        .collect();
    Ok(RoutineDef {
        name: ROUTINE_NAME.to_string(),
        steps,
    })
}

fn routine_header(plan_path: &Path, plan: &PlanDoc) -> String {
    let mode = match plan.on_ticket_empty {
        OnTicketEmpty::Stop => "stop",
        OnTicketEmpty::NormalEntry => "normal_entry",
    };
    format!(
        "# ticket_digest routine (anaden-tool plan-expand 生成 — Issue #248)\n\
         # 計画: {plan} (entries={n}, on_ticket_empty={mode}, auto_buy_key_with_stone={buy})\n\
         # 各ステップ = 1 ダンジョン 1 サイクル。interval {iv}s × max_iters {mi} = {budget}s\n\
         # (v0 pipeline goal Timeout(600) と同じ安全弁。FSM 拡張時に再調整)。\n\
         # on_ticket_empty / auto_buy_key_with_stone は live FSM (実機待ち) の契約で、\n\
         # 消費主体は計画 TOML 側 (routine スキーマは engine 変更なしの設計)。",
        plan = plan_path.display(),
        n = plan.entries.len(),
        buy = plan.auto_buy_key_with_stone,
        iv = STEP_INTERVAL_SECS,
        mi = STEP_MAX_ITERS,
        budget = STEP_INTERVAL_SECS * STEP_MAX_ITERS,
    )
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

pub fn run_plan_tenmei(
    char_names: &[String],
    data_path: &Path,
    difficulty: Difficulty,
    out: Option<&Path>,
) -> anyhow::Result<()> {
    let tenmei = load_tenmei(data_path)?;
    let (plan, notes) = generate_plan(&tenmei, char_names, difficulty)?;
    for note in notes {
        eprintln!("{note}");
    }
    let text = render_plan_toml(&plan)?;
    match out {
        Some(path) => {
            write_text_file(path, &text)?;
            eprintln!(
                "計画 {} エントリ ({difficulty}) → {}",
                plan.entries.len(),
                path.display()
            );
        }
        None => println!("{text}"),
    }
    Ok(())
}

pub fn run_plan_expand(
    plan_path: &Path,
    data_path: &Path,
    pipeline_dir: &str,
    out: Option<&Path>,
) -> anyhow::Result<()> {
    let plan = load_plan(plan_path)?;
    let tenmei = load_tenmei(data_path)?;
    let root = workspace_root();
    let dir =
        anaden_engine::resolve_pipeline_dir(Path::new(pipeline_dir), &root).ok_or_else(|| {
            anyhow::anyhow!(
                "pipeline ディレクトリが解決できない: {pipeline_dir} (root: {})",
                root.display()
            )
        })?;
    let manifest = anaden_vision::load_pipeline_manifest(&dir)
        .map_err(|e| anyhow::anyhow!("pipeline manifest 読込失敗 {}: {e}", dir.display()))?;
    let def = expand_plan(&plan, &tenmei, pipeline_dir, &manifest.start_task)?;
    let body = toml::to_string(&def).map_err(|e| anyhow::anyhow!("routine TOML 化に失敗: {e}"))?;
    let text = format!("{}\n\n{body}", routine_header(plan_path, &plan));
    match out {
        Some(path) => {
            write_text_file(path, &text)?;
            eprintln!(
                "routine '{}' ステップ {} 件 → {}",
                def.name,
                def.steps.len(),
                path.display()
            );
        }
        None => println!("{text}"),
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
#[path = "plan_tool_tests.rs"]
mod tests;
