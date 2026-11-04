#![allow(clippy::unwrap_used)]
#![allow(clippy::panic)]
#![allow(clippy::expect_used)]

use std::path::{Path, PathBuf};

use anaden_studio::scenario_load::{ScenarioLoadError, load_scenario_from_dir};
use anaden_studio::scenario_ui::{ScenarioPanel, save_scenario};
use anaden_vision::{TaskDef, load_pipeline, load_pipeline_manifest};

fn repo_templates() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("templates")
}

fn copy_pipelines_tomls(dst: &Path) -> PathBuf {
    let src = repo_templates().join("pipelines");
    let out = dst.join("templates").join("pipelines");
    copy_toml_tree(&src, &out);
    out
}

fn copy_toml_tree(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).expect("mkdir dst");
    for entry in std::fs::read_dir(src).expect("read src") {
        let p = entry.expect("entry").path();
        if p.is_dir() {
            let name = p.file_name().expect("dir name");
            copy_toml_tree(&p, &dst.join(name));
        } else {
            let is_toml = p
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("toml"));
            if is_toml {
                let name = p.file_name().expect("file name");
                std::fs::copy(&p, dst.join(name)).expect("copy toml");
            }
        }
    }
}

fn pipeline_dirs(root: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(root)
        .expect("read pipelines")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    dirs
}

fn task_toml_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .expect("read dir")
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("toml"))
                && p.file_name().and_then(|n| n.to_str()) != Some("pipeline.toml")
        })
        .filter_map(|p| p.file_name()?.to_str().map(String::from))
        .collect();
    names.sort();
    names
}

fn sorted_by_name(mut defs: Vec<TaskDef>) -> Vec<TaskDef> {
    defs.sort_by(|a, b| a.name.cmp(&b.name));
    defs
}

fn child_ui(ctx: &egui::Context) -> egui::Ui {
    egui::Ui::new(
        ctx.clone(),
        egui::Id::new("scenario-uc4-area"),
        egui::UiBuilder::new().max_rect(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(800.0, 600.0),
        )),
    )
}

#[test]
fn all_repo_pipelines_roundtrip_losslessly() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = copy_pipelines_tomls(tmp.path());
    let dirs = pipeline_dirs(&root);
    assert!(
        !dirs.is_empty(),
        "リポジトリ実 pipeline が少なくとも 1 つコピーされている前提: {:?}",
        dirs.iter()
            .map(|d| d.display().to_string())
            .collect::<Vec<_>>()
    );

    for dir in &dirs {
        let files_before = task_toml_names(dir);
        let before = sorted_by_name(load_pipeline(dir).expect("load before"));
        let manifest_before = dir
            .join("pipeline.toml")
            .exists()
            .then(|| load_pipeline_manifest(dir).expect("manifest before"));

        let state =
            load_scenario_from_dir(dir).unwrap_or_else(|e| panic!("{}: load: {e}", dir.display()));
        assert!(
            state.validate().is_ok(),
            "{}: 既存 pipeline のロード結果は保存可能な状態であるべき: {:?}",
            dir.display(),
            state.validation_issues()
        );

        let saved = save_scenario(&state, &[], &root).expect("edit-free save");
        assert_eq!(saved, *dir, "既定の保存先 = ロード元 dir");

        let after = sorted_by_name(load_pipeline(dir).expect("load after"));
        assert_eq!(
            after,
            before,
            "{}: 編集なし保存で TaskDef が意味等価であること",
            dir.display()
        );

        assert_eq!(
            task_toml_names(dir),
            files_before,
            "{}: TaskDef ファイル集合が不変 (stem へ書き戻し)",
            dir.display()
        );

        let manifest_after = load_pipeline_manifest(dir).expect("manifest after save");
        match manifest_before {
            Some(before) => assert_eq!(
                manifest_after,
                before,
                "{}: manifest が意味等価",
                dir.display()
            ),
            None => {
                assert!(
                    manifest_after.goals.is_empty(),
                    "{}: 空 goals 互換",
                    dir.display()
                );
                let source =
                    std::fs::read_to_string(dir.join("pipeline.toml")).expect("read manifest");
                assert!(
                    !source.contains("goal"),
                    "{}: goal 行自体が出ないこと:\n{source}",
                    dir.display()
                );
            }
        }
    }

    let fishing = std::fs::read_to_string(root.join("fishing").join("fishing_start.toml"))
        .expect("fishing_start.toml");
    assert!(
        fishing.contains("template = \"../field_loop_pc/hud_tr.png\""),
        "../ 参照が元の形で書き戻る:\n{fishing}"
    );
    let login =
        std::fs::read_to_string(root.join("login").join("tap_title.toml")).expect("tap_title.toml");
    assert!(
        login.contains("template = \"../../scenes/title_pc/title_logo_corner.png\""),
        "../../ 参照が元の形で書き戻る:\n{login}"
    );

    for name in ["field_loop_pc", "nav_to_field_pc", "fishing", "login"] {
        let handwritten = std::fs::read_to_string(
            repo_templates()
                .join("pipelines")
                .join(name)
                .join("pipeline.toml"),
        )
        .expect("handwritten manifest");
        let saved =
            std::fs::read_to_string(root.join(name).join("pipeline.toml")).expect("saved manifest");
        let hw: toml::Value = toml::from_str(&handwritten).expect("handwritten value");
        let sv: toml::Value = toml::from_str(&saved).expect("saved value");
        assert_eq!(sv, hw, "{name}: manifest 保存形式が手書き形式と一致");
    }
}

#[test]
fn manifestless_save_creates_behavior_compatible_manifest() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = copy_pipelines_tomls(tmp.path());
    let dir = root.join("nav_to_field_pc");
    std::fs::remove_file(dir.join("pipeline.toml")).expect("strip manifest for fixture");
    assert!(!dir.join("pipeline.toml").exists(), "前提: manifest 無し");

    let state = load_scenario_from_dir(&dir).expect("load");
    assert_eq!(
        anaden_studio::tasks::resolve_start_task(&dir).as_deref(),
        Some("field_hud_top"),
        "前提: 辞書順先頭 stem"
    );
    assert_eq!(
        state.start_task, "FieldHudTopPc",
        "stem でなく TaskDef name"
    );
    assert!(state.goals.is_empty());

    save_scenario(&state, &[], &root).expect("save");
    let manifest = load_pipeline_manifest(&dir).expect("新規生成 manifest");
    assert_eq!(manifest.start_task, "FieldHudTopPc");
    assert!(
        manifest.goals.is_empty(),
        "空 goals = goal 行自体に出ない (無限ループ後方互換)"
    );
}

#[test]
fn edit_threshold_and_next_changes_only_edited_fields() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = copy_pipelines_tomls(tmp.path());
    let dir = root.join("field_loop_pc");

    let before = sorted_by_name(load_pipeline(&dir).expect("load before"));
    let hud_before = before
        .iter()
        .find(|d| d.name == "TapHudTrPc")
        .expect("TapHudTrPc");
    assert!(
        (hud_before.threshold - 0.70).abs() < 1e-6,
        "前提: 編集前 threshold=0.70"
    );

    let mut state = load_scenario_from_dir(&dir).expect("load");
    state.task_mut("TapHudTrPc").unwrap().threshold = 0.77;
    state.task_mut("TapBottomStablePc").unwrap().next = Some(vec!["TapHudTrPc".to_string()]);
    save_scenario(&state, &[], &root).expect("save after edit");

    let after = sorted_by_name(load_pipeline(&dir).expect("load after"));
    let mut expected = before.clone();
    expected
        .iter_mut()
        .find(|d| d.name == "TapHudTrPc")
        .unwrap()
        .threshold = 0.77;
    expected
        .iter_mut()
        .find(|d| d.name == "TapBottomStablePc")
        .unwrap()
        .next = Some(vec!["TapHudTrPc".to_string()]);
    assert_eq!(after, expected, "編集フィールドのみ変化・他は意味保存");
    let hud_after = after
        .iter()
        .find(|d| d.name == "TapHudTrPc")
        .expect("TapHudTrPc");
    assert!((hud_after.threshold - 0.77).abs() < 1e-6);
}

#[test]
fn headless_open_edit_save_flow_via_panel() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = copy_pipelines_tomls(tmp.path());
    let dir = root.join("login");

    let mut panel = ScenarioPanel::new(root.clone());
    let mut status = String::new();
    assert!(panel.open_existing(&dir, &mut status), "status: {status}");

    let ctx = egui::Context::default();
    ctx.begin_pass(egui::RawInput::default());
    panel.ui(&mut child_ui(&ctx), None, &mut status);
    panel.ui(&mut child_ui(&ctx), None, &mut status);
    let _ = ctx.end_pass();

    panel.state.task_mut("LoginTapTitlePc").unwrap().threshold = 0.66;
    panel.save(&mut status);
    assert_eq!(panel.saved_pipeline(), Some(dir.as_path()));

    let defs = load_pipeline(&dir).expect("reload");
    assert!(
        defs.iter()
            .any(|d| d.name == "LoginTapTitlePc" && (d.threshold - 0.66).abs() < 1e-6),
        "編集が反映されている: {defs:?}"
    );
    let manifest = load_pipeline_manifest(&dir).expect("manifest");
    assert_eq!(manifest.start_task, "LoginTapTitlePc");
    assert_eq!(
        manifest.goals.len(),
        1,
        "login の Any 合成 goal が保持される"
    );
}

#[test]
fn load_fails_closed_on_invalid_inputs() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let e = load_scenario_from_dir(&tmp.path().join("ghost")).unwrap_err();
    assert!(matches!(e, ScenarioLoadError::NoTaskDefs { .. }), "{e:?}");
    let only = tmp.path().join("only_manifest");
    std::fs::create_dir_all(&only).expect("mkdir");
    std::fs::write(only.join("pipeline.toml"), "start_task = \"X\"\n").expect("write");
    let e = load_scenario_from_dir(&only).unwrap_err();
    assert!(matches!(e, ScenarioLoadError::NoTaskDefs { .. }), "{e:?}");
    let broken = tmp.path().join("broken");
    std::fs::create_dir_all(&broken).expect("mkdir");
    std::fs::write(
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
}

#[test]
fn loaded_pipeline_editing_warns_only_new_and_renamed_names() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = copy_pipelines_tomls(tmp.path());

    let mut state = load_scenario_from_dir(&root.join("nav_to_field_pc")).expect("load");
    assert!(
        !state.validation_issues().iter().any(|i| matches!(
            i,
            anaden_studio::scenario_ui::ScenarioValidationError::TaskNaming { .. }
        )),
        "既存名は変更しない限り警告しない"
    );

    state.task_mut("FieldHudTopPc").unwrap().name = "field_hud_v2".to_string();
    assert!(
        state
            .validation_issues()
            .iter()
            .any(|i| matches!(i, anaden_studio::scenario_ui::ScenarioValidationError::TaskNaming { name, .. } if name == "field_hud_v2")),
        "リネーム後の snake_case 名は検出"
    );

    let mut extra = state.task("TapToStartPc").unwrap().clone();
    extra.name = "Confirm".to_string();
    state.add_task(extra);
    assert!(
        state
            .validation_issues()
            .iter()
            .any(|i| matches!(i, anaden_studio::scenario_ui::ScenarioValidationError::TaskNaming { name, .. } if name == "Confirm")),
        "新規追加の過汎用名は検出"
    );
    state.task_mut("Confirm").unwrap().name = "FieldConfirmOpenPc".to_string();
    assert!(
        !state.validation_issues().iter().any(|i| matches!(
            i,
            anaden_studio::scenario_ui::ScenarioValidationError::TaskNaming { name, .. }
                if name == "FieldConfirmOpenPc"
        )),
        "PascalCase 新規名は許容"
    );
}

#[test]
fn save_sweeps_removed_and_renamed_task_files() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = copy_pipelines_tomls(tmp.path());
    let dir = root.join("nav_to_field_pc");

    let mut state = load_scenario_from_dir(&dir).expect("load");
    assert_eq!(state.task_names().len(), 3);

    assert!(state.remove_task("LoadGamePc"));
    state.task_mut("FieldHudTopPc").unwrap().name = "FieldArrivalSentinelPc".to_string();
    state.task_mut("TapToStartPc").unwrap().next = Some(vec!["FieldArrivalSentinelPc".to_string()]);
    assert!(state.set_start_task("FieldArrivalSentinelPc"));
    save_scenario(&state, &[], &root).expect("save");

    let after = sorted_by_name(load_pipeline(&dir).expect("reload"));
    let names: Vec<&str> = after.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["FieldArrivalSentinelPc", "TapToStartPc"],
        "削除済み LoadGamePc が復活せずリネームが反映: {names:?}"
    );
    assert!(
        !dir.join("load_game.toml").exists(),
        "削除タスクの旧 stem ファイルが掃除される"
    );
    assert!(
        !dir.join("field_hud_top.toml").exists(),
        "リネームタスクの旧 stem ファイルが掃除される"
    );
    assert!(dir.join("tap_to_start.toml").exists());
    assert!(dir.join("FieldArrivalSentinelPc.toml").exists());
}
