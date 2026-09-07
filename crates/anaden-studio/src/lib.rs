//! anaden-studio ライブラリ部。
//!
//! 元々 bin 専用クレートだったが、Issue #83 UC-3 異常系ヘッドレス統合テスト
//! (`tests/pipeline_error_paths_tests.rs`) から state machine モジュールを
//! 参照するため、モジュール群を lib として公開する。GUI のエントリポイント
//! (main / eframe 起動 / フォント登録) は main.rs に残る。

pub mod app;
pub mod app_state;
pub mod app_ui;
pub mod batch;
pub mod canvas;
pub mod childproc;
pub mod cli;
pub mod history;
pub mod history_ui;
pub mod library;
pub mod log_view;
pub mod proposals;
pub mod runner;
pub mod scenario_load;
pub mod scenario_task_link;
pub mod scenario_ui;
pub mod scoring;
pub mod settings;
pub mod shell;
pub mod shell_nav;
pub mod source;
pub mod strategy_ui;
pub mod tasks;
pub mod tasks_toml;
pub mod tasks_view;
