use serde::{Deserialize, Serialize};

const BUILTIN_MAX_ITERS: u64 = 10_000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StrategyOptionDef {
    pub id: String,
    pub label: String,
    pub default: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StrategyDef {
    pub id: String,
    pub label: String,
    pub options: Vec<StrategyOptionDef>,
    pub pipeline_dir: String,
    pub start_task: String,
    pub algorithm: String,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub max_iters: Option<u64>,
}

impl StrategyDef {
    #[must_use]
    pub fn to_run_args(&self) -> Vec<String> {
        let mut args = vec![
            "run".to_string(),
            "--algorithm".to_string(),
            self.algorithm.clone(),
        ];
        if let Some(max_iters) = self.max_iters {
            args.push("--max-iters".to_string());
            args.push(max_iters.to_string());
        }
        args.push(self.pipeline_dir.clone());
        args.push(self.start_task.clone());
        args
    }
}

fn pipeline_def(
    id: &str,
    label: &str,
    pipeline_dir: &str,
    start_task: &str,
    algorithm: &str,
    target: Option<&str>,
) -> StrategyDef {
    StrategyDef {
        id: id.to_string(),
        label: label.to_string(),
        options: Vec::new(),
        pipeline_dir: pipeline_dir.to_string(),
        start_task: start_task.to_string(),
        algorithm: algorithm.to_string(),
        target: target.map(std::string::ToString::to_string),
        max_iters: Some(BUILTIN_MAX_ITERS),
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StrategyCatalog {
    strategies: Vec<StrategyDef>,
}

impl StrategyCatalog {
    #[must_use]
    pub fn builtin() -> Self {
        Self {
            strategies: vec![
                pipeline_def(
                    "field_loop_pc",
                    "フィールド周回（PC 16:9）",
                    "templates/pipelines/field_loop_pc",
                    "TapBottomStablePc",
                    "ccoeff",
                    Some("windows"),
                ),
                pipeline_def(
                    "nav_to_field_pc",
                    "フィールドへ遷移（PC 16:9 コールドスタート）",
                    "templates/pipelines/nav_to_field_pc",
                    "TapToStartPc",
                    "ccoeff",
                    Some("windows"),
                ),
            ],
        }
    }

    #[must_use]
    pub fn strategies(&self) -> &[StrategyDef] {
        &self.strategies
    }

    #[must_use]
    pub fn find(&self, id: &str) -> Option<&StrategyDef> {
        self.strategies.iter().find(|s| s.id == id)
    }

    #[must_use]
    pub fn default_selection(&self) -> StrategySelection {
        StrategySelection::from_defaults(self)
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct StrategySelection {
    pub strategy: Option<String>,
    #[serde(default)]
    pub options: std::collections::BTreeMap<String, bool>,
}

impl StrategySelection {
    #[must_use]
    pub fn from_defaults(catalog: &StrategyCatalog) -> Self {
        let mut options = std::collections::BTreeMap::new();
        for s in &catalog.strategies {
            for o in &s.options {
                options.insert(format!("{}.{}", s.id, o.id), o.default);
            }
        }
        Self {
            strategy: None,
            options,
        }
    }

    #[must_use]
    pub fn option(&self, strategy_id: &str, option_id: &str) -> Option<bool> {
        self.options
            .get(&format!("{strategy_id}.{option_id}"))
            .copied()
    }

    pub fn set_option(&mut self, strategy_id: &str, option_id: &str, value: bool) {
        self.options
            .insert(format!("{strategy_id}.{option_id}"), value);
    }

    pub fn validate(&self, catalog: &StrategyCatalog) -> Result<(), SelectionError> {
        if let Some(id) = &self.strategy
            && catalog.find(id).is_none()
        {
            return Err(SelectionError::UnknownStrategy { id: id.clone() });
        }
        for key in self.options.keys() {
            if !is_known_option_key(catalog, key) {
                return Err(SelectionError::UnknownOption { key: key.clone() });
            }
        }
        Ok(())
    }

    pub fn retain_known_options(&mut self, catalog: &StrategyCatalog) {
        self.options
            .retain(|key, _| is_known_option_key(catalog, key));
    }
}

fn is_known_option_key(catalog: &StrategyCatalog, key: &str) -> bool {
    let Some((strategy_id, option_id)) = key.split_once('.') else {
        return false;
    };
    catalog
        .find(strategy_id)
        .is_some_and(|def| def.options.iter().any(|o| o.id == option_id))
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum SelectionError {
    #[error("unknown strategy: {id}")]
    UnknownStrategy { id: String },
    #[error("unknown option key: {key}")]
    UnknownOption { key: String },
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    const EXPECTED_IDS: [&str; 2] = ["field_loop_pc", "nav_to_field_pc"];

    #[test]
    fn builtin_catalog_contains_exactly_two_real_pc_pipelines() {
        let catalog = StrategyCatalog::builtin();
        let ids: Vec<&str> = catalog.strategies().iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, EXPECTED_IDS, "catalog ids: {ids:?}");
    }

    #[test]
    fn builtin_catalog_does_not_contain_nonexistent_fishing() {
        let catalog = StrategyCatalog::builtin();
        assert!(
            catalog.find("fishing").is_none(),
            "fishing はカタログ外 (templates/tasks のタスクバンク経由) であること"
        );
    }

    #[test]
    fn every_strategy_has_pipeline_dir_start_task_and_valid_algorithm() {
        let catalog = StrategyCatalog::builtin();
        for s in catalog.strategies() {
            assert!(!s.pipeline_dir.is_empty(), "{}: pipeline_dir 未設定", s.id);
            assert!(
                s.pipeline_dir.contains(&s.id),
                "{}: pipeline_dir に id が含まれること ({}),",
                s.id,
                s.pipeline_dir
            );
            assert!(!s.start_task.is_empty(), "{}: start_task 未設定", s.id);
            assert!(
                s.algorithm == "sse" || s.algorithm == "ccoeff",
                "{}: algorithm は sse|ccoeff のみ (actual: {})",
                s.id,
                s.algorithm
            );
        }
    }

    #[test]
    fn catalog_pipeline_dirs_actually_exist_in_repo() {
        let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let repo_root = manifest_dir.ancestors().nth(2).unwrap();
        let catalog = StrategyCatalog::builtin();
        for s in catalog.strategies() {
            let dir = repo_root.join(&s.pipeline_dir);
            assert!(
                dir.is_dir(),
                "{}: pipeline_dir が実在しない: {}",
                s.id,
                s.pipeline_dir
            );
        }
    }

    #[test]
    fn all_strategies_carry_windows_target() {
        let catalog = StrategyCatalog::builtin();
        for s in catalog.strategies() {
            assert_eq!(
                s.target.as_deref(),
                Some("windows"),
                "{}: target 設定が想定と異なる (actual: {:?})",
                s.id,
                s.target
            );
        }
    }

    #[test]
    fn to_run_args_builds_catalog_driven_argument_sequence() {
        let catalog = StrategyCatalog::builtin();
        let nav_pc = catalog.find("nav_to_field_pc").unwrap();
        assert_eq!(
            nav_pc.to_run_args(),
            vec![
                "run",
                "--algorithm",
                "ccoeff",
                "--max-iters",
                "10000",
                "templates/pipelines/nav_to_field_pc",
                "TapToStartPc",
            ]
        );
        let field = catalog.find("field_loop_pc").unwrap();
        assert_eq!(
            field.to_run_args(),
            vec![
                "run",
                "--algorithm",
                "ccoeff",
                "--max-iters",
                "10000",
                "templates/pipelines/field_loop_pc",
                "TapBottomStablePc",
            ]
        );
    }

    #[test]
    fn to_run_args_includes_explicit_max_iters_when_set() {
        let mut def = StrategyCatalog::builtin()
            .find("field_loop_pc")
            .unwrap()
            .clone();
        def.max_iters = Some(500);
        assert_eq!(
            def.to_run_args(),
            vec![
                "run",
                "--algorithm",
                "ccoeff",
                "--max-iters",
                "500",
                "templates/pipelines/field_loop_pc",
                "TapBottomStablePc",
            ]
        );
    }

    #[test]
    fn to_run_args_omits_max_iters_flag_when_none() {
        let mut def = StrategyCatalog::builtin()
            .find("field_loop_pc")
            .unwrap()
            .clone();
        def.max_iters = None;
        assert!(
            !def.to_run_args().iter().any(|a| a == "--max-iters"),
            "args: {:?}",
            def.to_run_args()
        );
    }

    #[test]
    fn builtin_strategies_carry_explicit_large_max_iters() {
        for s in StrategyCatalog::builtin().strategies() {
            let n = s
                .max_iters
                .unwrap_or_else(|| panic!("{}: max_iters must be set explicitly", s.id));
            assert!(
                n > 100,
                "{}: max_iters={n} must exceed the CLI default of 100",
                s.id
            );
        }
    }

    #[test]
    fn find_unknown_strategy_is_none() {
        let catalog = StrategyCatalog::builtin();
        assert!(catalog.find("nonexistent").is_none());
    }

    #[test]
    fn default_selection_has_no_strategy_and_empty_options() {
        let catalog = StrategyCatalog::builtin();
        let sel = catalog.default_selection();
        assert_eq!(sel.strategy, None);
        assert!(
            sel.options.is_empty(),
            "実在 2 パイプラインには ON/OFF オプションが無い"
        );
    }

    #[test]
    fn set_and_get_option_roundtrip() {
        let mut sel = StrategySelection::default();
        sel.set_option("field_loop_pc", "some_option", true);
        assert_eq!(sel.option("field_loop_pc", "some_option"), Some(true));
        assert_eq!(sel.option("field_loop_pc", "other"), None);
    }

    #[test]
    fn selection_toml_roundtrip_keeps_compatibility() {
        let mut sel = StrategySelection::from_defaults(&StrategyCatalog::builtin());
        sel.strategy = Some("field_loop_pc".to_string());
        sel.set_option("field_loop_pc", "demo", false);

        let toml_str = toml::to_string(&sel).expect("serialize");
        assert!(toml_str.contains(r#"strategy = "field_loop_pc""#));

        let parsed: StrategySelection = toml::from_str(&toml_str).expect("deserialize");
        assert_eq!(parsed, sel);
    }

    #[test]
    fn selection_parses_unknown_strategy_without_error() {
        let toml_str = r#"strategy = "fishing""#;
        let parsed: StrategySelection = toml::from_str(toml_str).expect("deserialize");
        assert_eq!(parsed.strategy.as_deref(), Some("fishing"));
    }

    #[test]
    fn validate_rejects_unknown_strategy() {
        let catalog = StrategyCatalog::builtin();
        let mut sel = StrategySelection {
            strategy: Some("bogus".to_string()),
            ..Default::default()
        };
        assert!(matches!(
            sel.validate(&catalog),
            Err(SelectionError::UnknownStrategy { .. })
        ));

        sel = StrategySelection {
            strategy: Some("fishing".to_string()),
            ..Default::default()
        };
        assert!(matches!(
            sel.validate(&catalog),
            Err(SelectionError::UnknownStrategy { .. })
        ));

        sel = StrategySelection {
            strategy: Some("field_loop_pc".to_string()),
            ..Default::default()
        };
        assert!(sel.validate(&catalog).is_ok());
        sel = StrategySelection::default();
        assert!(sel.validate(&catalog).is_ok());
    }

    fn catalog_with_one_option() -> StrategyCatalog {
        let mut def = pipeline_def(
            "field_loop_pc",
            "ラベル",
            "templates/pipelines/field_loop_pc",
            "TapBottomStablePc",
            "ccoeff",
            Some("windows"),
        );
        def.options.push(StrategyOptionDef {
            id: "skip_first".to_string(),
            label: "最初の1回をスキップ".to_string(),
            default: false,
        });
        StrategyCatalog {
            strategies: vec![def],
        }
    }

    #[test]
    fn validate_accepts_defined_option_key() {
        let catalog = catalog_with_one_option();
        let mut sel = StrategySelection::default();
        sel.set_option("field_loop_pc", "skip_first", true);
        assert!(sel.validate(&catalog).is_ok());
    }

    #[test]
    fn validate_rejects_undefined_option_key() {
        let catalog = catalog_with_one_option();
        let mut sel = StrategySelection::default();
        sel.set_option("field_loop_pc", "not_defined", true);
        assert!(matches!(
            sel.validate(&catalog),
            Err(SelectionError::UnknownOption { .. })
        ));
    }

    #[test]
    fn validate_rejects_option_key_of_removed_strategy() {
        let catalog = StrategyCatalog::builtin();
        let mut sel = StrategySelection::default();
        sel.set_option("field_loop_pc", "skip_first", true);
        assert!(matches!(
            sel.validate(&catalog),
            Err(SelectionError::UnknownOption { .. })
        ));
    }

    #[test]
    fn retain_known_options_drops_stale_and_keeps_defined() {
        let catalog = catalog_with_one_option();
        let mut sel = StrategySelection::default();
        sel.set_option("field_loop_pc", "skip_first", true);
        sel.set_option("nav_to_field_pc", "stale", false);
        sel.options.insert("no_dot".to_string(), true);
        sel.retain_known_options(&catalog);
        assert_eq!(sel.option("field_loop_pc", "skip_first"), Some(true));
        assert!(!sel.options.contains_key("nav_to_field_pc.stale"));
        assert!(!sel.options.contains_key("no_dot"));
    }
}
