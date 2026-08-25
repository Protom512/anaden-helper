//! 実行ログ/状態表示コア（シャード4, Issue #83）。
//!
//! pipeline を子プロセス（anaden CLI）として起動し、その stdout 行ストリームを
//! 読み取って (a) ログバッファ、(b) 実行状態サマリ（現在ゴール/ループ回数/
//! 停止理由）へ反映する純ロジックと、読み取りスレッドの枠組みを提供する。
//!
//! 設計方針:
//! - 行解析・バッファリング・状態更新はすべて純関数/純構造体（IO 無し）で
//!   単体テスト可能にする。
//! - IO（子プロセス stdout パイプ読み取り）は `spawn_stdout_reader` のみで、
//!   `std::sync::mpsc` で `LogEvent` を UI 側へ非ブロッキング配送する。
//! - UI（egui スクロールログビューア）は app.rs が本モジュールの状態を描画する。

use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::process::{Child, Stdio};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

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
        if line.contains("ERROR") {
            Self::Error
        } else if line.contains("WARN") {
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

/// pipeline の実行状態サマリ。ログ行から漸進的に更新される。
///
/// CLI（anaden-cli main.rs）は決定的な出力を行う:
/// - 開始時: `run_loop 開始: interval=... max_iters=N goal=<名前>`
/// - 終了時: `サイクル数: N` / `停止理由:   <ラベル>`
/// 本構造体はそれらの行を解析して状態を保持する。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RunStatus {
    /// 実行中か（開始行を観測し、停止理由行を未観測）。
    pub running: bool,
    /// 現在のゴール名（開始行の `goal=`。`(none)` は None）。
    pub goal: Option<String>,
    /// ループ回数（`サイクル数: N` 行。実行中は未知 = None）。
    pub iterations: Option<u64>,
    /// 停止理由ラベル（`停止理由:` 行の右辺）。
    pub stop_reason: Option<String>,
}

impl RunStatus {
    /// 未実行（何も観測していない）状態。
    pub fn new() -> Self {
        Self::default()
    }

    /// 1 行を観測して状態を更新する純メソッド。
    ///
    /// 解析対象行（anaden-cli の出力契約）:
    /// - `run_loop 開始: ... goal=X` → running=true, goal=Some(X)
    ///   （`goal=(none)` は goal=None）
    /// - `サイクル数: N` → iterations=Some(N)
    /// - `停止理由:   L`（コロン後の空白は任意） → stop_reason=Some(L),
    ///   running=false
    pub fn observe(&mut self, line: &str) {
        if line.contains("run_loop 開始") {
            self.running = true;
            self.iterations = None;
            self.stop_reason = None;
            self.goal = line.split("goal=").nth(1).map(|rest| {
                let g = rest.trim();
                (g == "(none)").then(|| g.to_string()).filter(|_| false)
                    .unwrap_or_else(|| g.to_string())
            });
        } else if let Some(rest) = line.split_once("サイクル数:") {
            self.iterations = rest.trim().parse::<u64>().ok();
        } else if let Some((_, rest)) = line.split_once("停止理由:") {
            self.stop_reason = Some(rest.trim().to_string());
            self.running = false;
        }
    }

    /// 状態の一行サマリ（UI のステータスバー表示用の純関数）。
    pub fn summary(&self) -> String {
        if self.running {
            format!(
                "実行中 goal={} iterations={}",
                self.goal.as_deref().unwrap_or("(none)"),
                self.iterations.map(|n| n.to_string()).unwrap_or_else(|| "?".into())
            )
        } else if let Some(reason) = &self.stop_reason {
            format!(
                "停止 reason={} iterations={}",
                reason,
                self.iterations.map(|n| n.to_string()).unwrap_or_else(|| "?".into())
            )
        } else {
            "未実行".to_string()
        }
    }
}

/// 固定長ログバッファ + 実行状態トラッカ。UI が毎フレーム `drain` する。
///
/// `SyncSender` は bounded channel（`spawn_stdout_reader` 参照）から来る
/// `LogEvent` を蓄え、上限を超えたら最古行を破棄する。state（RunStatus）は
/// ログ行とは独立に保持し、UI が参照できる。
pub struct LogBuffer {
    entries: VecDeque<LogEntry>,
    max_lines: usize,
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
            status: RunStatus::new(),
        }
    }

    /// ログ 1 行を push（レベル自動推定・状態更新・上限超過時は最古行を破棄）。
    pub fn push_line(&mut self, line: &str) {
        let entry = LogEntry {
            line: line.to_string(),
            level: LogLevel::from_line(line),
        };
        self.entries.push_back(entry);
        while self.entries.len() > self.max_lines {
            self.entries.pop_front();
        }
        self.status.observe(line);
    }

    /// 現在保持している行数。
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 空か。
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
    pub fn drain(&self, rx: &Receiver<LogEvent>) -> Vec<LogEntry> {
        let Ok(mut buf) = self.inner.lock() else {
            return Vec::new();
        };
        while let Ok(ev) = rx.try_recv() {
            match ev {
                LogEvent::Line(l) => buf.push_line(&l),
                LogEvent::Exit(code) => {
                    let label = match code {
                        Some(0) => "exit=0 (成功)",
                        Some(c) => "exit=エラー",
                        None => "exit=不明",
                    };
                    buf.push_line(&format!("[studio] プロセス終了: {label} (code={code:?})"));
                }
            }
        }
        buf.entries().cloned().collect()
    }

    /// 内部 LogBuffer への排他参照（テスト・UI 直接操作用）。
    pub fn with_buf<R>(&self, f: impl FnOnce(&mut LogBuffer) -> R) -> Option<R> {
        self.inner.lock().ok().map(|mut b| f(&mut b))
    }
}

/// 子プロセスの stdout を行単位で読み取り `tx` へ送るスレッドを起動する。
///
/// 読み取りスレッドは行を `SyncSender::try_send` で送る（bounded）。UI 側が
/// 受信を止めてもスレッドがブロックしないよう、`Full/Disconnected` 時は
/// 該当行を破棄して読み取りを継続する（ログは best-effort）。
///
/// 戻り値:
/// - `Ok((Child, JoinHandle))`: 起動成功。`Child` の stdout は本スレッドが
///   消費するため UI 側は wait のみ行うこと。JoinHandle は EOF 後に子の
///   exit code を待ち `LogEvent::Exit` を送って完了する。
/// - `Err(spawn 失敗)`: 子プロセス未起動。
///
/// # Errors
/// `std::process::Command::spawn` の失敗（実行ファイル不在等）をそのまま返す。
pub fn spawn_stdout_reader(
    mut cmd: std::process::Command,
    tx: SyncSender<LogEvent>,
) -> std::io::Result<(Child, JoinHandle<()>)> {
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::null()); // shard-4 では stdout のみ（tracing は stdout 出力）
    let mut child = cmd.spawn()?;
    let stdout = child.stdout.take();
    let reader = std::thread::spawn(move || {
        if let Some(out) = stdout {
            for line in BufReader::new(out).lines() {
                let Ok(line) = line else { break };
                if matches!(tx.try_send(LogEvent::Line(line)), Err(TrySendError::Full(_) | TrySendError::Disconnected(_))) {
                    // UI が受信しなくても読み取りは続行（EOF 検出のため）。
                }
            }
        }
        let code = child.wait().ok().and_then(|s| s.code());
        let _ = tx.try_send(LogEvent::Exit(code));
    });
    Ok((child, reader))
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
        assert_eq!(LogLevel::from_line("ERROR anaden: capture failed"), LogLevel::Error);
        assert_eq!(LogLevel::from_line("2026-01-01 INFO x: has WARN inside"), LogLevel::Warn);
        assert_eq!(LogLevel::from_line("INFO anaden: run_loop 開始"), LogLevel::Info);
        assert_eq!(LogLevel::from_line("=== 実行結果 ==="), LogLevel::Info);
    }

    #[test]
    fn level_prefers_error_over_warn() {
        assert_eq!(LogLevel::from_line("WARN then ERROR"), LogLevel::Error);
    }

    // ---- RunStatus ----

    #[test]
    fn status_start_line_sets_running_and_goal() {
        let mut s = RunStatus::new();
        s.observe("INFO anaden_cli: run_loop 開始: interval=2s max_iters=10 goal=farm50");
        assert!(s.running);
        assert_eq!(s.goal.as_deref(), Some("farm50"));
        assert_eq!(s.iterations, None);
        assert_eq!(s.stop_reason, None);
    }

    #[test]
    fn status_start_line_without_goal_is_none() {
        let mut s = RunStatus::new();
        s.observe("INFO anaden_cli: run_loop 開始: interval=2s max_iters=10 goal=(none)");
        assert!(s.running);
        assert_eq!(s.goal, None);
    }

    #[test]
    fn status_result_lines_set_iterations_and_stop() {
        let mut s = RunStatus::new();
        s.observe("run_loop 開始: interval=2s max_iters=10 goal=g1");
        s.observe("サイクル数: 42");
        s.observe("停止理由:   宣言的ゴール到達(正常)");
        assert!(!s.running);
        assert_eq!(s.iterations, Some(42));
        assert_eq!(s.stop_reason.as_deref(), Some("宣言的ゴール到達(正常)"));
    }

    #[test]
    fn status_summary_varies_by_phase() {
        let mut s = RunStatus::new();
        assert_eq!(s.summary(), "未実行");
        s.observe("run_loop 開始: interval=2s goal=g1");
        assert_eq!(s.summary(), "実行中 goal=g1 iterations=?");
        s.observe("サイクル数: 3");
        s.observe("停止理由: 最大サイクル到達");
        assert_eq!(s.summary(), "停止 reason=最大サイクル到達 iterations=3");
    }

    #[test]
    fn status_restart_resets_previous_result() {
        let mut s = RunStatus::new();
        s.observe("サイクル数: 5");
        s.observe("停止理由: 最大サイクル到達");
        s.observe("run_loop 開始: interval=2s goal=g2");
        assert!(s.running);
        assert_eq!(s.iterations, None);
        assert_eq!(s.stop_reason, None);
        assert_eq!(s.goal.as_deref(), Some("g2"));
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
        tx.send(LogEvent::Line("INFO run_loop 開始: goal=g".into())).unwrap();
        tx.send(LogEvent::Exit(Some(0))).unwrap();
        let snap = shared.drain(&rx);
        assert_eq!(snap.len(), 2);
        let buf_lines = shared.with_buf(|b| b.entries().cloned().collect::<Vec<_>>()).unwrap();
        assert_eq!(buf_lines.len(), 2);
        assert!(buf_lines[1].line.contains("exit=0"));
        assert!(shared.with_buf(|b| b.status.running).unwrap());
        // 空チャネルの再 drain は追記しない。
        assert_eq!(shared.drain(&rx).len(), 0);
    }

    // ---- spawn_stdout_reader (echo プロセスで統合確認) ----

    #[test]
    fn stdout_reader_streams_lines_and_exit() {
        // Windows は cmd /c、それ以外は sh。どちらも2行出力して終了する。
        let mut cmd = if cfg!(windows) {
            let mut c = std::process::Command::new("cmd");
            c.args(["/C", "echo one & echo two"]);
            c
        } else {
            let mut c = std::process::Command::new("sh");
            c.args(["-c", "echo one; echo two"]);
            c
        };
        let (tx, rx) = std::sync::mpsc::sync_channel::<LogEvent>(256);
        let (_child, handle) = spawn_stdout_reader(&mut cmd, tx).expect("spawn failed");
        handle.join().expect("reader thread panicked");
        let mut events: Vec<LogEvent> = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            events.push(ev);
        }
        let lines: Vec<&str> = events
            .iter()
            .filter_map(|e| match e {
                LogEvent::Line(l) => Some(l.as_str()),
                _ => None,
            })
            .map(str::trim)
            .collect();
        assert!(lines.contains(&"one"), "lines={lines:?}");
        assert!(lines.contains(&"two"), "lines={lines:?}");
        assert!(matches!(events.last(), Some(LogEvent::Exit(Some(0)))), "events={events:?}");
    }
}
