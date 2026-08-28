//! Issue #83 UC-3 異常系ヘッドレス統合テスト。
//!
//! 子プロセスが以下の異常終了をした際に、GUI 側 state machine
//! (`runner::PipelineRunnerApp` + `log_view` の状態) が
//! **panic せず** Stopped へ遷移し、失敗がログへ反映されることを検証する:
//!
//! - 非ゼロ exit code で終了
//! - 起動直後に即時クラッシュ(何も出力せず即 exit)
//! - 大量の stderr 出力(チャネル飽和)を伴う終了
//!
//! eframe ウィンドウは生成しない state-machine レベルのテストのため CI で
//! ヘッドレス実行可能 (承認条件)。実際のウィンドウ操作を伴う手動 E2E は
//! wiki の再現手順を参照。

#![allow(clippy::unwrap_used)]
#![allow(clippy::panic)]
#![allow(clippy::expect_used)]

use anaden_studio::log_view::LogLevel;
use anaden_studio::runner::{PipelineRunnerApp, RunnerStatus};

/// 非ゼロ exit で終了する子の引数。
///
/// `cmd /C exit 3`(Windows) / `sh -c "exit 3"`(Unix)。
fn exit_nonzero_args() -> Vec<String> {
    if cfg!(windows) {
        vec!["/C".to_string(), "exit".to_string(), "3".to_string()]
    } else {
        vec!["-c".to_string(), "exit 3".to_string()]
    }
}

/// stderr に大量出力して非ゼロ exit する子の引数。
///
/// Windows の cmd は遅いため行数を抑えめに、それでもチャネル容量
/// (1024) を確実に超える件数を出力する。
fn massive_stderr_args() -> Vec<String> {
    if cfg!(windows) {
        vec![
            "/C".to_string(),
            "for /L %i in (1,1,4000) do @echo eeeeeeeeeeeeeeeeeeeeeeeeeeee 1>&2 & exit /b 5"
                .to_string(),
        ]
    } else {
        vec![
            "-c".to_string(),
            "i=0; while [ $i -lt 6000 ]; do echo eeeeeeeeeeeeeeeeeeeeeeeeeeee >&2; i=$((i+1)); done; exit 5"
                .to_string(),
        ]
    }
}

/// 子の自然終了(非ゼロ exit) + ログ drain を待つ共通ヘルパ。
fn wait_for_exit_and_drain(app: &mut PipelineRunnerApp) {
    for _ in 0..300 {
        if app.status() == RunnerStatus::Stopped {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    // 終了後、reader スレッドの Exit イベントが届くまで drain を続ける。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        app.drain_logs();
        if app
            .log_snapshot()
            .iter()
            .any(|e| e.line.contains("プロセス終了"))
        {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "Exit イベントが drain されない: {:?}",
            app.log_snapshot()
                .iter()
                .map(|e| e.line.clone())
                .collect::<Vec<_>>()
                .last()
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// UC-3: 子が非ゼロ exit しても GUI state machine は panic せず
/// Stopped に遷移し、exit 行がログへ Error レベルで残る。
#[test]
fn nonzero_exit_transitions_to_stopped_without_panic() {
    let mut app = PipelineRunnerApp::new(cmd_or_sh());
    app.start_pipeline(&exit_nonzero_args());
    wait_for_exit_and_drain(&mut app);

    assert_eq!(app.status(), RunnerStatus::Stopped);
    let exit_lines: Vec<_> = app
        .log_snapshot()
        .iter()
        .filter(|e| e.line.contains("プロセス終了"))
        .collect();
    let last = exit_lines.last().expect("exit line should be drained");
    assert!(
        last.line.contains("exit=エラー"),
        "unexpected exit line: {}",
        last.line
    );
    assert_eq!(last.level, LogLevel::Error);
}

/// UC-3: 起動直後にクラッシュ(即時 exit・出力なし)しても panic しない。
#[test]
fn immediate_crash_stays_stopped_without_panic() {
    let mut app = PipelineRunnerApp::new(cmd_or_sh());
    app.start_pipeline(&exit_nonzero_args());
    // 即時終了を待つ。
    for _ in 0..100 {
        if app.status() == RunnerStatus::Stopped {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert_eq!(app.status(), RunnerStatus::Stopped);
    // 停止後もハンドラ呼び出しが panic しない。
    app.drain_logs();
    app.clear_logs();
    assert_eq!(app.status(), RunnerStatus::Stopped);
}

/// UC-3: 大量 stderr 出力(チャネル容量超過)でも reader は継続し、
/// GUI は panic せず Stopped に遷移する。ログバッファは上限で打ち切られる。
#[test]
fn massive_stderr_output_does_not_panic_and_caps_buffer() {
    let mut app = PipelineRunnerApp::new(cmd_or_sh());
    app.start_pipeline(&massive_stderr_args());
    wait_for_exit_and_drain(&mut app);

    assert_eq!(app.status(), RunnerStatus::Stopped);
    // stderr 行は接頭辞付きで届いている。
    assert!(
        app.log_snapshot()
            .iter()
            .any(|e| e.line.contains("[stderr] ")),
        "stderr lines should be present"
    );
    // バッファ上限 (DEFAULT_MAX_LINES=5000) 以下に打ち切られる。
    assert!(
        app.log_snapshot().len() <= 5000,
        "buffer should be capped, got {}",
        app.log_snapshot().len()
    );
}

/// UC-3: 異常終了後に再起動できる(Stopped 状態からの復帰)。
#[test]
fn restart_after_abnormal_exit_succeeds() {
    let mut app = PipelineRunnerApp::new(cmd_or_sh());
    app.start_pipeline(&exit_nonzero_args());
    wait_for_exit_and_drain(&mut app);
    assert_eq!(app.status(), RunnerStatus::Stopped);

    // 再起動。start は自然終了済み子を既に回収済みのため Err にならない。
    app.start_pipeline(&exit_nonzero_args());
    wait_for_exit_and_drain(&mut app);
    assert_eq!(app.status(), RunnerStatus::Stopped);
}

/// プラットフォーム別の解釈付きプロセス名。
fn cmd_or_sh() -> &'static str {
    if cfg!(windows) { "cmd" } else { "sh" }
}
