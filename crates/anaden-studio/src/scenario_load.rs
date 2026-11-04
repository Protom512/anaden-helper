use std::path::{Path, PathBuf};

use anaden_vision::TaskDef;

use crate::scenario_ui::ScenarioEditorState;

#[derive(Debug, thiserror::Error)]
pub enum ScenarioLoadError {
    #[error("pipeline load failed: {0}")]
    Vision(#[from] anaden_vision::TaskDefError),
    #[error("pipeline directory `{}` has no loadable TaskDef", dir.display())]
    NoTaskDefs { dir: PathBuf },
    #[error("pipeline directory `{}` has no name component", dir.display())]
    NoDirName { dir: PathBuf },
    #[error("cannot resolve start task for manifest-less pipeline `{}`", dir.display())]
    StartUnresolvable { dir: PathBuf },
}

pub fn load_scenario_from_dir(dir: &Path) -> Result<ScenarioEditorState, ScenarioLoadError> {
    let mut defs = anaden_vision::load_pipeline(dir)?;
    if defs.is_empty() {
        return Err(ScenarioLoadError::NoTaskDefs {
            dir: dir.to_path_buf(),
        });
    }
    let name = dir
        .file_name()
        .and_then(|n| n.to_str())
        .map(String::from)
        .ok_or_else(|| ScenarioLoadError::NoDirName {
            dir: dir.to_path_buf(),
        })?;

    let stems = task_file_stems(dir);
    let manifest_exists = dir.join(anaden_vision::PIPELINE_MANIFEST_FILENAME).exists();
    let (start_task, goals) = if manifest_exists {
        let m = anaden_vision::load_pipeline_manifest(dir)?;
        (m.start_task, m.goals)
    } else {
        (derive_start_task_name(dir, &defs, &stems)?, Vec::new())
    };

    defs.sort_by(|a, b| a.name.cmp(&b.name));
    let loaded_task_names = defs.iter().map(|d| d.name.clone()).collect();
    Ok(ScenarioEditorState {
        name,
        start_task,
        goals,
        tasks: defs,
        loaded_task_names,
        loaded_task_files: stems,
        loaded_from: Some(dir.to_path_buf()),
    })
}

fn derive_start_task_name(
    dir: &Path,
    defs: &[TaskDef],
    stems: &[(String, String)],
) -> Result<String, ScenarioLoadError> {
    let Some(first) = crate::tasks::resolve_start_task(dir) else {
        return Err(ScenarioLoadError::NoTaskDefs {
            dir: dir.to_path_buf(),
        });
    };
    let Some(name) = stems
        .iter()
        .find(|(_, stem)| *stem == first)
        .map(|(name, _)| name.clone())
    else {
        return Err(ScenarioLoadError::StartUnresolvable {
            dir: dir.to_path_buf(),
        });
    };
    if !defs.iter().any(|d| d.name == name) {
        return Err(ScenarioLoadError::StartUnresolvable {
            dir: dir.to_path_buf(),
        });
    }
    Ok(name)
}

fn task_file_stems(dir: &Path) -> Vec<(String, String)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()).map(String::from) else {
            continue;
        };
        let is_toml = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("toml"));
        let is_manifest = path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n == anaden_vision::PIPELINE_MANIFEST_FILENAME);
        if !is_toml || is_manifest {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(value) = toml::from_str::<toml::Value>(&content) else {
            continue;
        };
        if let Some(name) = value.get("name").and_then(|v| v.as_str()) {
            out.push((name.to_string(), stem));
        }
    }
    out
}

#[must_use]
pub fn list_pipeline_dirs(pipelines_root: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(pipelines_root) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .filter_map(|p| p.file_name()?.to_str().map(String::from))
        .filter(|n| !n.starts_with('.'))
        .collect();
    names.sort();
    names
}

#[must_use]
pub fn task_name_issue(name: &str) -> Option<&'static str> {
    let name = name.trim();
    if name.is_empty() {
        return Some("task name must not be empty");
    }
    if name.contains('_') {
        return Some(
            "snake_case (`_`) is not allowed; use PascalCase `<Domain><ActionOrObject><Role>`",
        );
    }
    let Some(first) = name.chars().next() else {
        return Some("task name must not be empty");
    };
    if !first.is_ascii_uppercase() {
        return Some("name must start with an uppercase ASCII letter (PascalCase)");
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Some("name must consist of ASCII alphanumerics only");
    }
    let stem = name.trim_end_matches(|c: char| c.is_ascii_digit());
    let single_word = stem.chars().filter(|c| c.is_ascii_uppercase()).count() <= 1;
    if stem.len() != name.len() && single_word {
        return Some(
            "sequential numbering names (Node1) are not allowed; encode the role instead \
             (e.g. FieldLoopSecondPass)",
        );
    }
    if matches!(name, "Confirm" | "Check" | "Click") {
        return Some("overly generic name is not allowed; prefix the domain (e.g. FieldConfirm)");
    }
    None
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use anaden_core::{Goal, StopCondition};
    use std::fs;

    /// テスト用 TaskDef TOML 本体。
    fn task_toml(name: &str, template: &str) -> String {
        format!(
            "name = \"{name}\"\nstate = \"Field\"\nalgorithm = \"ccoeff\"\n\
             template = \"{template}\"\nroi = [10, 20, 100, 50]\nthreshold = 0.8\n"
        )
    }

    /// 命名バリデータ: 受容・拒否テーブル (設計ノート C)。
    #[test]
    fn task_name_issue_accepts_pascalcase_and_rejects_conventions() {
        // PascalCase (既存 pipeline 実名含む) は受容。
        for ok in [
            "Start",
            "Loop",
            "A",
            "TapA",
            "TapBottomStable",
            "TapBottomStablePc",
            "FieldTapHudTrPc",
            "FieldHudEnteredPc",
            "FieldLoopSecondPass",
            "FieldLoop2",
        ] {
            assert_eq!(task_name_issue(ok), None, "{ok} must be accepted");
        }
        // snake_case / camelCase / 数字開始 / 空・空白 / 非英数。
        for bad in [
            "tap_hud",
            "template_01",
            "tapHud",
            "1Tap",
            "",
            "   ",
            "Tap-Hud",
            "タップ",
        ] {
            assert!(task_name_issue(bad).is_some(), "{bad:?} must be rejected");
        }
        // 連番 (英単語 1 語 + 末尾数字)。
        for seq in ["Node1", "Task2", "A1", "Field2"] {
            assert!(task_name_issue(seq).is_some(), "{seq} must be rejected");
        }
        // 過汎用名 (単体)。
        for generic in ["Confirm", "Check", "Click"] {
            assert!(
                task_name_issue(generic).is_some(),
                "{generic} must be rejected"
            );
        }
    }

    /// list_pipeline_dirs: サブディレクトリのみ辞書順・隠し/ファイル除外。
    #[test]
    fn list_pipeline_dirs_returns_sorted_visible_dirs_only() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        fs::create_dir_all(root.join("fishing")).expect("mkdir");
        fs::create_dir_all(root.join("_title_load")).expect("mkdir");
        fs::create_dir_all(root.join(".hidden")).expect("mkdir");
        fs::write(root.join("not_a_dir.toml"), "x = 1").expect("write");
        assert_eq!(
            list_pipeline_dirs(root),
            vec!["_title_load".to_string(), "fishing".to_string()]
        );
        assert!(list_pipeline_dirs(&root.join("ghost")).is_empty());
    }

    /// manifest 無し pipeline: start_task は resolve_start_task と同一規則
    /// (辞書順先頭 stem) のファイルの TaskDef name。stem ≠ name (tap_bottom →
    /// TapBottomStable) でも name が入る (CLI 契約 t.name == start_task)。
    #[test]
    fn manifestless_load_derives_start_task_from_first_stem_taskdef_name() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().join("field_loop");
        fs::create_dir_all(&dir).expect("mkdir");
        fs::write(
            dir.join("tap_hud_tr.toml"),
            task_toml("TapHudTr", "hud.png"),
        )
        .expect("write");
        fs::write(
            dir.join("tap_bottom.toml"),
            task_toml("TapBottomStable", "bottom.png"),
        )
        .expect("write");

        let st = load_scenario_from_dir(&dir).expect("load");
        // 辞書順先頭 stem = tap_bottom → その TaskDef name。
        assert_eq!(
            crate::tasks::resolve_start_task(&dir).as_deref(),
            Some("tap_bottom")
        );
        assert_eq!(st.start_task, "TapBottomStable");
        assert!(st.goals.is_empty(), "manifest 無しは goals 空");
        assert_eq!(st.name, "field_loop");
        // 決定論的ソート + baseline 記録。
        assert_eq!(st.task_names(), vec!["TapBottomStable", "TapHudTr"]);
        assert_eq!(st.loaded_task_names, vec!["TapBottomStable", "TapHudTr"]);
        assert!(
            st.loaded_task_files
                .contains(&("TapBottomStable".to_string(), "tap_bottom".to_string()))
        );
        // template は load_pipeline が絶対化済み。
        assert!(st.task("TapHudTr").unwrap().template.is_absolute());
        // manifest 無し (goals 空) でも検証を通る (無限ループ互換)。
        assert!(
            st.validate().is_ok(),
            "issues: {:?}",
            st.validation_issues()
        );
    }

    /// manifest 付き pipeline: start_task + goals をそのまま運ぶ。
    #[test]
    fn manifest_load_carries_start_task_and_goals() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().join("fishing");
        fs::create_dir_all(&dir).expect("mkdir");
        fs::write(
            dir.join("fishing_start.toml"),
            task_toml("FishingStartPc", "../field_loop_pc/hud_tr.png"),
        )
        .expect("write");
        fs::write(
            dir.join("pipeline.toml"),
            "start_task = \"FishingStartPc\"\n\n[[goal]]\nname = \"g\"\n[goal.stop]\nLoopCount = { target = 3 }\n",
        )
        .expect("write");

        let st = load_scenario_from_dir(&dir).expect("load");
        assert_eq!(st.start_task, "FishingStartPc");
        assert_eq!(
            st.goals,
            vec![Goal {
                name: "g".to_string(),
                stop: StopCondition::LoopCount { target: 3 }
            }]
        );
    }

    /// エッジ: 不在 dir / TaskDef ゼロ (manifest のみ) / 不正 TOML / 壊れ manifest。
    #[test]
    fn load_fails_closed_on_missing_empty_and_broken_inputs() {
        let tmp = tempfile::tempdir().expect("tempdir");
        // 不在ディレクトリ → load_pipeline は空 Vec → NoTaskDefs。
        let e = load_scenario_from_dir(&tmp.path().join("ghost")).unwrap_err();
        assert!(matches!(e, ScenarioLoadError::NoTaskDefs { .. }), "{e:?}");
        // manifest のみ (TaskDef ゼロ) → NoTaskDefs。
        let only = tmp.path().join("only_manifest");
        fs::create_dir_all(&only).expect("mkdir");
        fs::write(only.join("pipeline.toml"), "start_task = \"X\"\n").expect("write");
        let e = load_scenario_from_dir(&only).unwrap_err();
        assert!(matches!(e, ScenarioLoadError::NoTaskDefs { .. }), "{e:?}");
        // 不正 TaskDef TOML (未知フィールド) → Vision(ParseFailed)。
        let broken = tmp.path().join("broken");
        fs::create_dir_all(&broken).expect("mkdir");
        fs::write(
            broken.join("bad.toml"),
            "name = \"X\"\nstate = \"X\"\nalgorithm = \"ccoeff\"\ntemplate = \"t.png\"\nbogus = 1\n",
        )
        .expect("write");
        let e = load_scenario_from_dir(&broken).unwrap_err();
        assert!(
            matches!(
                e,
                ScenarioLoadError::Vision(anaden_vision::TaskDefError::ParseFailed { .. })
            ),
            "{e:?}"
        );
        // manifest 在るのに parse 不能 → fail-closed (manifest-less 扱いにしない)。
        let badm = tmp.path().join("bad_manifest");
        fs::create_dir_all(&badm).expect("mkdir");
        fs::write(badm.join("t.toml"), task_toml("T", "t.png")).expect("write");
        fs::write(badm.join("pipeline.toml"), "bogus = 1\n").expect("write");
        let e = load_scenario_from_dir(&badm).unwrap_err();
        assert!(
            matches!(
                e,
                ScenarioLoadError::Vision(anaden_vision::TaskDefError::ParseFailed { .. })
            ),
            "{e:?}"
        );
    }
}
