//! Pipeline 実行ランナーGUI: runner_exec / runner_ui への facade (Issue #168)。
//!
//! - runner_exec.rs: 実行状態機械 (egui 非依存)。`PipelineRunnerApp` (子プロセス
//!   管理・ログ drain/スナップショット・履歴追記・設定 I/O) / `RunnerStatus` /
//!   `RunnerPane` / `pick_anaden_bin` / `build_spawn_spec` / `build_run_args` /
//!   `resolve_pipeline_arg`。
//! - runner_ui.rs: egui パネル層。`render_body` 系 (Run/History/Strategy/Settings
//!   の 4 ペイン描画)・ログビューア・`level_color`・`eframe::App` 実装。
//!
//! 呼び出し元 (shell / shell_nav / app_ui / tests) は従来どおり
//! `crate::runner::{...}` (`anaden_studio::runner::{...}`) で参照できる
//! (`app` / `scenario_ui` facade と同一パターン・Issue #162/#166)。

pub use crate::runner_exec::{
    PipelineRunnerApp, RunnerPane, RunnerStatus, build_run_args, build_spawn_spec, pick_anaden_bin,
    resolve_pipeline_arg,
};
pub(crate) use crate::runner_ui::level_color;
