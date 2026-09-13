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

/// Routine UI パネルの状態。
#[derive(Debug, Clone)]
pub struct RoutinePanel {
    /// 発見した routine 一覧 (ファイル名 stem と絶対パス・名前順)。
    discovered: Vec<(String, PathBuf)>,
    /// 選択中の routine stem。未選択は None。
    selected: Option<String>,
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
#[must_use]
pub fn build_routine_args(routine_path: &Path) -> Vec<String> {
    let path = if routine_path.is_absolute() {
        routine_path.to_path_buf()
    } else {
        workspace_root().join(routine_path)
    };
    vec!["routine".to_string(), path.to_string_lossy().into_owned()]
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
        let args = build_routine_args(&abs);
        assert_eq!(args[0], "routine");
        assert!(Path::new(&args[1]).is_absolute(), "{}", args[1]);
        assert!(args[1].ends_with("daily.toml"), "{}", args[1]);
    }

    #[test]
    fn build_routine_args_from_relative_resolves_workspace_root() {
        let args = build_routine_args(Path::new("templates/routines/daily.toml"));
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
        let args = build_routine_args(&daily);
        assert!(Path::new(&args[1]).is_file(), "{}", args[1]);
    }
}
