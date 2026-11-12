use std::path::PathBuf;

use anaden_vision::{Action, load_pipeline_manifest, load_watch_defs};

fn login_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("templates")
        .join("pipelines")
        .join("login")
}

#[test]
#[allow(clippy::panic)]
fn login_ships_one_arrival_sentinel_outside_the_chain() {
    let defs =
        load_watch_defs(&login_dir()).unwrap_or_else(|e| panic!("load login watch defs: {e}"));
    assert_eq!(
        defs.len(),
        1,
        "the login pipeline ships exactly one arrival sentinel (DEF-008 pattern)"
    );
    let def = &defs[0];
    assert_eq!(def.name, "LoginFieldTownBarPc");
    assert!(
        matches!(def.action, Some(Action::Stop)),
        "an arrival sentinel must stop the loop, got {:?}",
        def.action
    );
    assert!(
        def.template.is_absolute(),
        "watch template path must resolve to an absolute path, got {:?}",
        def.template
    );
    assert!(
        def.template.exists(),
        "watch template PNG must exist at {:?}",
        def.template
    );
    let normalized = def.template.to_string_lossy().replace('\\', "/");
    assert!(
        normalized.ends_with("nav_to_field_pc/watch/town_bar.png"),
        "the login sentinel reuses the live-proven town bar needle, got {:?}",
        def.template
    );
}

#[test]
#[allow(clippy::panic)]
fn login_manifest_declares_the_sentinel_it_ships() {
    let manifest =
        load_pipeline_manifest(&login_dir()).unwrap_or_else(|e| panic!("load manifest: {e}"));
    let names: Vec<String> = load_watch_defs(&login_dir())
        .unwrap_or_else(|e| panic!("load watch defs: {e}"))
        .into_iter()
        .map(|d| d.name)
        .collect();
    for name in &manifest.watch {
        assert!(
            names.contains(name),
            "manifest watch name '{name}' must have a def in watch/ (got {names:?})"
        );
    }
    assert!(
        manifest.watch.iter().any(|n| n == "LoginFieldTownBarPc"),
        "login manifest must list the arrival sentinel, got {:?}",
        manifest.watch
    );
}

#[test]
#[allow(clippy::panic)]
fn arrival_sentinel_is_not_part_of_the_chain_graph() {
    let chain: Vec<String> = anaden_vision::load_pipeline(&login_dir())
        .unwrap_or_else(|e| panic!("load login chain: {e}"))
        .into_iter()
        .map(|d| d.name)
        .collect();
    assert!(
        !chain.contains(&"LoginFieldTownBarPc".to_string()),
        "a chain-visible sentinel would break the walkable-graph contract: {chain:?}"
    );
}
