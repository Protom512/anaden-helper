//! Issue #160 UC-3 (Shard 4) 統合テスト — 保存 → タスク登録 → 有効化のパネルフロー。
//!
//! MAA 4 段階フロー (a) pipeline 作成 → (b) task エントリ登録 → (c) 有効化 →
//! (d) queue 追加 (ホーム一覧) を GUI パネル経由で通し検証する。
//! `load_task_definitions` 再読込後に有効化タスクが選択可能 (selectable) になる
//! ことを機械保証する (UC-3 受入)。ヘッドレス (ウィンドウ生成なし)。

#![allow(clippy::unwrap_used)]
#![allow(clippy::panic)]
#![allow(clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};

use anaden_core::{Goal, ScreenRegion, StopCondition};
use anaden_studio::app::{PipelineActionKind, pipeline_task_spec};
use anaden_studio::scenario_task_link::{ScenarioPanelEvent, StubTaskOption, TaskLinkContext};
use anaden_studio::scenario_ui::ScenarioPanel;
use anaden_studio::tasks::TaskDefinition;
use anaden_vision::TaskDef;
use image::{DynamicImage, GrayImage, Luma};

/// 単色 GrayImage (scenario_editor_tests.rs と同構成)。
fn solid_image(w: u32, h: u32, v: u8) -> DynamicImage {
    DynamicImage::ImageLuma8(GrayImage::from_pixel(w, h, Luma([v])))
}

/// Authoring ペインと同一導出の追加候補 (pipeline_task_spec + crop PNG)。
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
    (task, solid_image(roi.width, roi.height, 160))
}

/// ヘッドレス egui コンテキスト内に子 Ui を作る (既存パターン)。
fn child_ui(ctx: &egui::Context) -> egui::Ui {
    egui::Ui::new(
        ctx.clone(),
        egui::Id::new("scenario-task-link-test-area"),
        egui::UiBuilder::new().max_rect(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(800.0, 600.0),
        )),
    )
}

/// 保存 → 新規登録・有効化 → ホーム一覧反映の通しフロー (UC-3 受入)。
#[test]
fn panel_flow_save_register_enable_reflects_in_home_list() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let pipelines_root = root.join("templates/pipelines");
    let tasks_dir = root.join("templates/tasks");
    fs::create_dir_all(&tasks_dir).expect("mkdir tasks");

    // (a) シナリオ作成・保存 (パネル経由)。
    let mut panel = ScenarioPanel::new(pipelines_root.clone());
    panel.state.name = "uc3_flow".to_string();
    let (tap, tap_png) = gui_candidate("TapMain", ScreenRegion::new(10, 20, 100, 50));
    panel.add_candidate(tap, tap_png).expect("add");
    let (wait, wait_png) = gui_candidate("WaitNext", ScreenRegion::new(5, 5, 40, 40));
    panel.add_candidate(wait, wait_png).expect("add");
    panel.state.task_mut("TapMain").unwrap().next = Some(vec!["WaitNext".to_string()]);
    panel.state.add_goal(Goal {
        name: "loop5".to_string(),
        stop: StopCondition::LoopCount { target: 5 },
    });
    let mut status = String::new();
    panel.save(&mut status);
    assert!(status.contains("シナリオ保存"), "status: {status}");
    assert_eq!(
        panel.saved_pipeline(),
        Some(pipelines_root.join("uc3_flow").as_path())
    );

    // 登録セクションのヘッドレス描画がパニック無し (stub あり/なし両方)。
    let egui_ctx = egui::Context::default();
    let stubs = vec![StubTaskOption {
        id: "stub_x".to_string(),
        title: "スタブX".to_string(),
    }];
    let ctx = TaskLinkContext {
        root,
        tasks_dir: &tasks_dir,
        stubs: &stubs,
    };
    egui_ctx.begin_pass(egui::RawInput::default());
    let ev = panel.ui_task_link(&mut child_ui(&egui_ctx), &ctx, &mut status);
    let _ = egui_ctx.end_pass();
    assert!(ev.is_none(), "描画だけではイベント無し: {ev:?}");
    let empty_ctx = TaskLinkContext {
        root,
        tasks_dir: &tasks_dir,
        stubs: &[],
    };
    egui_ctx.begin_pass(egui::RawInput::default());
    let _ = panel.ui_task_link(&mut child_ui(&egui_ctx), &empty_ctx, &mut status);
    let _ = egui_ctx.end_pass();

    // (b)+(c) 新規タスクとして登録・有効化 (ボタン実体 = 既存 save() パターン)。
    let ev = panel.register_as_new_task(&ctx, &mut status);
    assert!(
        matches!(ev, Some(ScenarioPanelEvent::TaskEnabled { .. })),
        "ev: {ev:?}"
    );
    assert!(status.contains("タスク登録・有効化"), "status: {status}");

    // (d) ホーム一覧反映: load_task_definitions 再読込で選択可能になる。
    let defs = anaden_studio::tasks::load_task_definitions(&tasks_dir).expect("reload");
    let def = defs
        .iter()
        .find(|d| d.id == "uc3_flow")
        .unwrap_or_else(|| panic!("uc3_flow must be listed: {defs:?}"));
    assert!(def.is_selectable(), "有効化タスクは選択可能: {def:?}");
    assert_eq!(def.start_task.as_deref(), Some("TapMain"));

    // (ii) 既存 stub への紐付け+有効化経路 (テンポラリ stub TOML)。
    fs::write(
        tasks_dir.join("stub_x.toml"),
        "# stub\nid = \"stub_x\"\ntitle = \"スタブX\"\nkind = \"pipeline_run\"\n\
         implemented = false\npipeline_dir = \"templates/pipelines/stub_x\"\n",
    )
    .expect("write stub");
    panel.bind_selection = Some("stub_x".to_string());
    let ev = panel.bind_stub_task(&ctx, &mut status);
    assert!(
        matches!(ev, Some(ScenarioPanelEvent::TaskEnabled { .. })),
        "ev: {ev:?}"
    );
    assert!(status.contains("タスク有効化"), "status: {status}");
    let bound = TaskDefinition::parse_toml(
        &fs::read_to_string(tasks_dir.join("stub_x.toml")).expect("read"),
        Path::new("stub_x.toml"),
    )
    .expect("reparse");
    assert!(bound.implemented && bound.is_selectable());
    assert_eq!(
        bound.pipeline_dir,
        Some(PathBuf::from("templates/pipelines/uc3_flow"))
    );
}

/// fail-closed: 未保存での登録・紐付けは status エラーとなりイベントを返さない。
#[test]
fn register_before_save_reports_error_without_event() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let tasks_dir = root.join("templates/tasks");
    let ctx = TaskLinkContext {
        root,
        tasks_dir: &tasks_dir,
        stubs: &[],
    };
    let mut panel = ScenarioPanel::new(root.join("templates/pipelines"));
    let mut status = String::new();

    assert!(panel.register_as_new_task(&ctx, &mut status).is_none());
    assert!(status.contains("保存"), "status: {status}");

    // 紐付けも未保存・未選択で拒否。
    let mut status2 = String::new();
    assert!(panel.bind_stub_task(&ctx, &mut status2).is_none());
    assert!(status2.contains("保存"), "status: {status2}");
}

/// fail-closed: 同名 task TOML 既存時の再登録は上書き拒否 (バイト不変)・
/// 不正 task id は UnsafeTaskId として status へ伝播する。
#[test]
fn panel_register_failures_propagate_to_status() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let pipelines_root = root.join("templates/pipelines");
    let tasks_dir = root.join("templates/tasks");
    fs::create_dir_all(&tasks_dir).expect("mkdir");

    let mut panel = ScenarioPanel::new(pipelines_root.clone());
    panel.state.name = "dup_flow".to_string();
    let (tap, tap_png) = gui_candidate("TapMain", ScreenRegion::new(10, 20, 100, 50));
    panel.add_candidate(tap, tap_png).expect("add");
    panel.state.add_goal(Goal {
        name: "loop5".to_string(),
        stop: StopCondition::LoopCount { target: 5 },
    });
    let mut status = String::new();
    panel.save(&mut status);
    assert!(status.contains("シナリオ保存"));

    // 既存 TOML (上書き対象 — 変更されてはならない)。
    let existing = tasks_dir.join("dup_flow.toml");
    let original = "# 既存\nid = \"dup_flow\"\ntitle = \"既存\"\nkind = \"launch_subcommand\"\n\
                    implemented = true\n";
    fs::write(&existing, original).expect("write existing");

    let ctx = TaskLinkContext {
        root: tmp.path(),
        tasks_dir: &tasks_dir,
        stubs: &[],
    };
    assert!(panel.register_as_new_task(&ctx, &mut status).is_none());
    assert!(
        status.contains("already exists"),
        "上書き拒否が status へ: {status}"
    );
    assert_eq!(
        fs::read_to_string(&existing).expect("read"),
        original,
        "既存ファイルは 1 バイトも変わらない"
    );

    // 不正 task id (パス区切り) も status へ fail-closed。
    panel.task_id_input = "a/b".to_string();
    assert!(panel.register_as_new_task(&ctx, &mut status).is_none());
    assert!(
        status.contains("not a safe task file name"),
        "UnsafeTaskId が status へ: {status}"
    );
    assert!(!tasks_dir.join("a").exists(), "パス区切りで親脱出しない");
}
