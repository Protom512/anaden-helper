use std::io::Write as _;
use std::path::{Path, PathBuf};

use async_trait::async_trait;

pub const EVIDENCE_LOG_FILE: &str = "uc2-e2e-evidence.log";

pub const EVIDENCE_META_FILE: &str = "uc2-e2e-metadata.json";

pub const SCREENSHOT_PREFIX: &str = "uc2-shot";

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

#[must_use]
pub fn unix_now() -> u64 {
    unix_now_millis() / 1000
}

#[must_use]
pub fn unix_now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[must_use]
pub(crate) fn date_parts_unix(unix_secs: u64) -> (i64, u64, u64, u64, u64, u64) {
    let days = (unix_secs / 86_400) as i64;
    let secs_of_day = unix_secs % 86_400;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (
        y,
        m as u64,
        d as u64,
        secs_of_day / 3600,
        (secs_of_day % 3600) / 60,
        secs_of_day % 60,
    )
}

#[must_use]
pub fn iso8601_utc(unix_secs: u64) -> String {
    let (y, m, d, hh, mm, ss) = date_parts_unix(unix_secs);
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

#[must_use]
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

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

pub struct EvidenceCapture<C> {
    inner: C,
    dir: PathBuf,
    cycle: std::sync::atomic::AtomicU32,
}

impl<C: anaden_engine::Capture + Send + Sync> EvidenceCapture<C> {
    pub fn new(inner: C, dir: PathBuf) -> Self {
        Self {
            inner,
            dir,
            cycle: std::sync::atomic::AtomicU32::new(0),
        }
    }

    #[cfg(test)]
    pub fn cycle_count(&self) -> u32 {
        self.cycle.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[async_trait]
impl<C: anaden_engine::Capture + Send + Sync> anaden_engine::Capture for EvidenceCapture<C> {
    async fn capture(&self) -> Result<image::DynamicImage, anaden_device::DeviceError> {
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

    #[test]
    fn iso8601_known_epoch() {
        assert_eq!(iso8601_utc(0), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn iso8601_known_2026() {
        assert_eq!(iso8601_utc(1788220800), "2026-09-01T00:00:00Z");
    }

    #[test]
    fn iso8601_leap_day_boundary() {
        assert_eq!(iso8601_utc(1_709_251_199), "2024-02-29T23:59:59Z");
        assert_eq!(iso8601_utc(1_709_251_200), "2024-03-01T00:00:00Z");
    }

    #[test]
    fn fnv1a_empty_is_offset_basis() {
        assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
    }

    #[test]
    fn fnv1a_known_vector() {
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
    }

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

    #[test]
    fn tree_hash_is_deterministic_and_content_sensitive() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.toml"), b"task_a").unwrap();
        std::fs::write(dir.path().join("b.toml"), b"task_b").unwrap();
        let h1 = tree_hash_of_pipeline(dir.path());
        let h2 = tree_hash_of_pipeline(dir.path());
        assert_eq!(h1, h2);
        assert!(h1.starts_with("fnv-1a64:"), "{h1}");

        std::fs::write(dir.path().join("a.toml"), b"task_A_CHANGED").unwrap();
        let h3 = tree_hash_of_pipeline(dir.path());
        assert_ne!(h1, h3);

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

    struct FakeCapture;

    #[async_trait]
    impl anaden_engine::Capture for FakeCapture {
        async fn capture(&self) -> Result<image::DynamicImage, anaden_device::DeviceError> {
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
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("not-a-dir");
        std::fs::write(&file_path, b"x").unwrap();
        let deco = EvidenceCapture::new(FakeCapture, file_path);
        let img = deco.capture().await.unwrap();
        assert_eq!(img.width(), 2);
        assert_eq!(deco.cycle_count(), 1);
    }
}
