//! pipeline 実行履歴 (run history) のコア実装 (Issue #83 シャード: 履歴)。
//!
//! egui 非依存の純ロジック:
//! - [`RunRecord`]: 1回の pipeline 実行の記録
//!   (開始時刻・戦略・終了状態・exit code・ログ末尾スナップショット)
//! - [`RunHistory`]: JSONL ファイルへの永続化と最大件数制限
//!
//! 保存先はアプリデータディレクトリ配下 (`history_path()` で解決)。
//! CWD 相対に依存せず、環境変数 `ANADEN_STUDIO_HISTORY` で明示上書き可能。

use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// 履歴ファイル名 (app data dir 配下)。
const HISTORY_FILE_NAME: &str = "history.jsonl";
/// 履歴ファイルの明示上書き用環境変数。
const HISTORY_ENV_VAR: &str = "ANADEN_STUDIO_HISTORY";
/// 既定の最大記録件数。
pub const DEFAULT_MAX_RECORDS: usize = 50;
/// 1レコードあたりのログ末尾スナップショット行数の上限。
pub const LOG_TAIL_MAX_LINES: usize = 20;

/// 実行の終了状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunOutcome {
    /// exit code 0 で正常終了。
    Success,
    /// exit code 非零で異常終了。
    Failed,
    /// ユーザー操作による中止。
    Cancelled,
}

/// 1回の pipeline 実行の記録。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunRecord {
    /// 実行開始時刻 (UNIX epoch 秒)。
    pub started_at_unix: u64,
    /// 実行した戦略名 (例: "fishing")。
    pub strategy: String,
    /// 終了状態。
    pub outcome: RunOutcome,
    /// 子プロセスの exit code (中止等で採取不能な場合は None)。
    pub exit_code: Option<i32>,
    /// ログ末尾スナップショット (LOG_TAIL_MAX_LINES 行まで)。
    pub log_tail: Vec<String>,
}

impl RunRecord {
    /// 新規記録を生成する。`log_tail` は上限行数に切り詰められる。
    pub fn new(
        started_at_unix: u64,
        strategy: impl Into<String>,
        outcome: RunOutcome,
        exit_code: Option<i32>,
        log_tail: Vec<String>,
    ) -> Self {
        Self {
            started_at_unix,
            strategy: strategy.into(),
            outcome,
            exit_code,
            log_tail: truncate_tail(log_tail),
        }
    }
}

/// 末尾側を LOG_TAIL_MAX_LINES 行に切り詰める。
fn truncate_tail(mut lines: Vec<String>) -> Vec<String> {
    if lines.len() > LOG_TAIL_MAX_LINES {
        let start = lines.len() - LOG_TAIL_MAX_LINES;
        lines.drain(..start);
    }
    lines
}

/// JSONL ファイル-backed の実行履歴ストア。
///
/// 追記のたびに全レコードを保持した JSONL を書き直す
/// (GUI ユースケースの件数 (≤ 数十) で十分な簡素化)。
#[derive(Debug)]
pub struct RunHistory {
    /// JSONL ファイルパス。
    path: PathBuf,
    /// 最大記録件数 (超過分は古い方から破棄)。
    max_records: usize,
    /// メモリ上のレコード (新しい順 = index 0 が最新)。
    records: Vec<RunRecord>,
}

impl RunHistory {
    /// 指定パス・最大件数で生成する。既存ファイルがあれば読み込む
    /// (破損行は読み飛ばし、読めなければ空履歴から開始する)。
    pub fn open(path: impl Into<PathBuf>, max_records: usize) -> Self {
        let path = path.into();
        let mut records = Self::read_file(&path);
        truncate_oldest(&mut records, max_records);
        Self {
            path,
            max_records,
            records,
        }
    }

    /// 既定パス (`history_path()` 解決) で生成する。
    pub fn open_default() -> Self {
        Self::open(history_path(), DEFAULT_MAX_RECORDS)
    }

    /// 履歴ファイルパスを解決する (純ロジック・テスト容易)。
    ///
    /// 優先順位:
    /// 1. 環境変数 `ANADEN_STUDIO_HISTORY` (ファイルパスを明示指定)
    /// 2. `{config_dir}/anaden-studio/history.jsonl`
    ///    - `XDG_CONFIG_HOME` / `$HOME/.config` (Unix 系のフォールバック)
    ///    - `{FOLDERID_RoamingAppData}` (`APPDATA`, Windows)
    /// 3. いずれも解決不能な場合はカレントディレクトリ直下 (最終フォールバック)
    pub fn history_path() -> PathBuf {
        history_path()
    }

    /// 記録を追加して永続化する。最大件数を超えた古い記録は破棄される。
    ///
    /// # Errors
    /// ファイル書き込みに失敗した場合 (`io::Error`)。メモリ上のレコードには
    /// 反映済みのため、リトライで復旧できる。
    pub fn append(&mut self, record: RunRecord) -> io::Result<()> {
        self.records.insert(0, record);
        truncate_oldest(&mut self.records, self.max_records);
        self.persist()
    }

    /// 記録一覧 (新しい順)。
    pub fn records(&self) -> &[RunRecord] {
        &self.records
    }

    /// 履歴ファイルパス。
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// JSONL 全行を書き込む (新しい順で書き出す)。
    fn persist(&self) -> io::Result<()> {
        if let Some(parent) = self.path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }

        let mut buf = String::new();
        for r in &self.records {
            match serde_json::to_string(r) {
                Ok(line) => {
                    buf.push_str(&line);
                    buf.push('\n');
                }
                // RunRecord は serialize 失敗しえないため、失敗時はその行をスキップ。
                Err(_) => continue,
            }
        }
        std::fs::write(&self.path, buf)
    }

    /// JSONL ファイルからレコードを読み込む (新しい順で保存されている前提)。
    /// 破損行・読み取り失敗時は空ベクタを返す (fail-soft)。
    fn read_file(path: &std::path::Path) -> Vec<RunRecord> {
        let Ok(content) = std::fs::read_to_string(path) else {
            return Vec::new();
        };
        content
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }
}

/// 履歴ファイルパスを解決する。
pub fn history_path() -> PathBuf {
    if let Ok(explicit) = std::env::var(HISTORY_ENV_VAR)
        && !explicit.trim().is_empty()
    {
        return PathBuf::from(explicit);
    }
    config_dir().join("anaden-studio").join(HISTORY_FILE_NAME)
}

/// プラットフォーム別の config ディレクトリを解決する。
fn config_dir() -> PathBuf {
    if let Ok(appdata) = std::env::var("APPDATA")
        && !appdata.trim().is_empty()
    {
        return PathBuf::from(appdata);
    }
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME")
        && !xdg.trim().is_empty()
    {
        return PathBuf::from(xdg);
    }
    if let Ok(home) = std::env::var("HOME")
        && !home.trim().is_empty()
    {
        return PathBuf::from(home).join(".config");
    }
    PathBuf::from(".")
}

/// 最大件数を超えた古いレコード (末尾側) を破棄する。
fn truncate_oldest(records: &mut Vec<RunRecord>, max_records: usize) {
    if records.len() > max_records {
        records.truncate(max_records);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    fn sample_record(strategy: &str, started: u64) -> RunRecord {
        RunRecord::new(
            started,
            strategy,
            RunOutcome::Success,
            Some(0),
            vec!["line1".to_string(), "line2".to_string()],
        )
    }

    // --- 正常系 ---

    #[test]
    fn append_persists_and_reload_returns_record() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.jsonl");
        {
            let mut h = RunHistory::open(&path, DEFAULT_MAX_RECORDS);
            h.append(sample_record("fishing", 1000)).unwrap();
            h.append(RunRecord::new(
                2000,
                "main_story",
                RunOutcome::Failed,
                Some(1),
                vec![],
            ))
            .unwrap();
        }
        let h = RunHistory::open(&path, DEFAULT_MAX_RECORDS);
        let rs = h.records();
        assert_eq!(rs.len(), 2);
        // 新しい順
        assert_eq!(rs[0].strategy, "main_story");
        assert_eq!(rs[0].outcome, RunOutcome::Failed);
        assert_eq!(rs[0].exit_code, Some(1));
        assert_eq!(rs[1].strategy, "fishing");
        assert_eq!(
            rs[1].log_tail,
            vec!["line1".to_string(), "line2".to_string()]
        );
    }

    #[test]
    fn history_path_respects_env_override() {
        // env-var テストは並列実行と競合しうるため一時的に排他は諦め、
        // 値を設定して即検証・復元する (他テストは open(path) 直指定で環境非依存)。
        let orig = std::env::var(HISTORY_ENV_VAR).ok();
        // safety: テストバイナリ内で他スレッドが環境変数を読み書きしていない
        // (当テストモジュールの他テストは環境非依存)。単一テスト内で完結。
        unsafe {
            std::env::set_var(HISTORY_ENV_VAR, "/tmp/custom-history.jsonl");
        }
        let p = history_path();
        unsafe {
            std::env::remove_var(HISTORY_ENV_VAR);
        }
        if let Some(o) = orig {
            unsafe {
                std::env::set_var(HISTORY_ENV_VAR, o);
            }
        }
        assert_eq!(p, PathBuf::from("/tmp/custom-history.jsonl"));
    }

    #[test]
    fn record_serializes_to_jsonl_line() {
        let r = sample_record("fishing", 42);
        let json = serde_json::to_string(&r).unwrap();
        let back: RunRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(back, r);
    }

    // --- エッジケース ---

    #[test]
    fn max_records_limit_drops_oldest() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.jsonl");
        let mut h = RunHistory::open(&path, 3);
        for i in 0..5u64 {
            h.append(sample_record("s", i)).unwrap();
        }
        assert_eq!(h.records().len(), 3);
        // 最新 3 件 (4, 3, 2) が残る
        assert_eq!(
            h.records()
                .iter()
                .map(|r| r.started_at_unix)
                .collect::<Vec<_>>(),
            vec![4, 3, 2]
        );
        // 再読み込み後も件数維持
        let h2 = RunHistory::open(&path, 3);
        assert_eq!(h2.records().len(), 3);
    }

    #[test]
    fn log_tail_truncated_to_limit() {
        let lines: Vec<String> = (0..100).map(|i| format!("l{i}")).collect();
        let r = RunRecord::new(1, "s", RunOutcome::Cancelled, None, lines.clone());
        assert_eq!(r.log_tail.len(), LOG_TAIL_MAX_LINES);
        assert_eq!(r.log_tail.first().unwrap(), "l80");
        assert_eq!(r.log_tail.last().unwrap(), "l99");
    }

    #[test]
    fn corrupt_lines_are_skipped_on_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.jsonl");
        let good = serde_json::to_string(&sample_record("ok", 1)).unwrap();
        let content = format!("{{broken json\n{good}\n\nnot-a-record\n");
        std::fs::write(&path, content).unwrap();
        let h = RunHistory::open(&path, DEFAULT_MAX_RECORDS);
        assert_eq!(h.records().len(), 1);
        assert_eq!(h.records()[0].strategy, "ok");
    }

    #[test]
    fn open_missing_file_yields_empty_history() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nonexistent.jsonl");
        let h = RunHistory::open(&path, DEFAULT_MAX_RECORDS);
        assert!(h.records().is_empty());
    }
}
