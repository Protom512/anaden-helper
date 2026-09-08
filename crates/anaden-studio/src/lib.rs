//! anaden-studio ライブラリ部。
//!
//! 元々 bin 専用クレートだったが、Issue #83 UC-3 異常系ヘッドレス統合テスト
//! (`tests/pipeline_error_paths_tests.rs`) から state machine モジュールを
//! 参照するため、モジュール群を lib として公開する。GUI のエントリポイント
//! (main / eframe 起動 / フォント登録) は main.rs に残る。

pub mod app;
pub mod app_state;
pub mod app_state_connection;
pub mod app_state_core;
pub mod app_state_pipeline_task;
pub mod app_ui;
pub mod app_ui_authoring;
pub mod app_ui_batch;
pub mod app_ui_body;
pub mod app_ui_tasks;
pub mod batch;
pub mod canvas;
pub mod childproc;
pub mod cli;
pub mod history;
pub mod history_ui;
pub mod library;
pub mod log_buffer;
pub mod log_process;
pub mod log_status;
pub mod log_view;
pub mod proposals;
pub mod runner;
pub mod runner_exec;
pub mod runner_ui;
pub mod scenario_editor;
pub mod scenario_load;
pub mod scenario_panel;
pub mod scenario_save;
pub mod scenario_state;
pub mod scenario_task_link;
pub mod scenario_ui;
pub mod scenario_validate;
pub mod scoring;
pub mod settings;
pub mod shell;
pub mod shell_nav;
pub mod source;
pub mod strategy_ui;
pub mod tasks;
pub mod tasks_def;
pub mod tasks_queue;
pub mod tasks_toml;
pub mod tasks_view;
