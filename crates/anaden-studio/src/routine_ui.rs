//! Routine 選択・実行 UI (Issue #199)。
//!
//! `templates/routines/*.toml` を離散スキャンして ComboBox で選択させ、
//! 選択された routine を `anaden routine <path>` 子プロセスとして起動する
//! 引数列を組み立てる。実行・履歴記録は既存 runner 経路
//! ([`crate::runner_exec::PipelineRunnerApp::start_pipeline`]) を再利用する
//! (二重の子プロセス管理を持たない)。
//!
//! 状態操作 (discovery・args 組立) は egui 非依存の純関数/メソッドに切り出し
//! ヘッドレスでユニットテスト可能にしている (strategy_ui.rs と同一パターン)。

use std::path::{Path, PathBuf};

use egui::Ui;

/// routine 定義ディレクトリ (workspace ルート相対)。
const ROUTINES_DIR: &str = "templates/routines";

/// evidence run-id のプレフィックス (Issue #202 UC-1)。
const EVIDENCE_RUN_ID_PREFIX: &str = "routine";

/// Routine UI パネルの状態。
#[derive(Debug, Clone)]
pub struct RoutinePanel {
    /// 発見した routine 一覧 (ファイル名 stem と絶対パス・名前順)。
    discovered: Vec<(String, PathBuf)>,
    /// 選択中の routine stem。未選択は None。
    selected: Option<String>,
    /// evidence 採取チェック (既定 ON・Issue #202 UC-1)。
    /// ON 時の spawn 引数に `--evidence-run-id routine-<timestamp>` が付く。
    evidence_enabled: bool,
}

impl Default for RoutinePanel {
    fn default() -> Self {
        Self::new(&workspace_root())
    }
}

impl RoutinePanel {
    /// workspace ルート基準で `templates/routines/*.toml` をスキャンして構築する。
    #[must_use]
    pub fn new(root: &Path) -> Self {
        Self {
            discovered: discover_routines(root),
            selected: None,
            evidence_enabled: true,
        }
    }

    /// 発見した routine の (stem, パス) 一覧。
    #[must_use]
    pub fn routines(&self) -> &[(String, PathBuf)] {
        &self.discovered
    }

    /// 選択中の routine パス (未選択は None)。
    #[must_use]
    pub fn selected_path(&self) -> Option<&Path> {
        let sel = self.selected.as_ref()?;
        self.discovered
            .iter()
            .find(|(name, _)| name == sel)
            .map(|(_, p)| p.as_path())
    }

    /// 選択中の routine stem (履歴ラベル用・未選択は None)。
    #[must_use]
    pub fn selected_name(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    /// routine を選択する (未発見の名前は無視 — UI は発見一覧のみ提示)。
    pub fn select(&mut self, name: &str) {
        if self.discovered.iter().any(|(n, _)| n == name) {
            self.selected = Some(name.to_string());
        }
    }

    /// evidence 採取チェックの現在値 (Issue #202 UC-1・既定 ON)。
    #[must_use]
    pub fn evidence_enabled(&self) -> bool {
        self.evidence_enabled
    }

    /// evidence 採取チェックを切り替える (ヘッドレス等価・テスト用)。
    pub fn set_evidence_enabled(&mut self, enabled: bool) {
        self.evidence_enabled = enabled;
    }

    /// パネルを描画する。戻り値は「選択状態が変化した」フラグ。
    pub fn ui(&mut self, ui: &mut Ui) -> bool {
        let mut changed = false;
        if self.discovered.is_empty() {
            ui.weak(format!(
                "routine が見つかりません ({ROUTINES_DIR}/*.toml を配置してください)"
            ));
            return changed;
        }
        let selected_label = self
            .selected
            .clone()
            .unwrap_or_else(|| "（未選択）".to_string());
        let entries: Vec<String> = self
            .discovered
            .iter()
            .map(|(name, _)| name.clone())
            .collect();
        egui::ComboBox::from_id_salt("routine_select")
            .selected_text(selected_label)
            .show_ui(ui, |ui| {
                ui.set_min_width(160.0);
                for name in &entries {
                    let is_sel = self.selected.as_deref() == Some(name.as_str());
                    if ui.selectable_label(is_sel, name).clicked() {
                        self.select(name);
                        changed = true;
                    }
                }
            });
        // evidence 採取チェック (Issue #202 UC-1・既定 ON)。ON 時の spawn に
        // --evidence-run-id が付く (履歴詳細から evidence ディレクトリを追跡可能)。
        ui.checkbox(
            &mut self.evidence_enabled,
            "evidence 採取 (.omc/logs/ へ証跡保存)",
        );
        changed
    }

    /// ルーチンタブの選択状態サマリ (runner 戦略サマリと同形式の純関数)。
    #[must_use]
    pub fn summary(&self) -> String {
        match self.selected_name() {
            Some(name) => format!("routine={name}"),
            None => "ルーチン未選択".to_string(),
        }
    }
}

/// `templates/routines/*.toml` をスキャンして (stem, 絶対パス) の名前順一覧を返す。
///
/// ディレクトリが存在しない場合は空 Vec (GUI は案内表示のみ・エラーにしない)。
#[must_use]
pub fn discover_routines(root: &Path) -> Vec<(String, PathBuf)> {
    let dir = root.join(ROUTINES_DIR);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out: Vec<(String, PathBuf)> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .and_then(|x| x.to_str())
                .is_some_and(|x| x.eq_ignore_ascii_case("toml"))
        })
        .filter_map(|p| {
            let stem = p.file_stem()?.to_string_lossy().into_owned();
            Some((stem, p))
        })
        .collect();
    out.sort();
    out
}

/// 選択された routine を起動する `anaden` CLI 引数列 (サブコマンド以降)。
///
/// パスは絶対化して渡す (子プロセス側 cwd 依存の解決を回避 —
/// `resolve_pipeline_arg` と同一の動機)。
///
/// `evidence_run_id` に [`Some`] を渡すと `--evidence-run-id <run-id>` を付与する
/// (Issue #202 UC-1・GUI の evidence 採取チェック ON)。run-id 文字列は呼出側で
/// 生成する (テストでは固定値を注入でき、実運用では [`new_evidence_run_id`])。
#[must_use]
pub fn build_routine_args(routine_path: &Path, evidence_run_id: Option<&str>) -> Vec<String> {
    let path = if routine_path.is_absolute() {
        routine_path.to_path_buf()
    } else {
        workspace_root().join(routine_path)
    };
    let mut args = vec!["routine".to_string(), path.to_string_lossy().into_owned()];
    if let Some(run_id) = evidence_run_id {
        args.push("--evidence-run-id".to_string());
        args.push(run_id.to_string());
    }
    args
}

/// evidence run-id (`routine-<YYYYMMDD-HHMMSS>`) をタイムスタンプから組む純関数。
///
/// timestamp は引数注入のためテスト決定的 (実運用は [`new_evidence_run_id`])。
#[must_use]
pub fn evidence_run_id(timestamp: &str) -> String {
    format!("{EVIDENCE_RUN_ID_PREFIX}-{timestamp}")
}

/// 現在時刻から evidence run-id を生成する (spawn 実行時に呼ぶ)。
#[must_use]
pub fn new_evidence_run_id() -> String {
    let unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    evidence_run_id(&compact_utc_timestamp(unix))
}

/// Unix 秒を UTC `YYYYMMDD-HHMMSS` へ変換する (chrono 非依存の最小実装)。
///
/// 日付変換は civil-from-days (Howard Hinnant) の手書き変換で、
/// `anaden-cli` e2e::iso8601_utc と同一アルゴリズム (既知値テストで検証)。
fn compact_utc_timestamp(unix_secs: u64) -> String {
    let days = (unix_secs / 86_400) as i64;
    let secs_of_day = unix_secs % 86_400;
    // civil_from_days: days=1970-01-01 起点の経過日 → y/m/d
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}{m:02}{d:02}-{:02}{:02}{:02}",
        secs_of_day / 3600,
        (secs_of_day % 3600) / 60,
        secs_of_day % 60
    )
}

/// コンパイル時に確定する workspace ルート (anaden-studio manifest から 2 階層上昇)。
fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    fn real_root() -> PathBuf {
        workspace_root()
    }

    // ---- discover_routines ----

    #[test]
    fn discover_finds_sample_daily_on_real_root() {
        let found = discover_routines(&real_root());
        assert!(
            found.iter().any(|(name, _)| name == "daily"),
            "daily.toml must be discovered: {found:?}"
        );
    }

    #[test]
    fn discover_missing_dir_returns_empty() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(discover_routines(tmp.path()).is_empty());
    }

    #[test]
    fn discover_ignores_non_toml_files() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(ROUTINES_DIR);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.toml"), b"name='a'").unwrap();
        std::fs::write(dir.join("b.txt"), b"not a routine").unwrap();
        let found = discover_routines(tmp.path());
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].0, "a");
    }

    // ---- RoutinePanel 状態操作 (egui 非依存) ----

    #[test]
    fn panel_defaults_discover_real_root_and_select_none() {
        let panel = RoutinePanel::default();
        assert!(panel.selected_name().is_none());
        assert!(panel.selected_path().is_none());
        assert_eq!(panel.summary(), "ルーチン未選択");
    }

    #[test]
    fn select_and_reject_unknown_routine() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(ROUTINES_DIR);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("morning.toml"), b"name='m'").unwrap();
        let mut panel = RoutinePanel::new(tmp.path());
        assert_eq!(panel.routines().len(), 1);

        panel.select("ghost");
        assert!(panel.selected_name().is_none(), "unknown is ignored");

        panel.select("morning");
        assert_eq!(panel.selected_name(), Some("morning"));
        let path = panel.selected_path().unwrap();
        assert!(path.to_string_lossy().contains("morning.toml"));
        assert_eq!(panel.summary(), "routine=morning");
    }

    // ---- build_routine_args (子プロセス引数契約) ----

    #[test]
    fn build_routine_args_from_absolute_path() {
        let abs = real_root().join(ROUTINES_DIR).join("daily.toml");
        let args = build_routine_args(&abs, None);
        assert_eq!(args[0], "routine");
        assert!(Path::new(&args[1]).is_absolute(), "{}", args[1]);
        assert!(args[1].ends_with("daily.toml"), "{}", args[1]);
    }

    #[test]
    fn build_routine_args_from_relative_resolves_workspace_root() {
        let args = build_routine_args(Path::new("templates/routines/daily.toml"), None);
        assert_eq!(args[0], "routine");
        let p = Path::new(&args[1]);
        assert!(p.is_absolute(), "{}", args[1]);
        assert!(
            p.ends_with(Path::new("templates").join("routines").join("daily.toml")),
            "must resolve under workspace templates: {}",
            args[1]
        );
    }

    /// 実在 daily.toml から組んだ引数のパスが実ファイルを指すこと (spawn 前提の保証)。
    #[test]
    fn build_routine_args_points_at_existing_daily_file() {
        let panel = RoutinePanel::default();
        let daily = panel
            .routines()
            .iter()
            .find(|(name, _)| name == "daily")
            .map(|(_, p)| p.clone())
            .unwrap_or_else(|| panic!("daily.toml must exist at real root"));
        let args = build_routine_args(&daily, None);
        assert!(Path::new(&args[1]).is_file(), "{}", args[1]);
    }

    // ---- evidence 採取 (Issue #202 UC-1) ----

    /// チェック ON/OFF の args 契約: ON は `--evidence-run-id routine-<ts>` 付与・
    /// OFF は `routine <path>` のみ (run-id は timestamp 注入で決定的)。
    #[test]
    fn build_routine_args_evidence_on_off_contract() {
        let daily = real_root().join(ROUTINES_DIR).join("daily.toml");

        // ON: フラグ + run-id がパスの後ろに付く。
        let run_id = evidence_run_id("20260914-013000");
        assert_eq!(run_id, "routine-20260914-013000");
        let args_on = build_routine_args(&daily, Some(&run_id));
        assert_eq!(args_on[0], "routine");
        let flag_at = args_on
            .iter()
            .position(|a| a == "--evidence-run-id")
            .unwrap_or_else(|| panic!("flag missing: {args_on:?}"));
        assert!(flag_at >= 2, "flag must come after path: {args_on:?}");
        assert_eq!(args_on[flag_at + 1], "routine-20260914-013000");
        assert_eq!(args_on.len(), 4, "{args_on:?}");

        // OFF: フラグなし (routine <path> のみ)。
        let args_off = build_routine_args(&daily, None);
        assert!(!args_off.iter().any(|a| a == "--evidence-run-id"));
        assert_eq!(args_off.len(), 2, "{args_off:?}");
    }

    /// パネルの evidence チェックは既定 ON・set_evidence_enabled で切替可能。
    /// (Issue #204 以降、run-id の発行は runner_exec::RunRequest::build_spawn に
    /// 集約されているため、パネルはチェック状態の保持のみ担う。)
    #[test]
    fn panel_evidence_check_defaults_on_and_toggles() {
        let mut panel = RoutinePanel::default();
        assert!(panel.evidence_enabled(), "default must be ON");
        panel.set_evidence_enabled(false);
        assert!(!panel.evidence_enabled());
        panel.set_evidence_enabled(true);
        assert!(panel.evidence_enabled());
    }

    /// compact_utc_timestamp の既知値検証 (civil-from-days 手書き変換の保証)。
    #[test]
    fn compact_utc_timestamp_known_values() {
        assert_eq!(compact_utc_timestamp(0), "19700101-000000");
        // 2000-01-01T00:00:00Z
        assert_eq!(compact_utc_timestamp(946_684_800), "20000101-000000");
        // 2001-09-09T01:46:40Z (unix 1000000000 の既知値)
        assert_eq!(compact_utc_timestamp(1_000_000_000), "20010909-014640");
        // 2024-02-29T12:34:56Z (うるう年・'YYYYMMDD-HHMMSS' 形式の桁揃え)
        assert_eq!(compact_utc_timestamp(1_709_210_096), "20240229-123456");
    }

    /// new_evidence_run_id は `routine-<15桁タイムスタンプ>` 形式を満たす。
    #[test]
    fn new_evidence_run_id_shape() {
        let id = new_evidence_run_id();
        let rest = id
            .strip_prefix("routine-")
            .unwrap_or_else(|| panic!("{id}"));
        // YYYYMMDD-HHMMSS = 8 + 1 + 6 桁。
        assert_eq!(rest.len(), 15, "{id}");
        assert_eq!(rest.as_bytes()[8], b'-', "{id}");
        assert!(
            rest.bytes().all(|b| b.is_ascii_digit() || b == b'-'),
            "{id}"
        );
    }
}
