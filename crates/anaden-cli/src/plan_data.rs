//! ticket_digest 計画データのスキーマと読み書き (Issue #248 offline)。
//!
//! - `data/ad_tenmei.toml`: アナダン天冥データ (GitHub Wiki
//!   `daily-automation-data/ad_tenmei.toml` 由来。ダンジョン定義 +
//!   クリア時天冥アップキャラ対応)。
//! - `data/ticket_digest_plan.toml`: ねこの手特急券の消化計画
//!   (設計: GitHub Wiki `Daily-Automation-ticket-digest` §6)。
//!
//! 両ファイルとも wiki を上流とする派生データのため `deny_unknown_fields` で
//! 厳格にパースし、上流スキーマの drift を fail-closed に検出する
//! (黙って未知フィールドを落として古い意味で動き続けない)。

use std::path::Path;

use serde::{Deserialize, Serialize};

/// 天冥データ (`ad_tenmei.toml`) の想定スキーマ版。
const TENMEI_VERSION: u32 = 1;
/// 計画 TOML (`ticket_digest_plan.toml`) の想定スキーマ版。
const PLAN_VERSION: u32 = 1;

/// 消費難易度 (TOML では `H` / `VH`)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Difficulty {
    /// ハード (グリーンキー消費。異境はグリーン 2 枚)。
    #[serde(rename = "H")]
    Hard,
    /// ベリーハード (レッドキー消費)。
    #[serde(rename = "VH")]
    VeryHard,
}

impl Difficulty {
    /// 生成ファイルの記述・表示に使う文字列 (TOML 値と同一)。
    #[must_use]
    pub fn as_toml_str(self) -> &'static str {
        match self {
            Self::Hard => "H",
            Self::VeryHard => "VH",
        }
    }
}

impl std::fmt::Display for Difficulty {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_toml_str())
    }
}

impl std::str::FromStr for Difficulty {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "H" => Ok(Self::Hard),
            "VH" => Ok(Self::VeryHard),
            other => Err(format!(
                "difficulty は `H` または `VH` です (指定値: {other})"
            )),
        }
    }
}

/// 報酬枠の天/冥 alignment (上流データの `ten` / `mei` / `both` / `all_dist`)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum SlotAlignment {
    /// 天側。
    #[serde(rename = "ten")]
    Ten,
    /// 冥側。
    #[serde(rename = "mei")]
    Mei,
    /// 天冥両方。
    #[serde(rename = "both")]
    Both,
    /// 全配布キャラの天冥がアップ (秘境 etc.)。
    #[serde(rename = "all_dist")]
    AllDist,
}

impl SlotAlignment {
    /// 計画 `note` 向けの日本語ラベル。
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::Ten => "天",
            Self::Mei => "冥",
            Self::Both => "天冥両方",
            Self::AllDist => "全配布キャラ",
        }
    }
}

/// 難易度別インラインテーブル (`key = { H = "..", VH = ".." }` 等の形状)。
///
/// 上流データには `key = {}` (キー消費なしの秘境) や
/// `alignment = { VH = "all_dist" }` (H 側欠落) が実在するため、
/// メンバーは欠落を許す `Option` にする。
/// `default = "..."` 用の汎用 None コンストラクタ
/// (serde の attribute path には variant `None` を直接書けないため)。
fn none<T>() -> Option<T> {
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DifficultyTable<T> {
    /// `H` 側の値。
    #[serde(rename = "H", default = "none")]
    pub hard: Option<T>,
    /// `VH` 側の値。
    #[serde(rename = "VH", default = "none")]
    pub very_hard: Option<T>,
}

impl<T> DifficultyTable<T> {
    /// 指定難易度側の値を取り出す。
    #[must_use]
    pub fn get(&self, difficulty: Difficulty) -> Option<&T> {
        match difficulty {
            Difficulty::Hard => self.hard.as_ref(),
            Difficulty::VeryHard => self.very_hard.as_ref(),
        }
    }
}

/// 報酬枠解放に必要な天冥値 (上流データ `[reward_slots]`。人間向け参照値)。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RewardSlots {
    /// ベリーハード枠の解放天冥値。
    pub vh: Vec<u32>,
    /// ハード枠の解放天冥値。
    pub hard: Vec<u32>,
    /// 異境ベリーハード枠。
    pub iseki_vh: Vec<u32>,
    /// 異境ハード枠。
    pub iseki_hard: Vec<u32>,
    /// ガルレア大陸 (5 枠)。
    pub gallerea: Vec<u32>,
}

/// `ad_tenmei.toml` 全体。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TenmeiDoc {
    /// スキーマ版。
    pub version: u32,
    /// データの出典 URL。
    pub source: String,
    /// 報酬枠解放天冥値 (参照用)。
    pub reward_slots: RewardSlots,
    /// ダンジョン一覧 (`[[dungeon]]`)。
    #[serde(rename = "dungeon")]
    pub dungeons: Vec<TenmeiDungeon>,
    /// 天冥アップキャラ対応 (`[[char]]`)。
    #[serde(rename = "char")]
    pub chars: Vec<TenmeiChar>,
}

/// `[[dungeon]]` 1 エントリ。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TenmeiDungeon {
    /// ダンジョン名 (計画 `entry.dungeon` と突合する識別子)。
    pub name: String,
    /// 分類 (`main` / `side` / `iseki` / `hiseki` 等)。
    pub category: String,
    /// ねこの手特急券のスキップ対象か。
    pub skip_target: bool,
    /// 難易度別キー色 (`green` / `red` / `green2`。参照用)。
    pub key: DifficultyTable<String>,
    /// 難易度別報酬枠 alignment。
    pub alignment: DifficultyTable<SlotAlignment>,
}

/// `[[char]]` 1 エントリ (上げたいキャラ → ダンジョンの逆引き表)。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TenmeiChar {
    /// キャラ名 (plan-tenmei の引数と完全一致させる)。
    pub name: String,
    /// このキャラの上がる側 (天 / 冥)。
    pub alignment: SlotAlignment,
    /// 天冥がアップするダンジョン名のリスト (記載順 = 逆引きの優先順)。
    pub dungeons: Vec<String>,
}

/// 特急券が尽きた時の挙動 (計画 TOML `on_ticket_empty`)。
///
/// 消費は live FSM 実装 (Issue #248 実機待ち) の契約。offline の段では
/// 計画ファイルへの round-trip のみ保証する。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OnTicketEmpty {
    /// pipeline を終了する (既定・推奨)。
    #[serde(rename = "stop")]
    Stop,
    /// 通常入場へ切替 (戦闘が発生するため別 pipeline 前提・非推奨)。
    #[serde(rename = "normal_entry")]
    NormalEntry,
}

/// 計画 TOML (`ticket_digest_plan.toml`) 全体。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanDoc {
    /// スキーマ版。
    pub version: u32,
    /// 特急券切れ時の挙動。
    pub on_ticket_empty: OnTicketEmpty,
    /// クロノス石でのキー自動購入を行うか (既定 false = 買わない)。
    pub auto_buy_key_with_stone: bool,
    /// 消化エントリ (`[[entry]]`。priority 小さい順に消化)。
    #[serde(rename = "entry")]
    pub entries: Vec<PlanEntry>,
}

/// 計画 `[[entry]]` 1 エントリ (= 1 ダンジョン周回サイクル)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanEntry {
    /// ダンジョン名 (`ad_tenmei.toml` `[[dungeon]].name` と突合)。
    pub dungeon: String,
    /// 消費難易度。
    pub difficulty: Difficulty,
    /// 消費順 (小さい順)。
    pub priority: u32,
    /// 人間向けメモ (どのキャラの天冥を狙っているか等)。
    pub note: String,
}

/// `ad_tenmei.toml` を読み込む (スキーマ版チェック込み・fail-closed)。
///
/// # Errors
/// ファイル読込失敗・TOML パース失敗 (未知フィールド・型不一致含む)・
/// スキーマ版不一致を anyhow で返す。
pub fn load_tenmei(path: &Path) -> anyhow::Result<TenmeiDoc> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("天冥データ読込失敗 {}: {e}", path.display()))?;
    let doc: TenmeiDoc = toml::from_str(&text)
        .map_err(|e| anyhow::anyhow!("天冥データパース失敗 {}: {e}", path.display()))?;
    if doc.version != TENMEI_VERSION {
        anyhow::bail!(
            "天冥データのスキーマ版が未対応です: {} (対応版 {TENMEI_VERSION})",
            doc.version
        );
    }
    Ok(doc)
}

/// 計画 TOML を読み込む (スキーマ版チェック込み・fail-closed)。
///
/// # Errors
/// ファイル読込失敗・TOML パース失敗 (未知フィールド・型不一致含む)・
/// スキーマ版不一致を anyhow で返す。
pub fn load_plan(path: &Path) -> anyhow::Result<PlanDoc> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("計画 TOML 読込失敗 {}: {e}", path.display()))?;
    let doc: PlanDoc = toml::from_str(&text)
        .map_err(|e| anyhow::anyhow!("計画 TOML パース失敗 {}: {e}", path.display()))?;
    if doc.version != PLAN_VERSION {
        anyhow::bail!(
            "計画 TOML のスキーマ版が未対応です: {} (対応版 {PLAN_VERSION})",
            doc.version
        );
    }
    Ok(doc)
}

/// 生成する計画 TOML に付けるヘッダコメント (生成物の自己記述)。
const PLAN_HEADER: &str = "# ticket_digest 実行計画 (anaden-tool plan-tenmei 生成)\n\
     #\n\
     # 消費順: priority の小さい順。difficulty のキーを 1 枚ずつ消費。\n\
     # 再生成: anaden-tool plan-tenmei <キャラ名>...\n\
     # routine 化: anaden-tool plan-expand <このファイル> [--out <routine.toml>]";

/// [`PlanDoc`] をヘッダコメント付き TOML 文字列へ整形する。
///
/// # Errors
/// TOML 化に失敗した場合 (通常到達しない) のみ anyhow で返す。
pub fn render_plan_toml(doc: &PlanDoc) -> anyhow::Result<String> {
    let body = toml::to_string(doc).map_err(|e| anyhow::anyhow!("計画 TOML 化に失敗: {e}"))?;
    Ok(format!("{PLAN_HEADER}\n\n{body}"))
}

/// テキストをファイルへ書き込む (親ディレクトリを自動作成)。
///
/// # Errors
/// ディレクトリ作成・書込の io エラーを anyhow で返す。
pub fn write_text_file(path: &Path, text: &str) -> anyhow::Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .map_err(|e| anyhow::anyhow!("ディレクトリ作成失敗 {}: {e}", parent.display()))?;
    }
    std::fs::write(path, text).map_err(|e| anyhow::anyhow!("書込失敗 {}: {e}", path.display()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// この crate から見た workspace ルート (main.rs `cli_workspace_root` と同式)。
    fn workspace_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
    }

    fn real_tenmei_path() -> PathBuf {
        workspace_root().join("data").join("ad_tenmei.toml")
    }

    fn real_plan_path() -> PathBuf {
        workspace_root()
            .join("data")
            .join("ticket_digest_plan.toml")
    }

    // ---- コミット済み実データのパース (wiki 同期契約) ----

    #[test]
    fn loads_committed_tenmei_data() {
        let doc = load_tenmei(&real_tenmei_path()).expect("real ad_tenmei.toml must parse");
        assert_eq!(
            doc.dungeons.len(),
            40,
            "dungeon count must match wiki source"
        );
        assert_eq!(doc.chars.len(), 58, "char count must match wiki source");
        assert_eq!(doc.reward_slots.vh, vec![24, 60, 120]);
        let mig = doc
            .dungeons
            .iter()
            .find(|d| d.name == "ミグランス城")
            .expect("ミグランス城");
        assert!(mig.skip_target);
        assert_eq!(
            mig.alignment.get(Difficulty::VeryHard),
            Some(&SlotAlignment::Ten)
        );
    }

    #[test]
    fn loads_committed_plan_template() {
        let doc = load_plan(&real_plan_path()).expect("real ticket_digest_plan.toml must parse");
        assert_eq!(doc.version, 1);
        assert_eq!(doc.on_ticket_empty, OnTicketEmpty::Stop);
        assert!(!doc.auto_buy_key_with_stone);
        let names: Vec<&str> = doc.entries.iter().map(|e| e.dungeon.as_str()).collect();
        assert_eq!(names, ["ミグランス城", "ゼノ・ドメイン", "次元戦艦"]);
        assert!(
            doc.entries
                .iter()
                .all(|e| e.difficulty == Difficulty::VeryHard)
        );
    }

    // ---- fail-closed パース ----

    #[test]
    fn plan_rejects_unknown_field() {
        let err = toml::from_str::<PlanDoc>(
            "version = 1\non_ticket_empty = \"stop\"\nauto_buy_key_with_stone = false\nunknown = 1\n",
        )
        .expect_err("unknown field must fail");
        assert!(format!("{err}").contains("unknown"), "{err}");
    }

    #[test]
    fn plan_rejects_unknown_difficulty() {
        let err = toml::from_str::<PlanEntry>(
            "dungeon = \"x\"\ndifficulty = \"N\"\npriority = 1\nnote = \"n\"\n",
        )
        .expect_err("unknown difficulty must fail");
        assert!(format!("{err}").contains("difficulty"), "{err}");
    }

    #[test]
    fn tenmei_rejects_version_drift() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("tenmei.toml");
        std::fs::write(
            &path,
            "version = 2\nsource = \"s\"\ndungeon = []\nchar = []\n\
             [reward_slots]\nvh = []\nhard = []\niseki_vh = []\niseki_hard = []\ngallerea = []\n",
        )
        .unwrap();
        let err = load_tenmei(&path).expect_err("version drift must fail");
        assert!(format!("{err}").contains("スキーマ版"), "{err}");
    }

    // ---- 生成 TOML の round-trip ----

    #[test]
    fn rendered_plan_roundtrips_through_toml_parse() {
        let plan = PlanDoc {
            version: 1,
            on_ticket_empty: OnTicketEmpty::Stop,
            auto_buy_key_with_stone: false,
            entries: vec![PlanEntry {
                dungeon: "ミグランス城".to_string(),
                difficulty: Difficulty::VeryHard,
                priority: 1,
                note: "アルド天 / VH報酬枠=天".to_string(),
            }],
        };
        let text = render_plan_toml(&plan).unwrap();
        assert!(text.contains("anaden-tool plan-tenmei"), "{text}");
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("plan.toml");
        write_text_file(&path, &text).unwrap();
        let back = load_plan(&path).unwrap();
        assert_eq!(back, plan);
    }

    /// `key = {}` (秘境) と `alignment = { VH = ... }` (H 欠落) の実在形状を
    /// 受け付ける (上流データ トト・シアターワールドの形)。
    #[test]
    fn tenmei_accepts_empty_key_and_partial_alignment() {
        let doc: TenmeiDoc = toml::from_str(
            "version = 1\nsource = \"s\"\n\
             [reward_slots]\nvh = []\nhard = []\niseki_vh = []\niseki_hard = []\ngallerea = []\n\
             [[dungeon]]\nname = \"秘境\"\ncategory = \"hiseki\"\nskip_target = false\nkey = {}\n\
             alignment = { VH = \"all_dist\" }\n\
             [[char]]\nname = \"誰か\"\nalignment = \"ten\"\ndungeons = []",
        )
        .expect("hiseki shape must parse");
        let hiseki = doc.dungeons.first().unwrap();
        assert_eq!(hiseki.key.get(Difficulty::Hard), None);
        assert_eq!(
            hiseki.alignment.get(Difficulty::VeryHard),
            Some(&SlotAlignment::AllDist)
        );
    }
}
