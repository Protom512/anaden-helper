use std::collections::VecDeque;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

use crate::log_status::RunStatus;

pub const DEFAULT_MAX_LINES: usize = 5000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

impl LogLevel {
    pub fn from_line(line: &str) -> Self {
        if line.contains("ERROR") || line.contains("panicked") || line.contains("PANIC") {
            Self::Error
        } else if line.contains("WARN") || line.starts_with("[stderr]") {
            Self::Warn
        } else {
            Self::Info
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEntry {
    pub line: String,
    pub level: LogLevel,
}

pub struct LogBuffer {
    entries: VecDeque<LogEntry>,
    max_lines: usize,
    revision: u64,
    pub status: RunStatus,
}

impl Default for LogBuffer {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_LINES)
    }
}

impl LogBuffer {
    pub fn new(max_lines: usize) -> Self {
        Self {
            entries: VecDeque::with_capacity(max_lines.min(1024)),
            max_lines: max_lines.max(1),
            revision: 0,
            status: RunStatus::new(),
        }
    }

    pub fn push_line(&mut self, line: &str) {
        let level = LogLevel::from_line(line);
        self.push_entry(line, level);
    }

    fn push_entry(&mut self, line: &str, level: LogLevel) {
        let entry = LogEntry {
            line: line.to_string(),
            level,
        };
        self.entries.push_back(entry);
        while self.entries.len() > self.max_lines {
            self.entries.pop_front();
        }
        self.status.observe(line);
        self.revision = self.revision.wrapping_add(1);
    }

    pub fn push_line_with_level(&mut self, line: &str, level: LogLevel) {
        self.push_entry(line, level);
    }

    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision
    }

    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn entries(&self) -> impl Iterator<Item = &LogEntry> {
        self.entries.iter()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.status = RunStatus::new();
        self.revision = self.revision.wrapping_add(1);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogEvent {
    Line(String),
    Exit(Option<i32>),
}

#[derive(Clone)]
pub struct SharedLogBuffer {
    inner: Arc<Mutex<LogBuffer>>,
}

impl SharedLogBuffer {
    pub fn new(max_lines: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(LogBuffer::new(max_lines))),
        }
    }

    #[cfg(test)]
    pub fn drain(&self, rx: &Receiver<LogEvent>) -> Vec<LogEntry> {
        let Ok(mut buf) = self.inner.lock() else {
            return Vec::new();
        };
        while let Ok(ev) = rx.try_recv() {
            match ev {
                LogEvent::Line(l) => buf.push_line(&l),
                LogEvent::Exit(code) => {
                    let (label, level) = match code {
                        Some(0) => ("exit=0 (成功)", LogLevel::Info),
                        Some(_) => ("exit=エラー", LogLevel::Error),
                        None => ("exit=不明", LogLevel::Error),
                    };
                    buf.push_line_with_level(
                        &format!("[studio] プロセス終了: {label} (code={code:?})"),
                        level,
                    );
                }
            }
        }
        buf.entries().cloned().collect()
    }

    pub fn with_buf<R>(&self, f: impl FnOnce(&mut LogBuffer) -> R) -> Option<R> {
        self.inner.lock().ok().map(|mut b| f(&mut b))
    }

    #[must_use]
    pub fn changed_entries(&self, cached_revision: u64) -> Option<(u64, Vec<LogEntry>)> {
        let buf = self.inner.lock().ok()?;
        if buf.revision == cached_revision {
            return None;
        }
        Some((buf.revision, buf.entries().cloned().collect()))
    }
}

pub fn drain_channel_into(
    log: &SharedLogBuffer,
    rx: &Receiver<LogEvent>,
) -> (usize, Option<Option<i32>>) {
    let mut new_lines = 0usize;
    let mut exit_code: Option<Option<i32>> = None;
    let Ok(mut buf) = log.inner.lock() else {
        return (0, None);
    };
    while let Ok(ev) = rx.try_recv() {
        match ev {
            LogEvent::Line(l) => {
                buf.push_line(&l);
                new_lines += 1;
            }
            LogEvent::Exit(code) => {
                let (label, level) = match code {
                    Some(0) => ("exit=0 (成功)", LogLevel::Info),
                    Some(_) => ("exit=エラー", LogLevel::Error),
                    None => ("exit=不明", LogLevel::Error),
                };
                buf.push_line_with_level(
                    &format!("[studio] プロセス終了: {label} (code={code:?})"),
                    level,
                );
                new_lines += 1;
                if exit_code.is_none() {
                    exit_code = Some(code);
                }
            }
        }
    }
    (new_lines, exit_code)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn level_detects_error_and_warn_anywhere_in_line() {
        assert_eq!(
            LogLevel::from_line("ERROR anaden: capture failed"),
            LogLevel::Error
        );
        assert_eq!(
            LogLevel::from_line("2026-01-01 INFO x: has WARN inside"),
            LogLevel::Warn
        );
        assert_eq!(
            LogLevel::from_line("INFO anaden: run_loop 開始"),
            LogLevel::Info
        );
        assert_eq!(LogLevel::from_line("=== 実行結果 ==="), LogLevel::Info);
    }

    #[test]
    fn level_prefers_error_over_warn() {
        assert_eq!(LogLevel::from_line("WARN then ERROR"), LogLevel::Error);
    }

    #[test]
    fn buffer_evicts_oldest_beyond_max() {
        let mut b = LogBuffer::new(3);
        for i in 0..5 {
            b.push_line(&format!("line-{i}"));
        }
        assert_eq!(b.len(), 3);
        let lines: Vec<&str> = b.entries().map(|e| e.line.as_str()).collect();
        assert_eq!(lines, vec!["line-2", "line-3", "line-4"]);
    }

    #[test]
    fn buffer_status_tracks_lines() {
        let mut b = LogBuffer::default();
        b.push_line("run_loop 開始: goal=g");
        assert!(b.status.running);
        b.clear();
        assert!(b.is_empty());
        assert_eq!(b.status.summary(), "未実行");
    }

    #[test]
    fn shared_buffer_drains_channel_and_appends_exit_line() {
        let (tx, rx) = std::sync::mpsc::sync_channel(16);
        let shared = SharedLogBuffer::new(100);
        tx.send(LogEvent::Line("INFO run_loop 開始: goal=g".into()))
            .unwrap();
        tx.send(LogEvent::Exit(Some(0))).unwrap();
        let snap = shared.drain(&rx);
        assert_eq!(snap.len(), 2);
        let buf_lines = shared
            .with_buf(|b| b.entries().cloned().collect::<Vec<_>>())
            .unwrap();
        assert_eq!(buf_lines.len(), 2);
        assert!(buf_lines[1].line.contains("exit=0"));
        assert!(shared.with_buf(|b| b.status.running).unwrap());
        assert_eq!(shared.drain(&rx).len(), 2);
    }

    #[test]
    fn changed_entries_returns_none_when_buffer_unchanged() {
        let shared = SharedLogBuffer::new(100);
        shared.with_buf(|b| b.push_line("line-1")).unwrap();
        let (rev, snap) = shared.changed_entries(0).unwrap();
        assert_eq!(snap.len(), 1);
        assert_eq!(rev, shared.with_buf(|b| b.revision()).unwrap());
        assert!(shared.changed_entries(rev).is_none());
    }

    #[test]
    fn changed_entries_returns_full_entries_after_push() {
        let shared = SharedLogBuffer::new(100);
        shared.with_buf(|b| b.push_line("line-1")).unwrap();
        let (rev1, _) = shared.changed_entries(0).unwrap();
        shared.with_buf(|b| b.push_line("line-2")).unwrap();
        let (rev2, snap) = shared.changed_entries(rev1).unwrap();
        assert_ne!(rev1, rev2);
        let lines: Vec<&str> = snap.iter().map(|e| e.line.as_str()).collect();
        assert_eq!(lines, vec!["line-1", "line-2"]);
    }

    #[test]
    fn changed_entries_detects_clear_as_change_to_empty() {
        let shared = SharedLogBuffer::new(100);
        shared.with_buf(|b| b.push_line("line-1")).unwrap();
        let (rev1, _) = shared.changed_entries(0).unwrap();
        shared.with_buf(LogBuffer::clear);
        let (rev2, snap) = shared.changed_entries(rev1).unwrap();
        assert_ne!(rev1, rev2);
        assert!(snap.is_empty());
        assert!(shared.changed_entries(rev2).is_none());
    }

    #[test]
    fn changed_entries_reflects_eviction_at_capacity() {
        let shared = SharedLogBuffer::new(3);
        for i in 0..3 {
            shared.with_buf(|b| b.push_line(&format!("l{i}"))).unwrap();
        }
        let (rev1, snap1) = shared.changed_entries(0).unwrap();
        assert_eq!(snap1.len(), 3);
        shared.with_buf(|b| b.push_line("l3")).unwrap();
        let (_rev2, snap2) = shared.changed_entries(rev1).unwrap();
        let lines: Vec<&str> = snap2.iter().map(|e| e.line.as_str()).collect();
        assert_eq!(lines, vec!["l1", "l2", "l3"]);
    }

    #[test]
    fn drain_channel_into_batches_lines_and_first_exit_wins() {
        let (tx, rx) = std::sync::mpsc::sync_channel(16);
        let shared = SharedLogBuffer::new(100);
        tx.send(LogEvent::Line("INFO run_loop 開始: goal=g".into()))
            .unwrap();
        tx.send(LogEvent::Exit(Some(0))).unwrap();
        tx.send(LogEvent::Exit(Some(2))).unwrap();
        tx.send(LogEvent::Line("tail".into())).unwrap();
        let (new_lines, exit) = drain_channel_into(&shared, &rx);
        assert_eq!(new_lines, 4);
        assert_eq!(exit, Some(Some(0)));
        let lines = shared
            .with_buf(|b| b.entries().cloned().collect::<Vec<_>>())
            .unwrap();
        assert_eq!(lines.len(), 4);
        assert!(lines.iter().any(|e| e.line.contains("exit=0")));
        assert!(lines.iter().any(|e| e.line == "tail"));
    }

    #[test]
    fn level_stderr_prefix_lines_are_warn() {
        assert_eq!(
            LogLevel::from_line("[stderr] some diagnostics"),
            LogLevel::Warn
        );
    }

    #[test]
    fn level_panic_lines_are_error() {
        assert_eq!(
            LogLevel::from_line("thread 'main' panicked at src/main.rs:2:3:"),
            LogLevel::Error
        );
        assert_eq!(LogLevel::from_line("PANIC in pipeline"), LogLevel::Error);
    }

    #[test]
    fn level_error_still_wins_over_stderr_warn() {
        assert_eq!(
            LogLevel::from_line("[stderr] ERROR something failed"),
            LogLevel::Error
        );
    }

    #[test]
    fn level_plain_lines_remain_info() {
        assert_eq!(LogLevel::from_line("=== 実行結果 ==="), LogLevel::Info);
        assert_eq!(LogLevel::from_line("サイクル数: 3"), LogLevel::Info);
    }
}
