//! UC-2 実機 E2E 証跡ヘルパー (Issue #139 T6)。
//!
//! `.claude/rules/pipeline-evidence-verification.md` 準拠の evidence 採取:
//! - 生コマンド出力・スクショ・tree hash を `.omc/logs/{run-id}/` へ永続化
//! - evidence は自己申告不可 (= 機械的検証可能な形式でファイルへ書く)
//! - メタデータ (runTimestamp / command / exit code) を JSON で残す
//!
//! GUI (anaden-studio) は本モジュールを直接呼ばない。CLI 側 `--evidence-dir`
//! フラグ (`run` サブコマンド) が採取を担い、studio は子プロセス引数へ
//! `--evidence-dir` を渡すのみ (contract coupling: 子プロセス境界)。

use std::io::Write as _;
use std::path::{Path, PathBuf};

use async_trait::async_trait;

/// E2E run ディレクトリ直下のEvidence ログ ファイル名 (生コマンド出力)。
pub const EVIDENCE_LOG_FILE: &str = "uc2-e2e-evidence.log";

/// メタデータ JSON ファイル名 (機械的検証可能な evidence 索引)。
pub const EVIDENCE_META_FILE: &str = "uc2-e2e-metadata.json";

/// スクショ PNG のファイル名プレフィクス。
pub const SCREENSHOT_PREFIX: &str = "uc2-shot";

/// E2E 証跡ディレクトリ (`{evidence_dir}` 直下に run-id ディレクトリを作る)。
///
/// `run_id` が空・`/`/`\`/`..` を含む場合は fail-closed として `None` を返す
/// (パス インジェクション・意図しないディレクトリ逸脱を防ぐ)。
#[must_use]
pub fn e2e_run_dir(evidence_dir: &Path, run_id: &str) -> Option<PathBuf> {
    if run_id.is_empty()
        || run_id == "."
        || run_id == ".."
        || run_id.contains('/')
        || run_id.contains('\\')
    {
        return None;
    }
    Some(evidence_dir.join(run_id))
}

/// `SystemTime` を Unix 秒へ射影 (メタデータの runTimestamp 生成用・テスト可能)。
#[must_use]
pub fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// ISO 8601 (UTC, 秒精度) 文字列を Unix 秒から生成。
///
/// 実装はcivil-from-days (Howard Hinnant アルゴリズム) の手書き変換。
/// chrono 依存を増やさないための最小実装 (テストで既知値を検証)。
#[must_use]
pub fn iso8601_utc(unix_secs: u64) -> String {
    let days = (unix_secs / 86_400) as i64;
    let secs_of_day = unix_secs % 86_400;
    // civil_from_days: days=1970-01-01 起点の経過日 → y/m/d
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        secs_of_day / 3600,
        (secs_of_day % 3600) / 60,
        secs_of_day % 60
    )
}

/// FNV-1a 64bit hash (tree hash 計算の単一情報源)。
///
/// `git write-tree` 相当の tree hash は git コマンド実行で取得するが、
/// git が無い環境でも evidence を hash 付与可能にするため、
/// 実行コマンド文字列 + パイプライン定義一式の内容 hash を本関数で計算する。
#[must_use]
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

/// evidence ログへ 1 行 (生コマンド出力) を追記する。
///
/// ファイルが無ければ作成する。append 失敗は呼出側でハンドリングできるよう
/// `io::Result` を返す (panic しない)。
pub fn append_evidence_line(dir: &Path, line: &str) -> std::io::Result<()> {
    use std::fs::OpenOptions;
    if !dir.exists() {
        std::fs::create_dir_all(dir)?;
    }
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(EVIDENCE_LOG_FILE))?;
    writeln!(f, "{line}")
}

/// スクショ PNG を `{dir}/{SCREENSHOT_PREFIX}-{seq:03}.png` へ保存する。
///
/// 保存済み最大 seq + 1 を次の seq とする (既存ファイル走査・決定的)。
pub fn save_screenshot(dir: &Path, image: &image::DynamicImage) -> std::io::Result<PathBuf> {
    if !dir.exists() {
        std::fs::create_dir_all(dir)?;
    }
    let mut seq: u32 = 0;
    loop {
        let path = dir.join(format!("{SCREENSHOT_PREFIX}-{seq:03}.png"));
        if !path.exists() {
            image
                .save_with_format(&path, image::ImageFormat::Png)
                .map_err(|e| {
                    std::io::Error::other(format!("PNG 保存失敗 {}: {e}", path.display()))
                })?;
            return Ok(path);
        }
        seq = seq.saturating_add(1);
    }
}

/// 保存済みスクショの件数 (認識サイクル証明の実測値カウント用)。
#[must_use]
pub fn count_screenshots(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter(|e| {
                    let n = e.file_name();
                    let n = n.to_string_lossy();
                    n.starts_with(SCREENSHOT_PREFIX) && n.ends_with(".png")
                })
                .count()
        })
        .unwrap_or(0)
}

/// メタデータ JSON (手書きシリアライズ — evidence 索引は本関数が単一情報源)。
///
/// フィールド: runId / runTimestamp / command / treeHash / screenshots /
/// evidenceLogFile / recordedAtUnix。JSON エスケープは最小 (\"" と \\ のみ。
/// command は外部入力ではない CLI 引数由来だが、念のためエスケープする)。
pub fn write_metadata(
    dir: &Path,
    run_id: &str,
    run_timestamp: &str,
    command: &str,
    tree_hash: &str,
    screenshots: usize,
) -> std::io::Result<PathBuf> {
    if !dir.exists() {
        std::fs::create_dir_all(dir)?;
    }
    let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    let json = format!(
        "{{\n  \"runId\": \"{}\",\n  \"runTimestamp\": \"{}\",\n  \"command\": \"{}\",\n  \"treeHash\": \"{}\",\n  \"screenshots\": {},\n  \"evidenceLogFile\": \"{}\",\n  \"recordedAtUnix\": {}\n}}\n",
        esc(run_id),
        esc(run_timestamp),
        esc(command),
        esc(tree_hash),
        screenshots,
        EVIDENCE_LOG_FILE,
        unix_now()
    );
    let path = dir.join(EVIDENCE_META_FILE);
    std::fs::write(&path, json)?;
    Ok(path)
}

/// 認識サイクル証明用 `Capture` デコレータ (Issue #139 T6)。
///
/// 内側の `Capture` 実装 (Win32Capture 等) へ委譲しつつ、**サイクル毎に**:
/// 1. スクショ PNG を evidence ディレクトリへ保存 (`uc2-shot-NNN.png`)
/// 2. 認証ログ行 (`cycle=N size=WxH`) を evidence ログへ追記
///
/// 保存失敗でもキャプチャ自体は成功扱いとする (evidence 採取が本番実行を
/// 壊さない)。ただし失敗はログ行へ記録する (静黙化しない)。
pub struct EvidenceCapture<C> {
    inner: C,
    dir: PathBuf,
    cycle: std::sync::atomic::AtomicU32,
}

impl<C: anaden_engine::Capture + Send + Sync> EvidenceCapture<C> {
    /// デコレータを構築。`dir` は e2e_run_dir で検証済みの run ディレクトリ。
    pub fn new(inner: C, dir: PathBuf) -> Self {
        Self {
            inner,
            dir,
            cycle: std::sync::atomic::AtomicU32::new(0),
        }
    }

    /// 現在のサイクル数 (テスト用)。
    #[cfg(test)]
    pub fn cycle_count(&self) -> u32 {
        self.cycle.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[async_trait]
impl<C: anaden_engine::Capture + Send + Sync> anaden_engine::Capture for EvidenceCapture<C> {
    async fn capture(&self) -> Result<image::DynamicImage, anaden_device::AdbError> {
        let img = self.inner.capture().await?;
        let n = self.cycle.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        match save_screenshot(&self.dir, &img) {
            Ok(path) => {
                let _ = append_evidence_line(
                    &self.dir,
                    &format!(
                        "cycle={n} screenshot={} size={}x{}",
                        path.file_name()
                            .map(|f| f.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                        img.width(),
                        img.height()
                    ),
                );
            }
            Err(e) => {
                let _ = append_evidence_line(&self.dir, &format!("cycle={n} screenshot-error={e}"));
            }
        }
        Ok(img)
    }
}

/// パイプラインディレクトリ内の `*.toml` 全ファイルの内容をソート順に連結して
/// FNV-1a 64 でハッシュした tree hash (`fnv-1a64:<hex>`) を返す。
///
/// git が無い環境でも pipeline 定義の同一性を機械検証可能にするための
/// content hash (`.claude/rules/pipeline-evidence-verification.md` §1.1 tree hash)。
/// ディレクトリが読めない/空の場合は `fnv-1a64:0` を返す (fail-closed: hash 欠損を
/// 捏造しない)。
#[must_use]
pub fn tree_hash_of_pipeline(dir: &Path) -> String {
    let mut names: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().map(|x| x == "toml").unwrap_or(false))
            .collect(),
        Err(_) => return "fnv-1a64:0".to_string(),
    };
    names.sort();
    let mut buf: Vec<u8> = Vec::new();
    for p in &names {
        if let Ok(content) = std::fs::read(p) {
            buf.extend_from_slice(p.to_string_lossy().as_bytes());
            buf.extend_from_slice(&content);
        }
    }
    format!("fnv-1a64:{:016x}", fnv1a64(&buf))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use anaden_engine::Capture as _;

    // ---- e2e_run_dir (fail-closed パス検証) ----

    #[test]
    fn e2e_run_dir_joins_run_id() {
        let got = e2e_run_dir(Path::new(".omc/logs"), "run-e2e-123").unwrap();
        assert_eq!(got, Path::new(".omc/logs").join("run-e2e-123"));
    }

    #[test]
    fn e2e_run_dir_rejects_empty() {
        assert!(e2e_run_dir(Path::new(".omc/logs"), "").is_none());
    }

    #[test]
    fn e2e_run_dir_rejects_traversal() {
        assert!(e2e_run_dir(Path::new(".omc/logs"), "..").is_none());
        assert!(e2e_run_dir(Path::new(".omc/logs"), "a/b").is_none());
        assert!(e2e_run_dir(Path::new(".omc/logs"), "a\\b").is_none());
    }

    // ---- iso8601_utc (既知値検証) ----

    #[test]
    fn iso8601_known_epoch() {
        assert_eq!(iso8601_utc(0), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn iso8601_known_2026() {
        // 2026-09-01T00:00:00Z = 1788220800 (実測: python datetime UTC)
        assert_eq!(iso8601_utc(1788220800), "2026-09-01T00:00:00Z");
    }

    #[test]
    fn iso8601_leap_day_boundary() {
        // 2024-02-29T23:59:59Z = 1709251199
        assert_eq!(iso8601_utc(1_709_251_199), "2024-02-29T23:59:59Z");
        // 2024-03-01T00:00:00Z = 1709251200
        assert_eq!(iso8601_utc(1_709_251_200), "2024-03-01T00:00:00Z");
    }

    // ---- fnv1a64 (テストベクトル) ----

    #[test]
    fn fnv1a_empty_is_offset_basis() {
        assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
    }

    #[test]
    fn fnv1a_known_vector() {
        // FNV-1a 64: "a" = 0xaf63dc4c8601ec8c
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
    }

    // ---- append_evidence_line / save_screenshot / count (統合) ----

    #[test]
    fn evidence_roundtrip_log_screenshot_count() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();

        append_evidence_line(
            d,
            "ANADEN_E2E command=anaden run --target windows field_loop start=field",
        )
        .unwrap();
        append_evidence_line(d, "cycle=0 Match task=field score=0.95").unwrap();

        let content = std::fs::read_to_string(d.join(EVIDENCE_LOG_FILE)).unwrap();
        assert!(content.contains("command=anaden run"));
        assert!(content.contains("cycle=0 Match"));

        let img = image::DynamicImage::new_rgb8(4, 4);
        let p1 = save_screenshot(d, &img).unwrap();
        let p2 = save_screenshot(d, &img).unwrap();
        assert!(p1.to_string_lossy().contains("uc2-shot-000.png"));
        assert!(p2.to_string_lossy().contains("uc2-shot-001.png"));
        assert_eq!(count_screenshots(d), 2);
    }

    #[test]
    fn evidence_log_appends_not_truncates() {
        let dir = tempfile::tempdir().unwrap();
        append_evidence_line(dir.path(), "line1").unwrap();
        append_evidence_line(dir.path(), "line2").unwrap();
        let content = std::fs::read_to_string(dir.path().join(EVIDENCE_LOG_FILE)).unwrap();
        assert!(content.contains("line1") && content.contains("line2"));
    }

    // ---- write_metadata ----

    #[test]
    fn metadata_contains_required_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_metadata(
            dir.path(),
            "run-e2e-1",
            "2026-09-01T00:00:00Z",
            "anaden run --target windows templates/pipelines/field_loop field",
            "fnv-1a64:deadbeef",
            3,
        )
        .unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        for required in [
            "\"runId\": \"run-e2e-1\"",
            "\"runTimestamp\": \"2026-09-01T00:00:00Z\"",
            "\"command\":",
            "\"treeHash\": \"fnv-1a64:deadbeef\"",
            "\"screenshots\": 3",
            "\"evidenceLogFile\": \"uc2-e2e-evidence.log\"",
        ] {
            assert!(
                content.contains(required),
                "missing {required} in:\n{content}"
            );
        }
    }

    #[test]
    fn metadata_escapes_quotes_in_command() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_metadata(
            dir.path(),
            "r",
            "2026-09-01T00:00:00Z",
            "say \"hi\" \\ back",
            "h",
            0,
        )
        .unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("say \\\"hi\\\" \\\\ back"), "{content}");
    }

    // ---- tree_hash_of_pipeline ----

    #[test]
    fn tree_hash_is_deterministic_and_content_sensitive() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.toml"), b"task_a").unwrap();
        std::fs::write(dir.path().join("b.toml"), b"task_b").unwrap();
        let h1 = tree_hash_of_pipeline(dir.path());
        let h2 = tree_hash_of_pipeline(dir.path());
        assert_eq!(h1, h2);
        assert!(h1.starts_with("fnv-1a64:"), "{h1}");

        // 内容変更で hash が変わる。
        std::fs::write(dir.path().join("a.toml"), b"task_A_CHANGED").unwrap();
        let h3 = tree_hash_of_pipeline(dir.path());
        assert_ne!(h1, h3);

        // 非 toml は対象外。
        std::fs::write(dir.path().join("ignore.txt"), b"x").unwrap();
        assert_eq!(h3, tree_hash_of_pipeline(dir.path()));
    }

    #[test]
    fn tree_hash_missing_dir_is_fail_closed_marker() {
        assert_eq!(
            tree_hash_of_pipeline(Path::new("C:/definitely/not/here")),
            "fnv-1a64:0"
        );
    }

    /// 常に 2x2 の RGB 画像を返す fake Capture (デバイス非依存テスト用)。
    struct FakeCapture;

    #[async_trait]
    impl anaden_engine::Capture for FakeCapture {
        async fn capture(&self) -> Result<image::DynamicImage, anaden_device::AdbError> {
            Ok(image::DynamicImage::new_rgb8(2, 2))
        }
    }

    #[tokio::test]
    async fn evidence_capture_saves_screenshot_and_log_per_cycle() {
        let dir = tempfile::tempdir().unwrap();
        let deco = EvidenceCapture::new(FakeCapture, dir.path().to_path_buf());

        let img = deco.capture().await.unwrap();
        assert_eq!((img.width(), img.height()), (2, 2));
        let img2 = deco.capture().await.unwrap();
        assert_eq!((img2.width(), img2.height()), (2, 2));

        // 2 サイクル = 2 スクショ + 2 ログ行。
        assert_eq!(deco.cycle_count(), 2);
        assert_eq!(count_screenshots(dir.path()), 2);
        let log = std::fs::read_to_string(dir.path().join(EVIDENCE_LOG_FILE)).unwrap();
        assert!(
            log.contains("cycle=0 screenshot=uc2-shot-000.png size=2x2"),
            "{log}"
        );
        assert!(
            log.contains("cycle=1 screenshot=uc2-shot-001.png size=2x2"),
            "{log}"
        );
    }

    #[tokio::test]
    async fn evidence_capture_survives_save_failure() {
        // evidence ディレクトリとして「ファイル」を指す → save_screenshot は失敗するが
        // capture 自体は成功する (evidence 採取が本番実行を壊さない)。
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("not-a-dir");
        std::fs::write(&file_path, b"x").unwrap();
        let deco = EvidenceCapture::new(FakeCapture, file_path);
        let img = deco.capture().await.unwrap();
        assert_eq!(img.width(), 2);
        // cycle は消費されている (呼び出し回数カウント)。
        assert_eq!(deco.cycle_count(), 1);
    }
}
