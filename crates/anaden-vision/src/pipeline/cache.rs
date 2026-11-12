//! 準備済みテンプレートのプロセス全体キャッシュ。
//!
//! キャッシュは [`super::recognize`] からだけ使われる実装詳細。現状は
//! プロセス全体の `static` で、mtime が変わったときだけ作り直す。
//! 「ファイルを消しても一致し続ける」ことを前提にしたテストがあるため、
//! インスタンス化は別の変更として扱う。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::SystemTime;

use image::DynamicImage;
use tracing::debug;

use crate::ccoeff::PreparedCcoeff;

use super::schema::TaskDefError;

pub(crate) struct PreparedTemplate {
    pub(crate) needle: DynamicImage,
    pub(crate) ccoeff: PreparedCcoeff,
}

pub(crate) struct CachedTemplate {
    pub(crate) mtime: Option<SystemTime>,
    pub(crate) prepared: Arc<PreparedTemplate>,
}

pub(crate) struct TemplateCache {
    pub(crate) entries: Mutex<HashMap<(PathBuf, u32, u32), CachedTemplate>>,
}

impl Default for TemplateCache {
    fn default() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
        }
    }
}

impl TemplateCache {
    fn get_or_prepare(
        &self,
        path: &Path,
        norm_w: u32,
        norm_h: u32,
    ) -> Result<Arc<PreparedTemplate>, TaskDefError> {
        let key = (path.to_path_buf(), norm_w, norm_h);
        let mut entries = match self.entries.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(hit) = entries.get(&key) {
            if mtime_unchanged(path, hit.mtime) {
                return Ok(hit.prepared.clone());
            }
            debug!("template cache invalidated (mtime changed): {path:?}");
            entries.remove(&key);
        }
        let needle = image::open(path).map_err(|e| TaskDefError::TemplateLoadFailed {
            path: path.to_path_buf(),
            reason: e.to_string(),
        })?;
        let mtime = file_mtime(path);
        let normalized = crate::scale::needle_to_normalized(&needle, norm_w, norm_h);
        let ccoeff = PreparedCcoeff::prepare(&normalized);
        let prepared = Arc::new(PreparedTemplate {
            needle: normalized,
            ccoeff,
        });
        entries.insert(
            key,
            CachedTemplate {
                mtime,
                prepared: prepared.clone(),
            },
        );
        Ok(prepared)
    }
}

pub(crate) fn file_mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

pub(crate) fn mtime_unchanged(path: &Path, cached: Option<SystemTime>) -> bool {
    match (file_mtime(path), cached) {
        (Some(current), Some(cached)) => current == cached,
        (None, _) => true,
        (Some(_), None) => false,
    }
}

pub(crate) static TEMPLATE_CACHE: LazyLock<TemplateCache> = LazyLock::new(TemplateCache::default);

pub(crate) fn prepared_template(
    path: &Path,
    norm_w: u32,
    norm_h: u32,
) -> Result<Arc<PreparedTemplate>, TaskDefError> {
    TEMPLATE_CACHE.get_or_prepare(path, norm_w, norm_h)
}
