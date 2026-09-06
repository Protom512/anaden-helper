//! Issue #160 T3 (UC-1/UC-2) シナリオエディタ統合テスト — AC-1 機械保証。
//!
//! GUI のシナリオエディタ状態 (`ScenarioEditorState` + `ScenarioPanel`) から
//! 保存した pipeline が、既存 anaden-vision の loader
//! (`load_pipeline` / `load_pipeline_manifest`) で parse 可能であることを
//! 検証する (保存 -> load 往復・start_task 解決・テンプレート PNG 実在)。
//!
//! - UC-1 (シナリオ作成): 多 TaskDef + manifest 保存が `templates/pipelines/<name>/`
//!   互換形式で書き出される
//! - UC-2 (テンプレート連携): 実リポジトリ `templates/` の PNG を相対参照した
//!   タスクが load 後に実在パスへ解決される
//!
//! ヘッドレス (ウィンドウ生成なし)。egui パネル描画は `app.rs` 単体テストと
//! 同じ `Context::default()` + `Ui::new` 方式でパニック無しを検証する。

#![allow(clippy::unwrap_used)]
#![allow(clippy::panic)]
#![allow(clippy::expect_used)]

use std::path::{Path, PathBuf};

use anaden_core::{Goal, ScreenRegion, StopCondition};
use anaden_studio::app::{PipelineActionKind, pipeline_task_spec};
use anaden_studio::scenario_ui::{
    ScenarioEditorState, ScenarioPanel, ScenarioSaveError, ScenarioValidationError, save_scenario,
};
use anaden_vision::{Action, Algorithm, TaskDef};
use image::{DynamicImage, GrayImage, Luma};

/// テスト用 TaskDef (roi=[10,20,100,50]・threshold=0.8・click_self・next 空)。
/// `template` は ROI 追加フロー (`pipeline_task_spec`) 相当の裸相対 `<name>.png`
/// (保存時の PNG 名 `<name>.png` と一致させる。大文字小文字保持)。
fn task_def(name: &str) -> TaskDef {
    TaskDef {
        name: name.to_string(),
        state: "Field".to_string(),
        algorithm: Algorithm::Ccoeff,
        template: PathBuf::from(format!("{name}.png")),
        roi: Some([10, 20, 100, 50]),
        threshold: 0.8,
        base: None,
        action: Some(Action::ClickSelf),
        next: Some(vec![]),
    }
}

/// 単色 GrayImage (サイズは可変・塗り値でタスク間を識別)。
fn solid_image(w: u32, h: u32, v: u8) -> DynamicImage {
    DynamicImage::ImageLuma8(GrayImage::from_pixel(w, h, Luma([v])))
}

/// AC-1: エディタ状態 → 保存 → load_pipeline / load_pipeline_manifest 往復。
/// ROI 追加タスク (pending PNG) 込みで manifest・TaskDef・PNG が揃い、
/// start_task が解決し、各タスクのテンプレート PNG が実在する。
#[test]
fn save_scenario_roundtrips_through_loaders_with_pngs() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("pipelines");

    let mut st = ScenarioEditorState::new("fishing2");
    st.add_task(task_def("Start"));
    st.add_task(task_def("Loop"));
    st.task_mut("Start").unwrap().next = Some(vec!["Loop".to_string()]);
    st.add_goal(Goal {
        name: "loop3".to_string(),
        stop: StopCondition::LoopCount { target: 3 },
    });
    st.add_goal(Goal {
        name: "timeout".to_string(),
        stop: StopCondition::Timeout { secs: 600 },
    });
    st.validate().expect("scenario must be valid");

    let pngs = vec![
        ("Start".to_string(), solid_image(100, 50, 120)),
        ("Loop".to_string(), solid_image(100, 50, 200)),
    ];
    let dir = save_scenario(&st, &pngs, &root).expect("save");
    assert_eq!(dir, root.join("fishing2"));

    // ディスク上の構成: manifest + TaskDef TOML + ROI crop PNG。
    assert!(dir.join("pipeline.toml").exists());
    assert!(dir.join("Start.toml").exists());
    assert!(dir.join("Loop.toml").exists());
    assert!(dir.join("Start.png").exists());
    assert!(dir.join("Loop.png").exists());

    // manifest は load_pipeline_manifest で parse 可能・start_task 解決。
    let manifest = anaden_vision::load_pipeline_manifest(&dir).expect("load manifest");
    assert_eq!(manifest.start_task, "Start");
    assert_eq!(manifest.goals.len(), 2);
    assert_eq!(
        manifest.goals[0].stop,
        StopCondition::LoopCount { target: 3 }
    );
    assert_eq!(manifest.goals[1].stop, StopCondition::Timeout { secs: 600 });

    // TaskDef 群は load_pipeline で parse 可能・next 鎖と PNG 実在を検証。
    let defs = anaden_vision::load_pipeline(&dir).expect("load pipeline");
    assert_eq!(
        defs.len(),
        2,
        "pipeline.toml (manifest) は TaskDef として読まない"
    );
    for def in &defs {
        assert!(
            def.template.is_absolute(),
            "load_pipeline は相対 template を絶対化する: {}",
            def.template.display()
        );
        assert!(
            def.template.exists(),
            "テンプレート PNG が実在する必要がある: {}",
            def.template.display()
        );
    }
    let start = defs.iter().find(|d| d.name == "Start").expect("Start");
    assert_eq!(
        start.next.as_deref(),
        Some(&["Loop".to_string()][..]),
        "next 鎖が往復後も保持される"
    );
}

/// fail-closed: 不正状態 (TaskDef 0 件) は Err + 何も書かない。
#[test]
fn save_scenario_fails_closed_on_invalid_state() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("pipelines");
    let st = ScenarioEditorState::new("empty_scenario");
    let err = save_scenario(&st, &[], &root).expect_err("must fail without tasks");
    assert!(
        matches!(
            err,
            ScenarioSaveError::Invalid(ScenarioValidationError::NoTasks)
        ),
        "err: {err:?}"
    );
    assert!(!root.join("empty_scenario").exists(), "1 バイトも書かない");
}

/// fail-closed: パス区切りを含むシナリオ名は UnsafeName で拒否。
#[test]
fn save_scenario_rejects_unsafe_name() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("pipelines");
    let mut st = ScenarioEditorState::new("a/b");
    st.add_task(task_def("Start"));
    let err = save_scenario(&st, &[], &root).expect_err("must fail on unsafe name");
    assert!(
        matches!(
            err,
            ScenarioSaveError::Invalid(ScenarioValidationError::UnsafeName { .. })
        ),
        "err: {err:?}"
    );
    assert!(!root.join("a").exists(), "親ディレクトリ脱出を書かない");
}

/// UC-2: 実リポジトリ templates/ の PNG を相対参照したタスクが、
/// 保存 -> load 後に実在絶対パスへ解決される (テンプレート作成ペイン成果物連携)。
#[test]
fn uc2_reference_to_real_templates_png_resolves_to_existing_file() {
    let repo_templates = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("templates");
    let png = repo_templates
        .join("pipelines")
        .join("field_loop_pc")
        .join("hud_tr.png");
    assert!(
        png.exists(),
        "実テンプレート PNG が前提として存在: {}",
        png.display()
    );

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("pipelines");
    let pipeline_dir = root.join("uc2_check");

    let mut st = ScenarioEditorState::new("uc2_check");
    st.add_task(task_def("Start"));
    assert!(st.assign_template("Start", &png, &pipeline_dir));
    st.add_goal(Goal {
        name: "match".to_string(),
        stop: StopCondition::TemplateMatch {
            task: "Start".to_string(),
            confidence: 0.85,
        },
    });
    let dir = save_scenario(&st, &[], &root).expect("save");

    let manifest = anaden_vision::load_pipeline_manifest(&dir).expect("load manifest");
    assert_eq!(manifest.start_task, "Start");
    let defs = anaden_vision::load_pipeline(&dir).expect("load pipeline");
    let start = defs.iter().find(|d| d.name == "Start").expect("Start");
    assert!(
        start.template.is_absolute() && start.template.exists(),
        "相対参照が load 後に実在パスへ解決される: {}",
        start.template.display()
    );
    assert!(
        start.template.ends_with("hud_tr.png"),
        "参照先が実テンプレート PNG: {}",
        start.template.display()
    );
}

/// ScenarioPanel の GUI フロー (候補追加・重複拒否・next 鎖・ゴール・
/// ヘッドレス描画・保存) を通した AC-1 検証。
#[test]
fn panel_flow_candidate_add_goal_save_roundtrip() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("pipelines");
    let mut panel = ScenarioPanel::new(root.clone());
    panel.state.name = "panel_flow".to_string();

    // 候補追加 (app::pipeline_task_spec 経由 = Authoring ペインと同一導出)。
    let (tap, tap_png) = gui_candidate("TapA", ScreenRegion::new(10, 20, 100, 50));
    panel.add_candidate(tap, tap_png).expect("add TapA");
    let (dup, dup_png) = gui_candidate("TapA", ScreenRegion::new(0, 0, 30, 30));
    assert!(
        panel.add_candidate(dup, dup_png).is_err(),
        "同名タスクの重複追加は拒否"
    );
    let (wait, wait_png) = gui_candidate("WaitB", ScreenRegion::new(5, 5, 40, 40));
    panel.add_candidate(wait, wait_png).expect("add WaitB");

    // next 鎖 (TapA -> WaitB) とゴール。
    panel.state.task_mut("TapA").unwrap().next = Some(vec!["WaitB".to_string()]);
    panel.state.add_goal(Goal {
        name: "loop5".to_string(),
        stop: StopCondition::LoopCount { target: 5 },
    });

    // ヘッドレス描画 (候補あり/なし両方) がパニック無しで完了する。
    let mut status = String::new();
    let ctx = egui::Context::default();
    ctx.begin_pass(egui::RawInput::default());
    let (extra, extra_png) = gui_candidate("Extra", ScreenRegion::new(1, 1, 20, 20));
    panel.ui(&mut child_ui(&ctx), Some((extra, extra_png)), &mut status);
    panel.ui(&mut child_ui(&ctx), None, &mut status);
    let _ = ctx.end_pass();

    // 保存 -> AC-1 往復検証。
    panel.save(&mut status);
    assert!(status.contains("シナリオ保存"), "status: {status}");
    let dir = root.join("panel_flow");
    let manifest = anaden_vision::load_pipeline_manifest(&dir).expect("load manifest");
    assert_eq!(manifest.start_task, "TapA", "start_task は最初の追加タスク");
    assert_eq!(manifest.goals.len(), 1);
    let defs = anaden_vision::load_pipeline(&dir).expect("load pipeline");
    assert_eq!(defs.len(), 2);
    for def in &defs {
        assert!(
            def.template.is_absolute() && def.template.exists(),
            "PNG 実在: {}",
            def.template.display()
        );
    }
    assert!(dir.join("TapA.png").exists());
    assert!(dir.join("WaitB.png").exists());
    let tap = defs.iter().find(|d| d.name == "TapA").expect("TapA");
    assert_eq!(tap.next.as_deref(), Some(&["WaitB".to_string()][..]));
}

/// ヘッドレス egui コンテキスト内に子 Ui を作る (app.rs 単体テストと同一方式)。
fn child_ui(ctx: &egui::Context) -> egui::Ui {
    egui::Ui::new(
        ctx.clone(),
        egui::Id::new("scenario-test-area"),
        egui::UiBuilder::new().max_rect(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(800.0, 600.0),
        )),
    )
}

/// `pipeline_task_spec` + crop PNG で Authoring ペインの追加候補を構築する。
fn gui_candidate(name: &str, roi: ScreenRegion) -> (TaskDef, DynamicImage) {
    let task = pipeline_task_spec(
        name,
        "field",
        "ccoeff",
        roi,
        0.85,
        PipelineActionKind::ClickSelf,
    )
    .expect("valid spec");
    let crop = solid_image(roi.width, roi.height, 160);
    (task, crop)
}
