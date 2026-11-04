#![allow(clippy::unwrap_used)]
#![allow(clippy::panic)]
#![allow(clippy::expect_used)]

use std::time::{Duration, Instant};

use anaden_studio::app::StudioApp;
use anaden_studio::childproc::SpawnSpec;
use anaden_studio::tasks::{QueueEntry, QueueState};

fn shell_program() -> &'static str {
    if cfg!(windows) { "cmd" } else { "sh" }
}

fn marker_spec(marker: &str, delay_secs: u32, code: i32) -> SpawnSpec {
    let script = if cfg!(windows) {
        if delay_secs == 0 {
            format!("echo {marker} & exit {code}")
        } else {
            format!(
                "echo {marker} & ping -n {} 127.0.0.1 > nul & exit {code}",
                delay_secs + 1
            )
        }
    } else if delay_secs == 0 {
        format!("echo {marker}; exit {code}")
    } else {
        format!("echo {marker}; sleep {delay_secs}; exit {code}")
    };
    let flag = if cfg!(windows) { "/C" } else { "-c" };
    SpawnSpec::new(shell_program(), [flag.to_string(), script])
}

fn queue_entry(label: &str, spec: SpawnSpec) -> QueueEntry {
    QueueEntry {
        label: label.to_string(),
        spec,
    }
}

fn log_lines(app: &StudioApp) -> Vec<&str> {
    app.task_log_lines()
        .iter()
        .map(|e| e.line.as_str())
        .collect()
}

fn separator_count(app: &StudioApp) -> usize {
    app.task_log_lines()
        .iter()
        .filter(|e| e.line.starts_with("[studio] === task:"))
        .count()
}

fn pump_until(
    app: &mut StudioApp,
    timeout: Duration,
    done: impl Fn(&QueueState) -> bool,
    what: &str,
) {
    let deadline = Instant::now() + timeout;
    loop {
        app.drain_task_logs();
        if let Some(q) = app.task_queue()
            && done(q.state())
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}: state={:?}",
            app.task_queue().map(|q| q.state().clone())
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn sequential_queue_runs_tasks_in_check_order_observing_each_exit() {
    let mut app = StudioApp::default();
    app.start_task_entries(vec![
        queue_entry("タスクA", marker_spec("marker-A", 0, 0)),
        queue_entry("タスクB", marker_spec("marker-B", 0, 0)),
        queue_entry("タスクC", marker_spec("marker-C", 0, 0)),
    ]);

    let mut observed_current: Vec<usize> = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        app.drain_task_logs();
        if let Some(q) = app.task_queue() {
            if let QueueState::Running { current } = q.state()
                && observed_current.last() != Some(current)
            {
                observed_current.push(*current);
            }
            if matches!(q.state(), QueueState::Completed) {
                break;
            }
        }
        assert!(
            Instant::now() < deadline,
            "timed out; observed_current={observed_current:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    assert!(
        observed_current.windows(2).all(|w| w[0] < w[1]),
        "non-monotonic current sequence: {observed_current:?}"
    );

    let queue = app.task_queue().unwrap();
    assert!(matches!(queue.state(), QueueState::Completed));
    assert_eq!(queue.summary(), "完了 3/3");
    assert!(!queue.is_aborted());

    let lines = log_lines(&app);
    let pos = |needle: &str| lines.iter().position(|l| l.contains(needle));
    let sep_a = pos("=== task: タスクA ===").expect("task A separator missing");
    let sep_b = pos("=== task: タスクB ===").expect("task B separator missing");
    let sep_c = pos("=== task: タスクC ===").expect("task C separator missing");
    assert!(
        sep_a < sep_b && sep_b < sep_c,
        "check order violated: {lines:?}"
    );

    let exit_positions: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.contains("プロセス終了"))
        .map(|(i, _)| i)
        .collect();
    assert_eq!(exit_positions.len(), 3, "lines: {lines:?}");
    assert!(
        exit_positions[0] < sep_b,
        "task A exit must precede B start: {lines:?}"
    );
    assert!(
        exit_positions[1] < sep_c,
        "task B exit must precede C start: {lines:?}"
    );
    let marker_a = pos("marker-A").expect("marker-A missing");
    let marker_b = pos("marker-B").expect("marker-B missing");
    let marker_c = pos("marker-C").expect("marker-C missing");
    assert!(
        sep_a < marker_a && marker_a < exit_positions[0],
        "{lines:?}"
    );
    assert!(
        sep_b < marker_b && marker_b < exit_positions[1],
        "{lines:?}"
    );
    assert!(
        sep_c < marker_c && marker_c < exit_positions[2],
        "{lines:?}"
    );
    assert!(lines.iter().any(|l| l.contains("キュー完了")));
}

#[test]
fn running_child_output_reaches_log_buffer_before_exit_observation() {
    let mut app = StudioApp::default();
    app.start_task_entries(vec![queue_entry(
        "長時間タスク",
        marker_spec("rt-marker", 3, 0),
    )]);

    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        app.drain_task_logs();
        let running = app
            .task_queue()
            .is_some_and(|q| matches!(q.state(), QueueState::Running { .. }));
        if running && log_lines(&app).iter().any(|l| l.contains("rt-marker")) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "marker not drained while running: {:?}",
            log_lines(&app)
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    assert!(
        !log_lines(&app).iter().any(|l| l.contains("プロセス終了")),
        "exit must not be observed yet: {:?}",
        log_lines(&app)
    );

    app.abort_task_queue();
    let queue = app.task_queue().unwrap();
    assert!(matches!(queue.state(), QueueState::Completed));
    assert!(queue.is_aborted());
}

#[test]
fn nonzero_exit_pauses_and_pumping_frames_does_not_start_next_task() {
    let mut app = StudioApp::default();
    app.start_task_entries(vec![
        queue_entry("失敗タスク", marker_spec("fail-marker", 0, 2)),
        queue_entry("後続タスク", marker_spec("followup-marker", 0, 0)),
    ]);
    pump_until(
        &mut app,
        Duration::from_secs(30),
        |s| matches!(s, QueueState::PausedAfterFailure { .. }),
        "failure pause",
    );

    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        app.drain_task_logs();
        let queue = app.task_queue().unwrap();
        assert!(
            matches!(
                queue.state(),
                QueueState::PausedAfterFailure {
                    current: 0,
                    exit_code: Some(2)
                }
            ),
            "state must stay paused: {:?}",
            queue.state()
        );
        assert_eq!(separator_count(&app), 1, "後続タスクは起動してはならない");
        assert!(
            !log_lines(&app)
                .iter()
                .any(|l| l.contains("followup-marker")),
            "後続タスクの出力があってはならない: {:?}",
            log_lines(&app)
        );
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    let summary = app.task_queue().unwrap().summary();
    assert!(summary.contains("失敗停止 1/2"), "summary: {summary}");
    assert!(summary.contains("exit=2"), "summary: {summary}");
    assert!(
        log_lines(&app)
            .iter()
            .any(|l| l.contains("exit=エラー") && l.contains("(code=Some(2))")),
        "exit line must record code 2: {:?}",
        log_lines(&app)
    );

    app.abort_task_queue();
}

#[test]
fn explicit_resume_runs_remaining_tasks_to_completion() {
    let mut app = StudioApp::default();
    app.start_task_entries(vec![
        queue_entry("失敗タスク", marker_spec("first-fail", 0, 1)),
        queue_entry("後続タスク", marker_spec("resumed-marker", 0, 0)),
    ]);
    pump_until(
        &mut app,
        Duration::from_secs(30),
        |s| matches!(s, QueueState::PausedAfterFailure { .. }),
        "failure pause",
    );
    assert_eq!(separator_count(&app), 1);

    app.resume_task_queue();
    pump_until(
        &mut app,
        Duration::from_secs(30),
        |s| matches!(s, QueueState::Completed),
        "resume completion",
    );

    let queue = app.task_queue().unwrap();
    assert!(matches!(queue.state(), QueueState::Completed));
    assert!(!queue.is_aborted());
    assert_eq!(queue.summary(), "完了 2/2");
    assert_eq!(separator_count(&app), 2, "両タスクが Start 済みであること");
    let lines = log_lines(&app);
    assert!(
        lines.iter().any(|l| l.contains("resumed-marker")),
        "resumed task output missing: {lines:?}"
    );
    assert!(
        lines.iter().any(|l| l.contains("キュー完了")),
        "completion line missing: {lines:?}"
    );
}

#[test]
fn empty_queue_start_is_rejected_without_spawning() {
    let mut app = StudioApp::default();
    app.start_task_entries(Vec::new());
    assert!(
        app.task_queue().is_none(),
        "空キューでキューが作られてはならない"
    );
    assert!(
        app.task_log_lines().is_empty(),
        "空キュー開始でログに出てはならない: {:?}",
        log_lines(&app)
    );

    app.start_task_entries(vec![queue_entry(
        "即終了タスク",
        marker_spec("after-reject", 0, 0),
    )]);
    assert!(app.task_queue().is_some());
    pump_until(
        &mut app,
        Duration::from_secs(30),
        |s| matches!(s, QueueState::Completed),
        "post-rejection completion",
    );
    assert_eq!(app.task_queue().unwrap().summary(), "完了 1/1");
}

#[test]
fn abort_terminates_queue_and_survives_killed_child_exit_drain() {
    let mut app = StudioApp::default();
    app.start_task_entries(vec![
        queue_entry("長時間タスク", marker_spec("abort-marker", 3, 0)),
        queue_entry("後続タスク", marker_spec("post-abort-marker", 0, 0)),
    ]);
    assert!(
        app.task_queue()
            .is_some_and(|q| matches!(q.state(), QueueState::Running { current: 0 }))
    );

    app.abort_task_queue();
    let queue = app.task_queue().unwrap();
    assert!(matches!(queue.state(), QueueState::Completed));
    assert!(queue.is_aborted());
    assert_eq!(queue.summary(), "中止");
    assert_eq!(queue.total(), 0);
    assert!(queue.entries().is_empty());

    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        app.drain_task_logs();
        if log_lines(&app).iter().any(|l| l.contains("プロセス終了")) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "killed child exit not drained: {:?}",
            log_lines(&app)
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    let queue = app.task_queue().unwrap();
    assert!(
        matches!(queue.state(), QueueState::Completed),
        "post-abort drain must not resurrect the queue"
    );
    assert!(queue.is_aborted());
    assert_eq!(separator_count(&app), 1, "残りタスクは起動してはならない");
    assert!(
        !log_lines(&app)
            .iter()
            .any(|l| l.contains("post-abort-marker")),
        "残りタスクの出力があってはならない: {:?}",
        log_lines(&app)
    );
    assert!(
        log_lines(&app).iter().any(|l| l.contains("キュー中止")),
        "abort line missing: {:?}",
        log_lines(&app)
    );
}
