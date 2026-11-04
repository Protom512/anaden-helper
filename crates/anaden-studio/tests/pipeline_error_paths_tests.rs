#![allow(clippy::unwrap_used)]
#![allow(clippy::panic)]
#![allow(clippy::expect_used)]

use anaden_studio::log_view::LogLevel;
use anaden_studio::runner::{PipelineRunnerApp, RunnerStatus};

fn exit_nonzero_args() -> Vec<String> {
    if cfg!(windows) {
        vec!["/C".to_string(), "exit".to_string(), "3".to_string()]
    } else {
        vec!["-c".to_string(), "exit 3".to_string()]
    }
}

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

fn wait_for_exit_and_drain(app: &mut PipelineRunnerApp) {
    for _ in 0..300 {
        if app.status() == RunnerStatus::Stopped {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
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

#[test]
fn immediate_crash_stays_stopped_without_panic() {
    let mut app = PipelineRunnerApp::new(cmd_or_sh());
    app.start_pipeline(&exit_nonzero_args());
    for _ in 0..100 {
        if app.status() == RunnerStatus::Stopped {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert_eq!(app.status(), RunnerStatus::Stopped);
    app.drain_logs();
    app.clear_logs();
    assert_eq!(app.status(), RunnerStatus::Stopped);
}

#[test]
fn massive_stderr_output_does_not_panic_and_caps_buffer() {
    let mut app = PipelineRunnerApp::new(cmd_or_sh());
    app.start_pipeline(&massive_stderr_args());
    wait_for_exit_and_drain(&mut app);

    assert_eq!(app.status(), RunnerStatus::Stopped);
    assert!(
        app.log_snapshot()
            .iter()
            .any(|e| e.line.contains("[stderr] ")),
        "stderr lines should be present"
    );
    assert!(
        app.log_snapshot().len() <= 5000,
        "buffer should be capped, got {}",
        app.log_snapshot().len()
    );
}

#[test]
fn restart_after_abnormal_exit_succeeds() {
    let mut app = PipelineRunnerApp::new(cmd_or_sh());
    app.start_pipeline(&exit_nonzero_args());
    wait_for_exit_and_drain(&mut app);
    assert_eq!(app.status(), RunnerStatus::Stopped);

    app.start_pipeline(&exit_nonzero_args());
    wait_for_exit_and_drain(&mut app);
    assert_eq!(app.status(), RunnerStatus::Stopped);
}

fn cmd_or_sh() -> &'static str {
    if cfg!(windows) { "cmd" } else { "sh" }
}
