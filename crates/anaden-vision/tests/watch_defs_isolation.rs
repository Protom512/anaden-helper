use std::path::PathBuf;

use anaden_vision::{load_pipeline, load_watch_defs};

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

fn nav_dir() -> PathBuf {
    workspace_root()
        .join("templates")
        .join("pipelines")
        .join("nav_to_field_pc")
}

#[test]
#[allow(clippy::unwrap_used)]
fn chain_loader_excludes_watch_subdir() {
    let chain_names: Vec<String> = load_pipeline(&nav_dir())
        .unwrap_or_else(|e| panic!("load nav chain: {e}"))
        .into_iter()
        .map(|d| d.name)
        .collect();
    assert!(
        chain_names.contains(&"TapToStartPc".to_string())
            && chain_names.contains(&"LoadGamePc".to_string())
            && chain_names.contains(&"FieldHudTopPc".to_string()),
        "nav chain defs must stay the pinned single-chain graph, got {chain_names:?}"
    );
    assert!(
        !chain_names.contains(&"FieldTownBarPc".to_string()),
        "watch sentinel must NOT appear in the chain loader: the single-linear-chain \
         cold-start contract (next.len()<=1 walk, single indegree-0 start) is pinned by \
         structural tests and a chain-visible watch def would break it"
    );
}

#[test]
#[allow(clippy::unwrap_used)]
fn watch_loader_reads_subdir_and_resolves_template_paths() {
    let defs = load_watch_defs(&nav_dir()).unwrap_or_else(|e| panic!("load watch defs: {e}"));
    assert_eq!(
        defs.len(),
        1,
        "nav_to_field_pc ships exactly one watch sentinel"
    );
    let def = &defs[0];
    assert_eq!(def.name, "FieldTownBarPc");
    assert!(def.template.is_absolute(), "template path must be absolute");
    assert!(
        def.template
            .parent()
            .map(|p| p.ends_with("watch"))
            .unwrap_or(false),
        "watch template must resolve under the watch/ subdir, got {:?}",
        def.template
    );
    assert!(
        def.template.exists(),
        "watch template PNG must exist at {:?}",
        def.template
    );
}

#[test]
#[allow(clippy::unwrap_used)]
fn watch_loader_returns_empty_without_subdir() {
    let tmp = tempfile::tempdir().unwrap();
    let defs = load_watch_defs(tmp.path())
        .unwrap_or_else(|e| panic!("load watch defs from empty dir: {e}"));
    assert!(
        defs.is_empty(),
        "no watch/ subdir means no watch defs (empty, not an error)"
    );
}

#[test]
#[allow(clippy::unwrap_used)]
fn manifest_watch_names_are_covered_by_watch_defs() {
    let manifest = anaden_vision::load_pipeline_manifest(&nav_dir())
        .unwrap_or_else(|e| panic!("load manifest: {e}"));
    let watch_def_names: Vec<String> = load_watch_defs(&nav_dir())
        .unwrap_or_else(|e| panic!("load watch defs: {e}"))
        .into_iter()
        .map(|d| d.name)
        .collect();
    for name in &manifest.watch {
        assert!(
            watch_def_names.contains(name),
            "manifest watch name '{name}' must have a def in watch/: the watch layer is \
             resolved from watch/ defs, not from the chain graph"
        );
    }
}
