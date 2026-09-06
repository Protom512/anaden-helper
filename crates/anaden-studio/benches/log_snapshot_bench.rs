//! Issue #160 UC-5 (Shard 6): 毎フレームログスナップショット経路のベンチマーク。
//!
//! 対象は GUI の毎フレーム実行される純ロジック — `StudioApp::drain_task_logs` +
//! `refresh_task_log_snapshot` (app.rs) / `PipelineRunnerApp::drain_logs` +
//! `refresh_snapshot` (runner.rs) と同一の操作列。GUI フレーム全体 (レイアウト・
//! ラスタライズ) はベンチに載らないため、その中でログバッファに触る部分だけを
//! 抽出している:
//!
//! - `frame_idle/...`   : 新着行 0 のフレーム (実行中キューの大多数のフレーム)
//! - `frame_burst10/...`: 1 フレームに 10 行到着したフレーム
//!
//! `before_*` ベンチは最適化前の production パターンの再現:
//! - drain は行ごとに `with_buf` でロックを取得する
//!   (`log_view::drain_channel_into` の旧実装と同一)。
//! - refresh はバッファ全行 (上限 `DEFAULT_MAX_LINES` = 5000 行) を
//!   `String` clone する (`refresh_task_log_snapshot` / `refresh_snapshot`
//!   の旧実装と同一)。
//!
//! `after_gated` ベンチは最適化後の production パターン (Issue #160 UC-5):
//! - drain はロック 1 回で全イベントを反映 (`drain_channel_into` 現行実装)。
//! - refresh は [`SharedLogBuffer::changed_entries`] による差分更新
//!   (改訂番号が同じ間 = 新着行なしのフレームは clone なし)。
//!
//! 計測方法・before/after 生値は `docs/perf-issue-160.md` を参照。

use std::hint::black_box;
use std::sync::mpsc::sync_channel;

use anaden_studio::log_view::{
    DEFAULT_MAX_LINES, LogEntry, LogEvent, SharedLogBuffer, drain_channel_into,
};
use criterion::{BatchSize, Criterion, criterion_group, criterion_main};

/// 実運用の anaden ログ行に近い長さ (~90 文字) の行を生成する。
fn sample_line(seq: usize) -> String {
    format!(
        "INFO anaden_engine: cycle={seq:05} task=TapBottomStable match=btn_start \
score=0.987123 pos=(1234,567) elapsed_ms=42.195 threshold=0.85"
    )
}

/// `n_lines` 行を充填済みの `SharedLogBuffer` (上限は production と同じ
/// `DEFAULT_MAX_LINES`) を構築する。`n_lines` が上限を超える分は最古行から
/// 破棄されるため、定常状態では常に上限いっぱいになる。
fn filled_log(n_lines: usize) -> SharedLogBuffer {
    let log = SharedLogBuffer::new(DEFAULT_MAX_LINES);
    for seq in 0..n_lines {
        let _ = log.with_buf(|b| b.push_line(&sample_line(seq)));
    }
    log
}

/// 最適化前の production refresh (app.rs `refresh_task_log_snapshot` /
/// runner.rs `refresh_snapshot` の旧実装と同一): バッファ全行を clone する。
fn legacy_refresh_snapshot(log: &SharedLogBuffer) -> Vec<LogEntry> {
    log.with_buf(|b| b.entries().cloned().collect())
        .unwrap_or_default()
}

/// 最適化後の production refresh (app.rs / runner.rs 現行実装と同一):
/// 改訂番号キャッシュによる差分更新。app/runner の保持フィールド
/// (`task_log_revision` / `log_revision`) と同じ役割のローカル変数を
/// Bencher クロージャ外のキャッシュとして持つ。
struct GatedSnapshot {
    revision: u64,
    entries: Vec<LogEntry>,
}

impl GatedSnapshot {
    fn new() -> Self {
        Self {
            revision: 0,
            entries: Vec::new(),
        }
    }

    /// app.rs `refresh_task_log_snapshot` / runner.rs `refresh_snapshot`
    /// の現行実装と同一の差分更新。
    fn refresh(&mut self, log: &SharedLogBuffer) {
        if let Some((rev, entries)) = log.changed_entries(self.revision) {
            self.revision = rev;
            self.entries = entries;
        }
    }
}

/// 新着行 0 のフレーム 1 回分 (drain + refresh) — 最適化前パターン。
fn bench_frame_idle_before(c: &mut Criterion) {
    let log = filled_log(DEFAULT_MAX_LINES);
    let (_tx, rx) = sync_channel::<LogEvent>(1024);
    c.bench_function("frame_idle/before_full_clone", |b| {
        b.iter(|| {
            let _ = drain_channel_into(black_box(&log), black_box(&rx));
            black_box(legacy_refresh_snapshot(black_box(&log)));
        });
    });
}

/// 1 フレームに 10 行到着したフレーム 1 回分 (drain + refresh) — 最適化前パターン。
///
/// setup (計測外): 空でない channel に 10 行積む。timed routine 内の drain が
/// バッファへ 10 行 push する (production と同じ: push は drain 中に起こる)。
fn bench_frame_burst10_before(c: &mut Criterion) {
    let log = filled_log(DEFAULT_MAX_LINES);
    c.bench_function("frame_burst10/before_full_clone", |b| {
        b.iter_batched(
            || {
                let (tx, rx) = sync_channel::<LogEvent>(1024);
                for i in 0..10 {
                    let _ = tx.try_send(LogEvent::Line(sample_line(900_000 + i)));
                }
                rx
            },
            |rx| {
                let _ = drain_channel_into(&log, &rx);
                black_box(legacy_refresh_snapshot(&log));
            },
            BatchSize::SmallInput,
        );
    });
}

/// 新着行 0 のフレーム 1 回分 (drain + refresh) — 最適化後パターン
/// (drain 単一ロック + revision-gated refresh)。
fn bench_frame_idle_after_gated(c: &mut Criterion) {
    let log = filled_log(DEFAULT_MAX_LINES);
    let (_tx, rx) = sync_channel::<LogEvent>(1024);
    // 初回 refresh でスナップショットを同期済みにする (定常フレームの計測)。
    let mut snap = GatedSnapshot::new();
    snap.refresh(&log);
    c.bench_function("frame_idle/after_gated", |b| {
        b.iter(|| {
            let _ = drain_channel_into(black_box(&log), black_box(&rx));
            snap.refresh(black_box(&log));
            black_box(&snap.entries);
        });
    });
}

/// 1 フレームに 10 行到着したフレーム 1 回分 (drain + refresh) —
/// 最適化後パターン (変更ありフレームは 1 回の全行再構築を払う)。
fn bench_frame_burst10_after_gated(c: &mut Criterion) {
    let log = filled_log(DEFAULT_MAX_LINES);
    let mut snap = GatedSnapshot::new();
    snap.refresh(&log);
    c.bench_function("frame_burst10/after_gated", |b| {
        b.iter_batched(
            || {
                let (tx, rx) = sync_channel::<LogEvent>(1024);
                for i in 0..10 {
                    let _ = tx.try_send(LogEvent::Line(sample_line(900_000 + i)));
                }
                rx
            },
            |rx| {
                let _ = drain_channel_into(&log, &rx);
                snap.refresh(&log);
                black_box(&snap.entries);
            },
            BatchSize::SmallInput,
        )
    });
}

criterion_group!(
    benches,
    bench_frame_idle_before,
    bench_frame_idle_after_gated,
    bench_frame_burst10_before,
    bench_frame_burst10_after_gated,
);
criterion_main!(benches);
