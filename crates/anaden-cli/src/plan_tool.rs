//! `anaden-tool plan-tenmei` / `plan-expand` (Issue #248 offline)。
//!
//! - **plan-tenmei**: 上げたいキャラ名から `data/ad_tenmei.toml`
//!   ([`crate::plan_data::TenmeiDoc`]) を逆引きし、計画 TOML
//!   (`ticket_digest_plan.toml` 形式) を生成する
//!   (設計 Wiki `Daily-Automation-ticket-digest` §6)。
//! - **plan-expand**: 計画 TOML を routine TOML へ**静的展開**する
//!   (設計 §7「1 ダンジョン 1 サイクル」「engine 変更不要」)。
//!   エントリ 1 件 = routine ステップ 1 件に展開し、生成物は
//!   `anaden_engine::load_routine` + `RoutineDef::validate` でそのまま検証できる。
//!
//! ループ・パラメタ化は engine に足さない (engine 変更不要の設計契約)。
//! per-dungeon の FSM 差し替え (difficulty 選択・skip_btn 等) は live テンプレート
//! 収集後の pipeline 拡張で扱う — 展開された各ステップは pipeline manifest の
//! `start_task` (単一情報源) から開始する。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anaden_engine::{OnFailure, RoutineDef, RoutineStep};

use crate::plan_data::{
    Difficulty, OnTicketEmpty, PlanDoc, PlanEntry, SlotAlignment, TenmeiChar, TenmeiDoc,
    TenmeiDungeon, load_plan, load_tenmei, render_plan_toml, write_text_file,
};

/// 生成する routine 名 (evidence・履歴の識別子)。
const ROUTINE_NAME: &str = "ticket_digest";
/// 展開ステップのループ間隔 (秒)。
///
/// v0 pipeline の goal 安全弁 `Timeout(600)` と揃えるため
/// interval 2s × max_iters 300 = 600s としている (FSM 拡張時に再調整)。
const STEP_INTERVAL_SECS: u64 = 2;
/// 展開ステップの最大サイクル数 (上記コメント参照)。
const STEP_MAX_ITERS: u64 = 300;

/// ダンジョン逆引き 1 件の判定結果 (生成ループの分岐をここに集約)。
enum DungeonLookup<'a> {
    /// 計画に含める (ダンジョン行を返す)。
    Include(&'a TenmeiDungeon),
    /// `[[dungeon]]` 表に存在しない (スキップ対象 AD 一覧の外)。
    Unknown,
    /// スキップ対象外 (`skip_target = false`)。
    NotSkippable,
    /// 同一 (ダンジョン, 難易度) が上位優先度で既に登録済み。
    Duplicate,
}

/// ダンジョン名の逆引き判定 (純関数)。
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

/// キャラ行とダンジョン行から `note` を組み立てる。
///
/// wiki 雛形の記法 (`ヘレナ冥 / VH報酬枠=天`) に揃える。
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

/// キャラ名列 (優先度順) から計画を逆引き生成する。
///
/// 判定: キャラは `[[char]].name` と完全一致 (不一致は fail-closed)。
/// ダンジョンは `skip_target = true` のみ採用し、`[[dungeon]]` 表に無いものと
/// 同一 (ダンジョン, 難易度) の重複は除外して `notes` に記録する。
///
/// # Errors
/// キャラ名が存在しない場合、および除外の結果エントリ 0 件になる場合。
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

/// 計画 TOML を routine 定義へ静的展開する (エントリ 1 件 = ステップ 1 件)。
///
/// 設計 §6 の突合契約: 各 `entry.dungeon` は `ad_tenmei.toml` `[[dungeon]].name`
/// と突合し、未知・`skip_target = false`・(ダンジョン, 難易度) 重複は
/// fail-closed にエラー (手書き編集された計画の誤りを実行前に止める)。
///
/// # Errors
/// 上記突合違反、およびエントリ 0 件の計画。
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

/// 生成 routine TOML のヘッダコメント (出所と v0 パラメタの根拠)。
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

/// この crate から見た workspace ルート。
///
/// main.rs `cli_workspace_root` と同式 (bin をまたいで共有できないための
/// 重複定義 — 値はコンパイル時に同一)。
fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

/// `plan-tenmei` サブコマンド本体。
///
/// TOML 成果物は `out` 未指定時 stdout・指定時ファイルへ。経過メッセージは
/// stderr (stdout の TOML がパイプでそのまま使えるようにする)。
///
/// # Errors
/// データ読込・逆引き失敗 (未知キャラ等)・書込失敗。
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

/// `plan-expand` サブコマンド本体。
///
/// `start_task` は pipeline manifest (`pipeline.toml`) の宣言値を単一情報源として
/// 採用する (FSM 拡張で manifest が書き換われば再生成で追従)。
/// 生成 routine は `anaden routine <file>` でそのまま実行可能。
///
/// # Errors
/// 計画・天冥データの読込失敗、pipeline 解決・manifest 読込失敗、
/// 展開の突合違反、書込失敗。
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
