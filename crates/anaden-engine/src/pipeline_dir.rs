//! pipeline ディレクトリ指定の決定的解決 (単一実装・Issue #202 UC-3)。
//!
//! 従来 `anaden run` (CLI main.rs) と routine (engine routine.rs) がそれぞれ
//! 同一候補順の resolver を二重実装していた (PR #201 lane2 minor)。
//! 本モジュールへ統一し、CLI 側は失敗時「元のパスを返す」契約の薄い委譲に
//! なった (`anaden-cli` main.rs `resolve_pipeline_dir`)。
//!
//! 解決は **workdir 非依存** (基準点は manifest 由来の workspace ルート)。
//! 従来 `run` は pipeline_dir を cwd 相対で `load_pipeline` に渡していたため、
//! 起動 workdir 次第で「パイプライン読込失敗」になっていた (GUI 子プロセス
//! 起動で顕在化・Issue #139 T2 の動機)。

use std::path::{Path, PathBuf};

/// pipeline_dir 指定を決定的に解決する (単一情報源)。
///
/// 候補順 (最初に実在するディレクトリを採用):
/// 1. 与えられたパス自体 (相対・絶対を問わず cwd 解決)
/// 2. `<root>/<与えられた相対パス>` (例: `templates/pipelines/login`)。
///    絶対パスで非実在の場合は basename のみ候補とする
///    (例: `C:\x\login` → `<root>/login`)。
/// 3. `<root>/templates/pipelines/<相対パス>` (bare name `login` 等の解決先。
///    `templates` コンポーネントを含む指定は自己解決済みのため対象外)
///
/// いずれも実在しなければ [`None`] を返す (fail-closed: 偽パスを捏造しない)。
/// 呼出側の契約に応じて:
/// - routine 系 (engine) は [`None`] を [`crate::RoutineError`] 等へ変換
/// - `run` (CLI) は元のパスをそのまま返し下流 `load_pipeline` の
///   fail-closed エラーに委譲する
#[must_use]
pub fn resolve_pipeline_dir(input: &Path, root: &Path) -> Option<PathBuf> {
    if input.is_dir() {
        return Some(input.to_path_buf());
    }
    let rel = if input.is_absolute() {
        // 絶対パスで非実在の場合は候補 2/3 の basename のみ試す。
        PathBuf::from(input.file_name()?.to_string_lossy().into_owned())
    } else {
        input.to_path_buf()
    };
    if rel.as_os_str().is_empty() {
        return None;
    }
    let joined = root.join(&rel);
    if joined.is_dir() {
        return Some(joined);
    }
    // bare name (`login` 等) は templates/pipelines 基準で解決する。
    // `templates` コンポーネントを含む指定 (例: `templates/pipelines/ghost`) は
    // 候補 2 で解決済みのため候補 3 の対象外 (二重接頭は試さない)。
    if !rel.components().any(|c| c.as_os_str() == "templates") {
        let pipelined = root.join("templates").join("pipelines").join(&rel);
        if pipelined.is_dir() {
            return Some(pipelined);
        }
    }
    None
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    fn make_pipeline(root: &Path, name: &str) -> PathBuf {
        let dir = root.join("templates").join("pipelines").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    // ---- 候補 1: パス自体 ----

    #[test]
    fn resolves_existing_cwd_relative_path_as_is() {
        let dir = tempfile::tempdir().unwrap();
        let input = make_pipeline(dir.path(), "x");
        assert_eq!(resolve_pipeline_dir(&input, dir.path()), Some(input));
    }

    #[test]
    fn absolute_existing_path_resolves_to_itself() {
        let dir = tempfile::tempdir().unwrap();
        let input = make_pipeline(dir.path(), "field_loop");
        assert_eq!(
            resolve_pipeline_dir(&input, dir.path()),
            Some(input.clone())
        );
    }

    // ---- 候補 2: root 相対 ----

    #[test]
    fn relative_template_path_falls_back_to_workspace_root() {
        let root = tempfile::tempdir().unwrap();
        let dir = make_pipeline(root.path(), "field_loop_pc");
        let input = PathBuf::from("templates/pipelines/field_loop_pc");
        assert_eq!(resolve_pipeline_dir(&input, root.path()), Some(dir));
    }

    #[test]
    fn absolute_nonexistent_tries_basename_under_root() {
        let root = tempfile::tempdir().unwrap();
        let dir = make_pipeline(root.path(), "login");
        let input = PathBuf::from("C:/definitely/not/here/login");
        assert_eq!(resolve_pipeline_dir(&input, root.path()), Some(dir));
    }

    // ---- 候補 3: templates/pipelines 基準 ----

    #[test]
    fn bare_pipeline_name_resolves_under_templates_pipelines() {
        let root = tempfile::tempdir().unwrap();
        let dir = make_pipeline(root.path(), "field_loop_pc");
        let input = PathBuf::from("field_loop_pc");
        assert_eq!(resolve_pipeline_dir(&input, root.path()), Some(dir));
    }

    #[test]
    fn nested_relative_path_also_tries_templates_pipelines() {
        // `run` (CLI) 従来契約: 相対パスは候補 3 でもテンプレ基準で試す。
        let root = tempfile::tempdir().unwrap();
        let dir = root
            .path()
            .join("templates")
            .join("pipelines")
            .join("a")
            .join("b");
        std::fs::create_dir_all(&dir).unwrap();
        let input = PathBuf::from("a/b");
        assert_eq!(resolve_pipeline_dir(&input, root.path()), Some(dir));
    }

    // ---- fail-closed ----

    #[test]
    fn nonexistent_relative_returns_none() {
        let root = tempfile::tempdir().unwrap();
        let input = PathBuf::from("templates/pipelines/ghost");
        assert_eq!(resolve_pipeline_dir(&input, root.path()), None);
    }

    #[test]
    fn nonexistent_bare_name_returns_none() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(
            resolve_pipeline_dir(&PathBuf::from("ghost"), root.path()),
            None
        );
    }

    #[test]
    fn templates_prefixed_path_does_not_double_join_prefix() {
        // `templates/pipelines/x` が非実在の場合、候補 3 で
        // `templates/pipelines/templates/pipelines/x` は試さない (無駄打ち検証)。
        let root = tempfile::tempdir().unwrap();
        let dir = root
            .path()
            .join("templates")
            .join("pipelines")
            .join("templates")
            .join("pipelines")
            .join("x");
        std::fs::create_dir_all(&dir).unwrap();
        let input = PathBuf::from("templates/pipelines/x");
        assert_eq!(resolve_pipeline_dir(&input, root.path()), None);
    }

    // ---- 実ワークスペース (manifest 由来ルート) ----

    fn workspace_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
    }

    /// 実在 pipeline の相対指定・bare name 指定がすべて実ディレクトリへ解決される
    /// (Issue #188 で android 用 4 ディレクトリを削除済みの現行構成)。
    #[test]
    fn all_real_pipelines_resolve_on_real_root() {
        let root = workspace_root();
        for id in ["field_loop_pc", "nav_to_field_pc", "fishing", "login"] {
            let rel = PathBuf::from(format!("templates/pipelines/{id}"));
            let got = resolve_pipeline_dir(&rel, &root)
                .unwrap_or_else(|| panic!("{id}: relative form must resolve on real root"));
            assert!(got.is_dir(), "{id}: {got:?}");

            let bare = resolve_pipeline_dir(&PathBuf::from(id), &root)
                .unwrap_or_else(|| panic!("{id}: bare name must resolve on real root"));
            assert!(bare.is_dir(), "{id}: {bare:?}");
        }
    }
}
