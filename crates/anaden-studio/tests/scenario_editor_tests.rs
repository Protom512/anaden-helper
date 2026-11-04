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

fn solid_image(w: u32, h: u32, v: u8) -> DynamicImage {
    DynamicImage::ImageLuma8(GrayImage::from_pixel(w, h, Luma([v])))
}

fn minimal_state(name: &str) -> ScenarioEditorState {
    let mut st = ScenarioEditorState::new(name);
    st.add_task(task_def("Start"));
    st.add_goal(Goal {
        name: "loop3".to_string(),
        stop: StopCondition::LoopCount { target: 3 },
    });
    st
}

fn dir_snapshot(dir: &Path) -> Vec<(String, u64)> {
    let mut out: Vec<_> = std::fs::read_dir(dir)
        .expect("read dir")
        .map(|e| {
            let e = e.expect("entry");
            let len = std::fs::metadata(e.path()).expect("metadata").len();
            (e.file_name().to_string_lossy().into_owned(), len)
        })
        .collect();
    out.sort();
    out
}

#[test]
fn save_scenario_refuses_existing_pipeline_dir_of_other_scenario() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("pipelines");
    let victim = root.join("login");
    save_scenario(&minimal_state("login"), &[], &root).expect("create victim pipeline");
    let before = dir_snapshot(&victim);

    let mut other = minimal_state("login");
    other.task_mut("Start").unwrap().threshold = 0.99;
    let err = save_scenario(&other, &[], &root).expect_err("must refuse existing dir");
    assert!(
        matches!(err, ScenarioSaveError::PipelineDirAlreadyExists { ref dir } if dir == &victim),
        "unexpected error: {err:?}"
    );
    assert_eq!(dir_snapshot(&victim), before, "拒否時は 1 バイトも書かない");
}

#[test]
fn save_scenario_allows_writeback_to_loaded_source_dir() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("pipelines");
    save_scenario(&minimal_state("field"), &[], &root).expect("create pipeline");

    let mut st =
        anaden_studio::scenario_load::load_scenario_from_dir(&root.join("field")).expect("load");
    assert_eq!(
        st.loaded_from.as_deref(),
        Some(root.join("field").as_path())
    );
    let first_task = st.tasks[0].name.clone();
    st.task_mut(&first_task).unwrap().threshold = 0.9;
    save_scenario(&st, &[], &root).expect("writeback to loaded dir must be allowed");
}

#[test]
fn save_scenario_refuses_rename_onto_other_existing_pipeline() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("pipelines");
    save_scenario(&minimal_state("alpha"), &[], &root).expect("alpha");
    save_scenario(&minimal_state("beta"), &[], &root).expect("beta");
    let before = dir_snapshot(&root.join("beta"));

    let mut st =
        anaden_studio::scenario_load::load_scenario_from_dir(&root.join("alpha")).expect("load");
    st.name = "beta".to_string();
    let err = save_scenario(&st, &[], &root).expect_err("must refuse rename onto beta");
    assert!(
        matches!(err, ScenarioSaveError::PipelineDirAlreadyExists { .. }),
        "unexpected error: {err:?}"
    );
    assert_eq!(dir_snapshot(&root.join("beta")), before, "beta は不変");
}

#[test]
fn panel_consecutive_saves_of_new_scenario_are_allowed() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("pipelines");
    let mut panel = ScenarioPanel::new(root.clone());
    panel.state = minimal_state("neko_run");

    let mut status = String::new();
    panel.save(&mut status);
    assert!(status.contains("シナリオ保存"), "first save: {status}");
    let saved_dir = root.join("neko_run");
    assert_eq!(
        panel.state.loaded_from.as_deref(),
        Some(saved_dir.as_path())
    );

    panel.state.task_mut("Start").unwrap().threshold = 0.95;
    panel.save(&mut status);
    assert!(
        status.contains("シナリオ保存"),
        "second save must pass: {status}"
    );
}

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

    assert!(dir.join("pipeline.toml").exists());
    assert!(dir.join("Start.toml").exists());
    assert!(dir.join("Loop.toml").exists());
    assert!(dir.join("Start.png").exists());
    assert!(dir.join("Loop.png").exists());

    let manifest = anaden_vision::load_pipeline_manifest(&dir).expect("load manifest");
    assert_eq!(manifest.start_task, "Start");
    assert_eq!(manifest.goals.len(), 2);
    assert_eq!(
        manifest.goals[0].stop,
        StopCondition::LoopCount { target: 3 }
    );
    assert_eq!(manifest.goals[1].stop, StopCondition::Timeout { secs: 600 });

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

#[test]
fn panel_flow_candidate_add_goal_save_roundtrip() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("pipelines");
    let mut panel = ScenarioPanel::new(root.clone());
    panel.state.name = "panel_flow".to_string();

    let (tap, tap_png) = gui_candidate("TapA", ScreenRegion::new(10, 20, 100, 50));
    panel.add_candidate(tap, tap_png).expect("add TapA");
    let (dup, dup_png) = gui_candidate("TapA", ScreenRegion::new(0, 0, 30, 30));
    assert!(
        panel.add_candidate(dup, dup_png).is_err(),
        "同名タスクの重複追加は拒否"
    );
    let (wait, wait_png) = gui_candidate("WaitB", ScreenRegion::new(5, 5, 40, 40));
    panel.add_candidate(wait, wait_png).expect("add WaitB");

    panel.state.task_mut("TapA").unwrap().next = Some(vec!["WaitB".to_string()]);
    panel.state.add_goal(Goal {
        name: "loop5".to_string(),
        stop: StopCondition::LoopCount { target: 5 },
    });

    let mut status = String::new();
    let ctx = egui::Context::default();
    ctx.begin_pass(egui::RawInput::default());
    let (extra, extra_png) = gui_candidate("Extra", ScreenRegion::new(1, 1, 20, 20));
    panel.ui(&mut child_ui(&ctx), Some((extra, extra_png)), &mut status);
    panel.ui(&mut child_ui(&ctx), None, &mut status);
    let _ = ctx.end_pass();

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
