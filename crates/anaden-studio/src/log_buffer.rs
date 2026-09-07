//! 固定長ログバッファ + 改訂番号ベース差分スナップショット + チャネル drain
//! (Issue #172: log_view.rs 分割で旧モジュールから移動)。
//!
//! 設計方針 (旧 log_view.rs から継承):
//! - 行解析・バッファリング・状態更新はすべて純関数/純構造体（IO 無し）で
//!   単体テスト可能。UI（egui スクロールログビューア）は app.rs が描画する。
//! - [`SharedLogBuffer::changed_entries`] は改訂番号比較で「新着行のない
//!   フレームの全行 clone」を回避する (Issue #160 UC-5)。
//! - [`drain_channel_into`] は 1 フレーム分のイベント列をロック 1 回で
//!   反映する共有ヘルパ (Issue #154 Shard 1)。

use std::collections::VecDeque;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

use crate::log_status::RunStatus;

/// ログバッファの既定上限行数。超過分は先頭から破棄（リングバッファ相当）。
pub const DEFAULT_MAX_LINES: usize = 5000;

/// ログ行の重要度。CLI の tracing 出力（INFO/WARN/ERROR）と非 tracing 行に対応。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    /// 通常行（tracing INFO 相当・レベル接頭辞なし行を含む）。
    Info,
    /// 警告行（`WARN` を含む行）。
    Warn,
    /// エラー行（`ERROR` を含む行）。
    Error,
}

impl LogLevel {
    /// 行テキストから重要度を推定する純関数。
    ///
    /// tracing の既定フォーマットは行頭にレベル（例: `INFO anaden_engine: ...`）
    /// を出すが、`RUST_LOG` 无し運用や println! 直接出力（CLI の `=== 実行結果 ===`
    /// 等）もあるため、**行内のどこかに大文字トークンがあれば**そのレベルとみなす。
    /// 複数ヒット時は ERROR > WARN > INFO の優先度。
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

/// バッファ済みログ 1 行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEntry {
    /// 行テキスト（改行なし）。
    pub line: String,
    /// 重要度。
    pub level: LogLevel,
}

/// 固定長ログバッファ + 実行状態トラッカ。UI が毎フレーム `drain` する。
///
/// `SyncSender` は bounded channel（`spawn_stdout_reader` 参照）から来る
/// `LogEvent` を蓄え、上限を超えたら最古行を破棄する。state（RunStatus）は
/// ログ行とは独立に保持し、UI が参照できる。
pub struct LogBuffer {
    entries: VecDeque<LogEntry>,
    max_lines: usize,
    /// バッファ内容の改訂番号 (push/clear 時に増分・単調増加)。
    ///
    /// UI 側スナップショットの差分更新 ([`SharedLogBuffer::changed_entries`])
    /// で「新着行のないフレームの全行 clone」を回避するためのカウンタ
    /// (Issue #160 UC-5)。
    revision: u64,
    /// 実行状態サマリ（ログ行から漸進更新）。
    pub status: RunStatus,
}

impl Default for LogBuffer {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_LINES)
    }
}

impl LogBuffer {
    /// 上限 `max_lines` 行のバッファを構築する。
    pub fn new(max_lines: usize) -> Self {
        Self {
            entries: VecDeque::with_capacity(max_lines.min(1024)),
            max_lines: max_lines.max(1),
            revision: 0,
            status: RunStatus::new(),
        }
    }

    /// ログ 1 行を push（レベル自動推定・状態更新・上限超過時は最古行を破棄）。
    pub fn push_line(&mut self, line: &str) {
        let level = LogLevel::from_line(line);
        self.push_entry(line, level);
    }

    /// ログ 1 行を明示レベルで push（終了通知等、文字列から推定できない行用）。
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

    /// 明示レベル指定の push（終了/システム通知行。公開ヘッドレステスト用）。
    pub fn push_line_with_level(&mut self, line: &str, level: LogLevel) {
        self.push_entry(line, level);
    }

    /// 現在の改訂番号 (スナップショット差分更新用・[`SharedLogBuffer::changed_entries`] 参照)。
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// 現在保持している行数。
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 空か。
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 保持行への参照（UI 描画用・昇順）。
    pub fn entries(&self) -> impl Iterator<Item = &LogEntry> {
        self.entries.iter()
    }

    /// バッファと状態をクリアする（次実行に備える）。
    pub fn clear(&mut self) {
        self.entries.clear();
        self.status = RunStatus::new();
        self.revision = self.revision.wrapping_add(1);
    }
}

/// UI スレッドへ送るイベント。
///
/// `Exit` は子プロセスの終了（stdout EOF + wait 完了）を通知する。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogEvent {
    /// stdout の 1 行（改行除去済み）。
    Line(String),
    /// 子プロセス終了。exit code（wait 成功時）。
    Exit(Option<i32>),
}

/// 読み取りスレッドから LogBuffer への排他ハンドル。
///
/// UI は毎フレーム `lock` して新着行をバッファへ反映する。 poisoning は
/// 読み取りスレッド内で unwrap しない限り起こらないため、`PoisonError` は
/// 内部状態をそのまま復帰させる（ログは best-effort 表示でよい）。
#[derive(Clone)]
pub struct SharedLogBuffer {
    inner: Arc<Mutex<LogBuffer>>,
}

impl SharedLogBuffer {
    /// 新規作成。
    pub fn new(max_lines: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(LogBuffer::new(max_lines))),
        }
    }

    /// 新着 LogEvent を非ブロッキングで drain してバッファへ反映し、
    /// バッファのスナップショット（全行クローン）を返す。
    ///
    /// 戻り値は UI 描画用。ロック中毒時は空スナップショットを返す
    /// （ログ表示は best-effort で、UI を落とさない）。
    /// テストからのみ使用（runner は Exit イベント観測のため drain をインライン化）。
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

    /// 内部 LogBuffer への排他参照（テスト・UI 直接操作用）。
    pub fn with_buf<R>(&self, f: impl FnOnce(&mut LogBuffer) -> R) -> Option<R> {
        self.inner.lock().ok().map(|mut b| f(&mut b))
    }

    /// 前回取得時 (`cached_revision`) からバッファが変化している場合のみ、
    /// 全行を複製したスナップショットを `(新改訂番号, 行列)` で返す
    /// (Issue #160 UC-5)。
    ///
    /// 従来 UI は毎フレーム `with_buf(|b| b.entries().cloned().collect())`
    /// （上限 [`DEFAULT_MAX_LINES`] = 5000 行の全 `String` clone）を払って
    /// いた。本メソッドは改訂番号が同じ間（新着行なしのフレーム）は
    /// [`None`] を返し、ロック 1 回 + 整数比較の O(1) で完了する。
    /// [`Some`] が返った場合は呼び出し側で改訂番号をキャッシュし、次回の
    /// `cached_revision` へ渡すこと。
    ///
    /// ロック中毒時は [`None`]（旧スナップショットを維持・best-effort）。
    #[must_use]
    pub fn changed_entries(&self, cached_revision: u64) -> Option<(u64, Vec<LogEntry>)> {
        let buf = self.inner.lock().ok()?;
        if buf.revision == cached_revision {
            return None;
        }
        Some((buf.revision, buf.entries().cloned().collect()))
    }
}

/// チャネルを drain してバッファへ反映し、観測した Exit code を返す
/// 共有ヘルパ (Issue #154 Shard 1: runner.rs `drain_logs` / app.rs
/// `drain_task_logs` の単一実装)。
///
/// - `LogEvent::Line` は [`LogBuffer::push_line`] で記録 (レベル自動推定)。
/// - `LogEvent::Exit` は `[studio] プロセス終了: ...` 行として記録し、
///   最初の Exit の exit code を戻り値の第 2 要素へ返す (Exit 無しは None)。
///
/// 戻り値の第 1 要素は今回記録した行数 (自動スクロール追従の新着行数用)。
///
/// Issue #160 UC-5: 1 フレーム分のイベント列を **ロック 1 回** で反映する
/// (旧実装は行ごとに `with_buf` で lock/unlock していた)。バッファの
/// ロック取得者は UI スレッド (drain / push / snapshot) のみで reader
/// スレッドはチャネルへ送るだけのため、ドレイン中の保持で競合しない。
/// ロック中毒時は何も反映せず `(0, None)` を返す (旧実装も行を破棄して
/// いたのと同じ best-effort)。
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

    // ---- LogLevel ----

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

    // ---- LogBuffer ----

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

    // ---- SharedLogBuffer drain ----

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
        // 空チャネルの再 drain は追記しない（drain はバッファ全体を返すため
        // 行数は増えないことを検証する）。
        assert_eq!(shared.drain(&rx).len(), 2);
    }

    // ---- revision / changed_entries / drain_channel_into (Issue #160 UC-5) ----

    /// 変化なしのフレームでは changed_entries は None を返す
    /// (全行 clone が発生しないことの契約)。
    #[test]
    fn changed_entries_returns_none_when_buffer_unchanged() {
        let shared = SharedLogBuffer::new(100);
        shared.with_buf(|b| b.push_line("line-1")).unwrap();
        let (rev, snap) = shared.changed_entries(0).unwrap();
        assert_eq!(snap.len(), 1);
        assert_eq!(rev, shared.with_buf(|b| b.revision()).unwrap());
        assert!(shared.changed_entries(rev).is_none());
    }

    /// push 後は改訂番号が進み、全行 (既存 + 新規) のスナップショットを返す。
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

    /// clear も改訂番号を進める (クリア後の stale スナップショット残留防止)。
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

    /// 上限到達後の push (最古行破棄) も改訂番号を進め、破棄込みの内容を返す。
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

    /// drain_channel_into (単一ロック版): 行数カウント・Exit 行記録・
    /// 最初の Exit のみ観測という契約は旧実装と同一。
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
        // 行 2 + Exit 行 2 が記録され、観測 Exit は最初の 1 件のみ。
        assert_eq!(new_lines, 4);
        assert_eq!(exit, Some(Some(0)));
        let lines = shared
            .with_buf(|b| b.entries().cloned().collect::<Vec<_>>())
            .unwrap();
        assert_eq!(lines.len(), 4);
        assert!(lines.iter().any(|e| e.line.contains("exit=0")));
        assert!(lines.iter().any(|e| e.line == "tail"));
    }

    // ---- LogLevel 強化 (T4: [stderr] 接頭辞・panic 系行の検出) ----

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
