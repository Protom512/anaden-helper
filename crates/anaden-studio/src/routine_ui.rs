use std::path::{Path, PathBuf};

use egui::Ui;

const ROUTINES_DIR: &str = "templates/routines";

const EVIDENCE_RUN_ID_PREFIX: &str = "routine";

#[derive(Debug, Clone)]
pub struct RoutinePanel {
    discovered: Vec<(String, PathBuf)>,
    selected: Option<String>,
    evidence_enabled: bool,
}

impl Default for RoutinePanel {
    fn default() -> Self {
        Self::new(&workspace_root())
    }
}

impl RoutinePanel {
    #[must_use]
    pub fn new(root: &Path) -> Self {
        Self {
            discovered: discover_routines(root),
            selected: None,
            evidence_enabled: true,
        }
    }

    #[must_use]
    pub fn routines(&self) -> &[(String, PathBuf)] {
        &self.discovered
    }

    #[must_use]
    pub fn selected_path(&self) -> Option<&Path> {
        let sel = self.selected.as_ref()?;
        self.discovered
            .iter()
            .find(|(name, _)| name == sel)
            .map(|(_, p)| p.as_path())
    }

    #[must_use]
    pub fn selected_name(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    pub fn select(&mut self, name: &str) {
        if self.discovered.iter().any(|(n, _)| n == name) {
            self.selected = Some(name.to_string());
        }
    }

    #[must_use]
    pub fn evidence_enabled(&self) -> bool {
        self.evidence_enabled
    }

    pub fn set_evidence_enabled(&mut self, enabled: bool) {
        self.evidence_enabled = enabled;
    }

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
        ui.checkbox(
            &mut self.evidence_enabled,
            "evidence 採取 (.omc/logs/ へ証跡保存)",
        );
        changed
    }

    #[must_use]
    pub fn summary(&self) -> String {
        match self.selected_name() {
            Some(name) => format!("routine={name}"),
            None => "ルーチン未選択".to_string(),
        }
    }
}

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

#[must_use]
pub fn evidence_run_id(timestamp: &str) -> String {
    format!("{EVIDENCE_RUN_ID_PREFIX}-{timestamp}")
}

#[must_use]
pub fn new_evidence_run_id() -> String {
    let unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    evidence_run_id(&compact_utc_timestamp(unix))
}

fn compact_utc_timestamp(unix_secs: u64) -> String {
    let days = (unix_secs / 86_400) as i64;
    let secs_of_day = unix_secs % 86_400;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}{m:02}{d:02}-{:02}{:02}{:02}",
        secs_of_day / 3600,
        (secs_of_day % 3600) / 60,
        secs_of_day % 60
    )
}

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

    #[test]
    fn build_routine_args_evidence_on_off_contract() {
        let daily = real_root().join(ROUTINES_DIR).join("daily.toml");

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

        let args_off = build_routine_args(&daily, None);
        assert!(!args_off.iter().any(|a| a == "--evidence-run-id"));
        assert_eq!(args_off.len(), 2, "{args_off:?}");
    }

    #[test]
    fn panel_evidence_check_defaults_on_and_toggles() {
        let mut panel = RoutinePanel::default();
        assert!(panel.evidence_enabled(), "default must be ON");
        panel.set_evidence_enabled(false);
        assert!(!panel.evidence_enabled());
        panel.set_evidence_enabled(true);
        assert!(panel.evidence_enabled());
    }

    #[test]
    fn compact_utc_timestamp_known_values() {
        assert_eq!(compact_utc_timestamp(0), "19700101-000000");
        assert_eq!(compact_utc_timestamp(946_684_800), "20000101-000000");
        assert_eq!(compact_utc_timestamp(1_000_000_000), "20010909-014640");
        assert_eq!(compact_utc_timestamp(1_709_210_096), "20240229-123456");
    }

    #[test]
    fn new_evidence_run_id_shape() {
        let id = new_evidence_run_id();
        let rest = id
            .strip_prefix("routine-")
            .unwrap_or_else(|| panic!("{id}"));
        assert_eq!(rest.len(), 15, "{id}");
        assert_eq!(rest.as_bytes()[8], b'-', "{id}");
        assert!(
            rest.bytes().all(|b| b.is_ascii_digit() || b == b'-'),
            "{id}"
        );
    }
}
