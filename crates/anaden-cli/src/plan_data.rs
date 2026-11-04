use std::path::Path;

use serde::{Deserialize, Serialize};

const TENMEI_VERSION: u32 = 1;
const PLAN_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Difficulty {
    #[serde(rename = "H")]
    Hard,
    #[serde(rename = "VH")]
    VeryHard,
}

impl Difficulty {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum SlotAlignment {
    #[serde(rename = "ten")]
    Ten,
    #[serde(rename = "mei")]
    Mei,
    #[serde(rename = "both")]
    Both,
    #[serde(rename = "all_dist")]
    AllDist,
}

impl SlotAlignment {
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

fn none<T>() -> Option<T> {
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DifficultyTable<T> {
    #[serde(rename = "H", default = "none")]
    pub hard: Option<T>,
    #[serde(rename = "VH", default = "none")]
    pub very_hard: Option<T>,
}

impl<T> DifficultyTable<T> {
    #[must_use]
    pub fn get(&self, difficulty: Difficulty) -> Option<&T> {
        match difficulty {
            Difficulty::Hard => self.hard.as_ref(),
            Difficulty::VeryHard => self.very_hard.as_ref(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RewardSlots {
    pub vh: Vec<u32>,
    pub hard: Vec<u32>,
    pub iseki_vh: Vec<u32>,
    pub iseki_hard: Vec<u32>,
    pub gallerea: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TenmeiDoc {
    pub version: u32,
    pub source: String,
    pub reward_slots: RewardSlots,
    #[serde(rename = "dungeon")]
    pub dungeons: Vec<TenmeiDungeon>,
    #[serde(rename = "char")]
    pub chars: Vec<TenmeiChar>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TenmeiDungeon {
    pub name: String,
    pub category: String,
    pub skip_target: bool,
    pub key: DifficultyTable<String>,
    pub alignment: DifficultyTable<SlotAlignment>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TenmeiChar {
    pub name: String,
    pub alignment: SlotAlignment,
    pub dungeons: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OnTicketEmpty {
    #[serde(rename = "stop")]
    Stop,
    #[serde(rename = "normal_entry")]
    NormalEntry,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanDoc {
    pub version: u32,
    pub on_ticket_empty: OnTicketEmpty,
    pub auto_buy_key_with_stone: bool,
    #[serde(rename = "entry")]
    pub entries: Vec<PlanEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanEntry {
    pub dungeon: String,
    pub difficulty: Difficulty,
    pub priority: u32,
    pub note: String,
}

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

const PLAN_HEADER: &str = "# ticket_digest 実行計画 (anaden-tool plan-tenmei 生成)\n\
     #\n\
     # 消費順: priority の小さい順。difficulty のキーを 1 枚ずつ消費。\n\
     # 再生成: anaden-tool plan-tenmei <キャラ名>...\n\
     # routine 化: anaden-tool plan-expand <このファイル> [--out <routine.toml>]";

pub fn render_plan_toml(doc: &PlanDoc) -> anyhow::Result<String> {
    let body = toml::to_string(doc).map_err(|e| anyhow::anyhow!("計画 TOML 化に失敗: {e}"))?;
    Ok(format!("{PLAN_HEADER}\n\n{body}"))
}

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
