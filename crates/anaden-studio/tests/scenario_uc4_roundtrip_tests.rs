//! Issue #160 UC-4 (Shard 5) 統合テスト — 既存 pipeline のエディタロード・
//! 編集・保存のロスネス往復機械保証。
//!
//! リポジトリ実 8 pipeline (`templates/pipelines/*`) 全てについて:
//! load → `ScenarioEditorState` → (編集なし) save → 再 load の TaskDef ベクタ
//! 意味比較 + manifest 比較 + 元ファイル名 (stem) 保全を検証する。
//! リポジトリ実ファイルを書き換えないよう、TOML のみをテンポラリ dir へ
//! コピーした同一階層構造上で検証する (`load_pipeline` / `save_task_def` は
//! PNG 実体を読み書きしないため PNG コピーは不要)。

#![allow(clippy::unwrap_used)]
#![allow(clippy::panic)]
#![allow(clippy::expect_used)]

use std::path::{Path, PathBuf};

use anaden_studio::scenario_load::{ScenarioLoadError, load_scenario_from_dir};
use anaden_studio::scenario_ui::{ScenarioPanel, save_scenario};
use anaden_vision::{TaskDef, load_pipeline, load_pipeline_manifest};

/// リポジトリ実 `templates/` ルート。
fn repo_templates() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("templates")
}

/// `templates/pipelines/` 配下の `*.toml` のみを `dst/templates/pipelines/` へ
/// 同一階層構造でコピーし、コピー先の pipelines ルートを返す
/// (vision 側 `copy_toml_files_only` と同じ手法)。
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

/// pipelines ルート直下の pipeline ディレクトリ一覧 (辞書順)。
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

/// ディレクトリ内の非 manifest `*.toml` ファイル名集合 (sorted・比較用)。
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

/// TaskDef ベクタを name 辞書順へソート (load_pipeline の read_dir 順は OS 依存
/// のため、意味比較を決定論化する)。
fn sorted_by_name(mut defs: Vec<TaskDef>) -> Vec<TaskDef> {
    defs.sort_by(|a, b| a.name.cmp(&b.name));
    defs
}

/// ヘッドレス egui コンテキスト内に子 Ui を作る (scenario_editor_tests と同一方式)。
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

/// 正常系 1: リポジトリ実 8 pipeline 全てについて load → (編集なし) save →
/// 再 load が意味等価。TaskDef ベクタ・manifest・元ファイル名 (stem) が保全され、
/// template 相対参照 (`../`・`../../`・サブディレクトリ) が元の形で書き戻る。
#[test]
fn all_eight_repo_pipelines_roundtrip_losslessly() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = copy_pipelines_tomls(tmp.path());
    let dirs = pipeline_dirs(&root);
    assert_eq!(
        dirs.len(),
        8,
        "既存 8 pipeline が前提: {:?}",
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

        // 編集なし保存 (テンポラリコピーの同一 dir へ上書き)。
        let saved = save_scenario(&state, &[], &root).expect("edit-free save");
        assert_eq!(saved, *dir, "既定の保存先 = ロード元 dir");

        // TaskDef ベクタの意味比較 (PartialEq・template 絶対パス含む)。
        let after = sorted_by_name(load_pipeline(dir).expect("load after"));
        assert_eq!(
            after,
            before,
            "{}: 編集なし保存で TaskDef が意味等価であること",
            dir.display()
        );

        // 元ファイル名 (stem ≠ name) が保全され、name ファイルが二重に作られない。
        assert_eq!(
            task_toml_names(dir),
            files_before,
            "{}: TaskDef ファイル集合が不変 (stem へ書き戻し)",
            dir.display()
        );

        // manifest 比較。
        let manifest_after = load_pipeline_manifest(dir).expect("manifest after save");
        match manifest_before {
            Some(before) => assert_eq!(
                manifest_after,
                before,
                "{}: manifest が意味等価",
                dir.display()
            ),
            None => {
                // manifest 無し pipeline: 保存で新規生成。空 goals は goal 行自体が
                // 出ない (= 無限ループの手書き後方互換)。
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

    // template 相対参照の保存形式保全 (サブディレクトリ・../ 参照含む)。
    let fishing = std::fs::read_to_string(root.join("fishing").join("fishing_start.toml"))
        .expect("fishing_start.toml");
    assert!(
        fishing.contains("template = \"../field_loop_pc/hud_tr.png\""),
        "../ 参照が元の形で書き戻る:\n{fishing}"
    );
    let login =
        std::fs::read_to_string(root.join("login").join("tap_title.toml")).expect("tap_title.toml");
    assert!(
        login.contains("template = \"../../scenes/title_pc/version_label.png\""),
        "../../ 参照が元の形で書き戻る:\n{login}"
    );
    let worldmap = std::fs::read_to_string(root.join("worldmap_loop").join("tap_ancient_tab.toml"))
        .expect("tap_ancient_tab.toml");
    assert!(
        worldmap.contains("template = \"ancient_tab.png\""),
        "pipeline dir 内の裸相対が元の形で書き戻る:\n{worldmap}"
    );

    // manifest 付き pipeline の保存 TOML は手書き pipeline.toml と toml::Value 等価
    // (= 「保存済み TOML は人間可読・既存形式と互換」の構造的証明)。
    for name in ["field_loop_pc", "fishing", "login"] {
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

/// 正常系 2 (焦点): manifest 無し pipeline (_title_load) の load → save で
/// pipeline.toml が新規生成され、start_task は resolve_start_task と同一規則
/// (辞書順先頭 TOML stem) のファイルの **TaskDef name** になる (anaden CLI の
/// `t.name == start_task` 実行契約と整合)。
#[test]
fn manifestless_save_creates_behavior_compatible_manifest() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = copy_pipelines_tomls(tmp.path());
    let dir = root.join("_title_load");
    assert!(!dir.join("pipeline.toml").exists(), "前提: manifest 無し");

    let state = load_scenario_from_dir(&dir).expect("load");
    // _title_load の TaskDef TOML は load_game.toml (stem 辞書順先頭) のみで、
    // その name は LoadGame。
    assert_eq!(
        anaden_studio::tasks::resolve_start_task(&dir).as_deref(),
        Some("load_game"),
        "前提: 辞書順先頭 stem"
    );
    assert_eq!(state.start_task, "LoadGame", "stem でなく TaskDef name");
    assert!(state.goals.is_empty());

    save_scenario(&state, &[], &root).expect("save");
    let manifest = load_pipeline_manifest(&dir).expect("新規生成 manifest");
    assert_eq!(manifest.start_task, "LoadGame");
    assert!(
        manifest.goals.is_empty(),
        "空 goals = goal 行自体に出ない (無限ループ後方互換)"
    );
}

/// 正常系 3: ロード → threshold / next 編集 → save で当該フィールドのみ変化し、
/// 他 (state/algorithm/template/roi/base/action) は意味保存される。
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
    // 期待値 = before に同じ編集を適用したもの (他フィールドは全て before 由来)。
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
    // 編集が実際に反映されていること (vacuous pass 防止)。
    let hud_after = after
        .iter()
        .find(|d| d.name == "TapHudTrPc")
        .expect("TapHudTrPc");
    assert!((hud_after.threshold - 0.77).abs() < 1e-6);
}

/// 正常系 4: ヘッドレス egui で「開く → 編集 → 保存」フロー (UC-4 GUI)。
/// 「既存 pipeline を開く」UI を含むパネル描画がパニック無しで完了し、
/// 保存実体がロード元 dir に記録される。
#[test]
fn headless_open_edit_save_flow_via_panel() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = copy_pipelines_tomls(tmp.path());
    let dir = root.join("login");

    let mut panel = ScenarioPanel::new(root.clone());
    let mut status = String::new();
    assert!(panel.open_existing(&dir, &mut status), "status: {status}");

    // ヘッドレス描画 2 パス (オープン UI・コンボ列挙を含む)。
    let ctx = egui::Context::default();
    ctx.begin_pass(egui::RawInput::default());
    panel.ui(&mut child_ui(&ctx), None, &mut status);
    panel.ui(&mut child_ui(&ctx), None, &mut status);
    let _ = ctx.end_pass();

    // 編集 (threshold) → 保存 → 保存実体 = ロード元 dir。
    panel.state.task_mut("LoginTapTitlePc").unwrap().threshold = 0.66;
    panel.save(&mut status);
    assert_eq!(panel.saved_pipeline(), Some(dir.as_path()));

    let defs = load_pipeline(&dir).expect("reload");
    assert!(
        defs.iter()
            .any(|d| d.name == "LoginTapTitlePc" && (d.threshold - 0.66).abs() < 1e-6),
        "編集が反映されている: {defs:?}"
    );
    // manifest も保存され直される (goal 含む)。
    let manifest = load_pipeline_manifest(&dir).expect("manifest");
    assert_eq!(manifest.start_task, "LoginTapTitlePc");
    assert_eq!(
        manifest.goals.len(),
        1,
        "login の Any 合成 goal が保持される"
    );
}

/// エッジ 1: 不正 pipeline dir / TaskDef ゼロ / 不正 TOML → エラー伝播。
#[test]
fn load_fails_closed_on_invalid_inputs() {
    let tmp = tempfile::tempdir().expect("tempdir");
    // 不在 dir。
    let e = load_scenario_from_dir(&tmp.path().join("ghost")).unwrap_err();
    assert!(matches!(e, ScenarioLoadError::NoTaskDefs { .. }), "{e:?}");
    // TaskDef ゼロ (manifest のみ)。
    let only = tmp.path().join("only_manifest");
    std::fs::create_dir_all(&only).expect("mkdir");
    std::fs::write(only.join("pipeline.toml"), "start_task = \"X\"\n").expect("write");
    let e = load_scenario_from_dir(&only).unwrap_err();
    assert!(matches!(e, ScenarioLoadError::NoTaskDefs { .. }), "{e:?}");
    // 不正 TaskDef TOML。
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

/// エッジ 2: 命名バリデータ — 既存 pipeline 編集時は既存名を警告せず、
/// リネーム・新規追加名は MDA 規約 (PascalCase 強制・snake_case/連番/過汎用名拒否)
/// で検出する。GUI フロー (validation_issues) 経由で検証。
#[test]
fn loaded_pipeline_editing_warns_only_new_and_renamed_names() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = copy_pipelines_tomls(tmp.path());

    // 既存 pipeline をロードしたままでは既存名 (PascalCase 適合済み) は無警告。
    let mut state = load_scenario_from_dir(&root.join("nav_to_field_pc")).expect("load");
    assert!(
        !state.validation_issues().iter().any(|i| matches!(
            i,
            anaden_studio::scenario_ui::ScenarioValidationError::TaskNaming { .. }
        )),
        "既存名は変更しない限り警告しない"
    );

    // リネーム: snake_case 名へは警告。
    state.task_mut("FieldHudTopPc").unwrap().name = "field_hud_v2".to_string();
    assert!(
        state
            .validation_issues()
            .iter()
            .any(|i| matches!(i, anaden_studio::scenario_ui::ScenarioValidationError::TaskNaming { name, .. } if name == "field_hud_v2")),
        "リネーム後の snake_case 名は検出"
    );

    // 新規追加: 過汎用名 (Confirm 単体) は警告。
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
    // PascalCase 新規名は命名警告されない (snake_case リネーム分は残存)。
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

/// エッジ 3: 削除・リネームしたタスクの旧 TaskDef ファイルは保存時に掃除され、
/// 再 load で旧タスクが復活しない (残存タスクの元 stem ファイル保全との両立)。
#[test]
fn save_sweeps_removed_and_renamed_task_files() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = copy_pipelines_tomls(tmp.path());
    let dir = root.join("nav_to_field_pc");

    let mut state = load_scenario_from_dir(&dir).expect("load");
    assert_eq!(state.task_names().len(), 3);

    // 削除 (LoadGamePc) + リネーム (FieldHudTopPc → FieldArrivalSentinelPc)。
    // 削除で TapToStartPc.next が dangling に、リネームで start_task (manifest
    // 無し導出値 = FieldHudTopPc) が dangling になるため両方繋ぎ替える。
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
    // 残存タスクは元 stem ファイルへ、リネームタスクは新 name ファイルへ。
    assert!(dir.join("tap_to_start.toml").exists());
    assert!(dir.join("FieldArrivalSentinelPc.toml").exists());
}
