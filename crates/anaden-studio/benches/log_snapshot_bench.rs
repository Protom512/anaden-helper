use std::hint::black_box;
use std::sync::mpsc::sync_channel;

use anaden_studio::log_view::{
    DEFAULT_MAX_LINES, LogEntry, LogEvent, SharedLogBuffer, drain_channel_into,
};
use criterion::{BatchSize, Criterion, criterion_group, criterion_main};

fn sample_line(seq: usize) -> String {
    format!(
        "INFO anaden_engine: cycle={seq:05} task=TapBottomStable match=btn_start \
score=0.987123 pos=(1234,567) elapsed_ms=42.195 threshold=0.85"
    )
}

fn filled_log(n_lines: usize) -> SharedLogBuffer {
    let log = SharedLogBuffer::new(DEFAULT_MAX_LINES);
    for seq in 0..n_lines {
        let _ = log.with_buf(|b| b.push_line(&sample_line(seq)));
    }
    log
}

fn legacy_refresh_snapshot(log: &SharedLogBuffer) -> Vec<LogEntry> {
    log.with_buf(|b| b.entries().cloned().collect())
        .unwrap_or_default()
}

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

    fn refresh(&mut self, log: &SharedLogBuffer) {
        if let Some((rev, entries)) = log.changed_entries(self.revision) {
            self.revision = rev;
            self.entries = entries;
        }
    }
}

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

fn bench_frame_idle_after_gated(c: &mut Criterion) {
    let log = filled_log(DEFAULT_MAX_LINES);
    let (_tx, rx) = sync_channel::<LogEvent>(1024);
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
