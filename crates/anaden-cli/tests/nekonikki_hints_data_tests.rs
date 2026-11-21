#![allow(clippy::unwrap_used)]
#![allow(clippy::expect_used)]
#![allow(clippy::panic)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Deserialize;

const HINT_COUNT: usize = 73;
const ERAS: &[&str] = &["現代", "古代", "未来", "冥峡界"];
const CONTINENTS: &[&str] = &["ガルレア大陸", "ミグレイナ大陸"];

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

fn hints_path() -> PathBuf {
    workspace_root().join("data").join("nekonikki_hints.toml")
}

fn nav_routes_path() -> PathBuf {
    workspace_root().join("data").join("nav_routes.toml")
}

#[derive(Deserialize)]
struct HintsFile {
    version: u32,
    #[serde(default)]
    source: String,
    #[serde(default)]
    hint: Vec<HintEntry>,
}

#[derive(Deserialize)]
struct HintEntry {
    hint_id: u32,
    hint_text: String,
    era: String,
    continent: String,
    area_name: String,
    area_sub: String,
    template_status: String,
    nav_route_ref: String,
}

#[derive(Deserialize)]
struct NavRoutes {
    version: u32,
    #[serde(default)]
    route: Vec<NavRoute>,
}

#[derive(Deserialize)]
struct NavRoute {
    id: String,
    hint_id: u32,
    era: String,
    continent: String,
    area_name: String,
    area_sub: String,
    route_status: String,
}

fn load_hints() -> HintsFile {
    let raw = std::fs::read_to_string(hints_path())
        .unwrap_or_else(|e| panic!("data/nekonikki_hints.toml must be readable: {e}"));
    toml::from_str(&raw).expect("data/nekonikki_hints.toml must parse into the hint schema")
}

fn load_nav_routes() -> NavRoutes {
    let raw = std::fs::read_to_string(nav_routes_path())
        .unwrap_or_else(|e| panic!("data/nav_routes.toml must be readable: {e}"));
    toml::from_str(&raw).expect("data/nav_routes.toml must parse into the route schema")
}

fn hint_index(hints: &HintsFile) -> BTreeMap<u32, &HintEntry> {
    hints.hint.iter().map(|h| (h.hint_id, h)).collect()
}

#[test]
fn nekonikki_hints_data_parses_all_73_entries_with_complete_unique_ids() {
    let hints = load_hints();
    assert!(hints.version >= 1, "hints data version must be at least 1");
    assert!(
        !hints.source.is_empty(),
        "hints data must record its source provenance"
    );
    assert_eq!(
        hints.hint.len(),
        HINT_COUNT,
        "design section 6 lists exactly {HINT_COUNT} hints"
    );
    let ids: Vec<u32> = hints.hint.iter().map(|h| h.hint_id).collect();
    let expected: Vec<u32> = (1..=73).collect();
    assert_eq!(
        ids, expected,
        "hint_id must be unique and cover 1..=73 in file order"
    );
}

#[test]
fn nekonikki_hints_data_required_fields_are_complete_and_valid() {
    let hints = load_hints();
    for h in &hints.hint {
        assert!(
            !h.hint_text.is_empty(),
            "hint {} must carry its hint_text",
            h.hint_id
        );
        assert!(
            ERAS.contains(&h.era.as_str()),
            "hint {} carries unknown era {}",
            h.hint_id,
            h.era
        );
        assert!(
            !h.area_name.is_empty(),
            "hint {} must carry its area_name",
            h.hint_id
        );
        if h.era == "冥峡界" {
            assert_eq!(
                h.continent, "",
                "hint {}: the 冥峡界 era has no continent column",
                h.hint_id
            );
        } else {
            assert!(
                CONTINENTS.contains(&h.continent.as_str()),
                "hint {} carries unknown continent {}",
                h.hint_id,
                h.continent
            );
        }
        assert!(
            matches!(h.template_status.as_str(), "pending" | "collected"),
            "hint {} carries unknown template_status {}",
            h.hint_id,
            h.template_status
        );
        if !h.nav_route_ref.is_empty() {
            assert_eq!(
                h.nav_route_ref,
                format!("r{:02}", h.hint_id),
                "hint {} nav_route_ref must follow the rNN id alignment used by nav_routes",
                h.hint_id
            );
        }
    }
}

#[test]
fn nekonikki_hints_data_nav_route_refs_resolve_symmetrically_into_nav_routes() {
    let hints = load_hints();
    let routes = load_nav_routes();
    assert!(
        routes.version >= 1,
        "nav_routes data version must be at least 1"
    );
    assert!(
        !routes.route.is_empty(),
        "nav_routes must enumerate the design section 6 representative routes"
    );

    let mut route_by_id: BTreeMap<&str, &NavRoute> = BTreeMap::new();
    for r in &routes.route {
        let fresh = route_by_id.insert(r.id.as_str(), r).is_none();
        assert!(fresh, "nav route id {} must be unique", r.id);
        assert_eq!(
            r.id,
            format!("r{:02}", r.hint_id),
            "nav route {} must stay id-aligned with its own hint_id {}",
            r.id,
            r.hint_id
        );
        assert!(
            matches!(r.route_status.as_str(), "pending" | "wired"),
            "nav route {} carries unknown route_status {}",
            r.id,
            r.route_status
        );
    }

    for h in &hints.hint {
        if h.nav_route_ref.is_empty() {
            continue;
        }
        let route = route_by_id
            .get(h.nav_route_ref.as_str())
            .copied()
            .unwrap_or_else(|| {
                panic!(
                    "hint {} references nav route {} but nav_routes.toml defines no such route",
                    h.hint_id, h.nav_route_ref
                )
            });
        assert_eq!(
            route.hint_id, h.hint_id,
            "nav route {} must back-reference hint {}",
            route.id, h.hint_id
        );
        assert_eq!(
            route.era, h.era,
            "hint {} vs route {} era drift",
            h.hint_id, route.id
        );
        assert_eq!(
            route.continent, h.continent,
            "hint {} vs route {} continent drift",
            h.hint_id, route.id
        );
        assert_eq!(
            route.area_name, h.area_name,
            "hint {} vs route {} area_name drift",
            h.hint_id, route.id
        );
        assert_eq!(
            route.area_sub, h.area_sub,
            "hint {} vs route {} area_sub drift",
            h.hint_id, route.id
        );
    }

    let index = hint_index(&hints);
    for r in &routes.route {
        let hint = index.get(&r.hint_id).copied().unwrap_or_else(|| {
            panic!(
                "route {} names hint {} but the hints file has no such entry",
                r.id, r.hint_id
            )
        });
        assert_eq!(
            hint.nav_route_ref, r.id,
            "route {} must be referenced by its own hint {} (symmetric resolution)",
            r.id, r.hint_id
        );
    }
}

#[test]
fn nekonikki_hints_data_single_collected_entry_matches_miparu_anchor_inventory() {
    let hints = load_hints();
    let collected: Vec<u32> = hints
        .hint
        .iter()
        .filter(|h| h.template_status == "collected")
        .map(|h| h.hint_id)
        .collect();
    assert_eq!(
        collected.len(),
        1,
        "exactly one hint anchor is collected so far; update this test in the same PR \
         as the next anchor collection"
    );
    let hint_id = collected
        .first()
        .copied()
        .expect("the single collected hint id");
    assert_eq!(
        hint_id, 1,
        "the collected hint is the miparu anchor captured from diary row No.01"
    );
    let entry = hints
        .hint
        .iter()
        .find(|h| h.hint_id == hint_id)
        .expect("the collected hint entry must exist");
    assert_eq!(
        entry.nav_route_ref, "r01",
        "the collected hint must route through the wired modern-era nav chain entrance"
    );
    let anchor = workspace_root()
        .join("templates")
        .join("scenes")
        .join("neko_pc")
        .join("hint_text_miparu.png");
    assert!(
        anchor.is_file(),
        "collected hint {} must have its template png on disk at {}",
        hint_id,
        anchor.display()
    );
}
