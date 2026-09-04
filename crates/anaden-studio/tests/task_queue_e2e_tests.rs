//! Issue #154 Shard 3 (前半): チェック順逐次実行キューのヘッドレス E2E テスト。
//!
//! 実子プロセス (OS スタブ: Windows `cmd /C` / Unix `sh -c`) を起動し、
//! `StudioApp` の公開 API (`start_task_entries` / `drain_task_logs` /
//! `resume_task_queue` / `abort_task_queue` / `task_queue` / `task_log_lines`)
//! 経由で Issue #154 の受け入れ基準を機械検証する:
//!
//! 1. 逐次実行: 複数タスクがチェック順に、前タスクの Exit 観測後に次が
//!    Start される (セパレータ行・Exit 行・stdout マーカのログ順で機械検証)。
//! 2. リアルタイム drain: 実行中タスクの stdout が Exit 観測前に LogBuffer
//!    へ反映される。
//! 3. 失敗停止: 非零 exit で PausedAfterFailure に停止し、明示 resume 操作まで
//!    フレーム (drain) を回し続けても後続タスクが起動しない。
//! 4. 継続完了: resume 後に残タスクが実行され Completed に到達する。
//! 5. エッジ: 空キュー開始の拒否・abort 後の状態 (kill された子の Exit drain
//!    が流入しても状態は崩れない)。
//!
//! eframe ウィンドウを生成しない state machine レベルのテストのため CI で
//! ヘッドレス実行可能 (`tests/pipeline_error_paths_tests.rs` と同一パターン)。
//! 実 anaden 実行バイナリ・実機ゲームは扱わない (OS スタブのみ)。

#![allow(clippy::unwrap_used)]
#![allow(clippy::panic)]
#![allow(clippy::expect_used)]

use std::time::{Duration, Instant};

use anaden_studio::app::StudioApp;
use anaden_studio::childproc::SpawnSpec;
use anaden_studio::tasks::{QueueEntry, QueueState};

/// OS 標準シェル (スタブ子プロセスの program)。
fn shell_program() -> &'static str {
    if cfg!(windows) { "cmd" } else { "sh" }
}

/// 一意マーカ行を stdout へ出力し、`delay_secs` 秒待ってから exit `code` で
/// 終了する OS スタブ子プロセスの SpawnSpec。
///
/// - Windows: `cmd /C "echo MARKER & ping -n N 127.0.0.1 > nul & exit CODE"`
///   (ping は両 OS に存在するスリープ代替。`-n N` で約 N-1 秒待つ)
/// - Unix: `sh -c "echo MARKER; sleep N; exit CODE"`
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

/// 表示ラベル付きキューエントリ。
fn queue_entry(label: &str, spec: SpawnSpec) -> QueueEntry {
    QueueEntry {
        label: label.to_string(),
        spec,
    }
}

/// タスク実行ログの行テキスト一覧。
fn log_lines(app: &StudioApp) -> Vec<&str> {
    app.task_log_lines()
        .iter()
        .map(|e| e.line.as_str())
        .collect()
}

/// タスク境界セパレータ行の数 (= Start 済みタスク数)。
fn separator_count(app: &StudioApp) -> usize {
    app.task_log_lines()
        .iter()
        .filter(|e| e.line.starts_with("[studio] === task:"))
        .count()
}

/// drain (UI フレーム相当) を回しつつ指定状態になるまで待つ。タイムアウトで fail。
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

// ---- 受け入れ基準 1: 逐次実行 (チェック順・Exit 観測後に次が Start) ----

/// 3 タスクはチェック順 (A → B → C) に実行され、各タスクの Exit 行が次タスクの
/// セパレータ (Start) より前に現れる — 前タスクの Exit 観測後に次が Start され
/// ていることのログ順機械検証。全完了にも到達する。
#[test]
fn sequential_queue_runs_tasks_in_check_order_observing_each_exit() {
    let mut app = StudioApp::default();
    app.start_task_entries(vec![
        queue_entry("タスクA", marker_spec("marker-A", 0, 0)),
        queue_entry("タスクB", marker_spec("marker-B", 0, 0)),
        queue_entry("タスクC", marker_spec("marker-C", 0, 0)),
    ]);

    // drain を回して完了へ。実行中 current の観測列も記録する (単調性検証用)。
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

    // 実行中タスク位置の観測列は狭義単調 (前タスク実行中に次へ進まない)。
    assert!(
        observed_current.windows(2).all(|w| w[0] < w[1]),
        "non-monotonic current sequence: {observed_current:?}"
    );

    // 完了状態と進行サマリ。
    let queue = app.task_queue().unwrap();
    assert!(matches!(queue.state(), QueueState::Completed));
    assert_eq!(queue.summary(), "完了 3/3");
    assert!(!queue.is_aborted());

    // ログ順の機械検証: セパレータ (Start)・Exit 行・stdout マーカの順序。
    let lines = log_lines(&app);
    let pos = |needle: &str| lines.iter().position(|l| l.contains(needle));
    let sep_a = pos("=== task: タスクA ===").expect("task A separator missing");
    let sep_b = pos("=== task: タスクB ===").expect("task B separator missing");
    let sep_c = pos("=== task: タスクC ===").expect("task C separator missing");
    assert!(
        sep_a < sep_b && sep_b < sep_c,
        "check order violated: {lines:?}"
    );

    // Exit 行 ([studio] プロセス終了) は 3 回・各タスク 1 回ずつ。
    let exit_positions: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.contains("プロセス終了"))
        .map(|(i, _)| i)
        .collect();
    assert_eq!(exit_positions.len(), 3, "lines: {lines:?}");
    // 前タスクの Exit 行が次タスクの Start (セパレータ) より前 = Exit 観測後に Start。
    assert!(
        exit_positions[0] < sep_b,
        "task A exit must precede B start: {lines:?}"
    );
    assert!(
        exit_positions[1] < sep_c,
        "task B exit must precede C start: {lines:?}"
    );
    // 各タスクの stdout マーカは自タスクの Start と Exit の間に出力される。
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
    // 完了セパレータ。
    assert!(lines.iter().any(|l| l.contains("キュー完了")));
}

// ---- 受け入れ基準 2: リアルタイム drain ----

/// 実行中タスク (3 秒生存スタブ) の stdout マーカが、Exit 観測 (プロセス終了行)
/// より前に LogBuffer へ反映される — 実行中のリアルタイム drain 検証。
///
/// 生存時間はマーカ drain 観測 (~100ms) に対する CI 負荷マージン。abort 時の
/// `ChildProcess::stop` は kill 後も孫プロセス (ping/sleep) のパイプ ハンドル
/// 終端まで reader join を待つため、生存時間分だけ後片付けに掛かる点にも注意。
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
            break; // 実行中にマーカが drain された
        }
        assert!(
            Instant::now() < deadline,
            "marker not drained while running: {:?}",
            log_lines(&app)
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    // この時点では Exit 未観測 (= プロセス終了行がまだ無い) ことが保証される。
    assert!(
        !log_lines(&app).iter().any(|l| l.contains("プロセス終了")),
        "exit must not be observed yet: {:?}",
        log_lines(&app)
    );

    // 後片付け: abort で子を kill しキューを終端化する。
    app.abort_task_queue();
    let queue = app.task_queue().unwrap();
    assert!(matches!(queue.state(), QueueState::Completed));
    assert!(queue.is_aborted());
}

// ---- 受け入れ基準 3: 失敗停止 (明示 resume まで後続不起動) ----

/// 非零 exit (exit 2) で PausedAfterFailure に停止し、その後 2 秒間フレーム
/// (drain) を回し続けても状態は PausedAfterFailure のまま・後続タスクは
/// 一切起動しない (自動継続禁止の実プロセス検証)。
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

    // 明示 resume までフレームを回し続けても Noop (後続不起動) のまま。
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

    // 失敗サマリ (exit code 込み) とエラー行。
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

    // 後片付け。
    app.abort_task_queue();
}

// ---- 受け入れ基準 4: 継続完了 ----

/// 失敗停止中の明示 resume で残タスク (後続タスク) が実行され、Completed 到達
/// (進行サマリ「完了 2/2」・セパレータ 2 行・完了行)。
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

// ---- 受け入れ基準 5a: 空キュー開始拒否 ----

/// 空エントリ列での開始は拒否される (キュー未作成・子プロセス不起動・ログ空)。
/// 拒否は致命的でなく、引き続いて正常キューを開始できる。
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

    // 拒否後も正常キューを開始して完走できる。
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

// ---- 受け入れ基準 5b: abort 後の状態 ----

/// abort はキューを終端化 (Completed + is_aborted + エントリ破棄) し、kill された
/// 子の Exit 行が後から drain されても状態は復帰せず残りタスクは起動しない。
#[test]
fn abort_terminates_queue_and_survives_killed_child_exit_drain() {
    let mut app = StudioApp::default();
    app.start_task_entries(vec![
        queue_entry("長時間タスク", marker_spec("abort-marker", 3, 0)),
        queue_entry("後続タスク", marker_spec("post-abort-marker", 0, 0)),
    ]);
    // 実行中 (Running) を確認してから abort。
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

    // kill された子の Exit 行が drain されるまで待つ (状態が崩れないこと)。
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

    // Exit drain 後も: 状態は Completed (aborted) のまま・後続は未起動。
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
