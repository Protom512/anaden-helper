//! シナリオ編集状態のバリデーション層 (Issue #160 UC-4 / Issue #173 で
//! scenario_editor.rs から分割)。
//!
//! [`ScenarioEditorState`] の検査 (保存前 validate / フォーム全件一覧表示) と
//! そのエラー型 [`ScenarioValidationError`]・ROI 検証基準の画面寸法
//! ([`SCREEN_WIDTH`]/[`SCREEN_HEIGHT`]) を担う。検査メソッド
//! (`validation_issues` / `validate`) は [`crate::scenario_state::ScenarioEditorState`]
//! への追加分離 impl として本モジュールに置く (状態操作は
//! [`crate::scenario_state`]、保存は [`crate::scenario_save`])。

use std::path::Path;

use anaden_core::GoalError;

use crate::scenario_state::ScenarioEditorState;

/// ROI 検証基準の画面寸法 (raw-1258x708 PC キャプチャ空間)。
/// pipeline.rs テストの `assert_roi_within_1258x708` と同一契約。
pub const SCREEN_WIDTH: u32 = 1258;
pub const SCREEN_HEIGHT: u32 = 708;

/// シナリオ編集状態のバリデーションエラー。
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ScenarioValidationError {
    /// シナリオ名が空 (保存先ディレクトリ名になれない)。
    #[error("scenario name must not be empty")]
    EmptyName,
    /// シナリオ名が保存先ディレクトリ名として不適 (パス区切り・`.`/`..` 等)。
    /// `templates/pipelines/<name>/` の `<name>` は単一パス要素でなければならない。
    #[error("scenario name `{name}` is not a safe directory name")]
    UnsafeName {
        /// 不適だったシナリオ名。
        name: String,
    },
    /// TaskDef が 1 つもない (manifest 単独では実行不能)。
    #[error("scenario must contain at least 1 task")]
    NoTasks,
    /// start_task が未設定。
    #[error("start_task must be set")]
    EmptyStartTask,
    /// start_task が TaskDef 名前空間に存在しない。
    #[error("start_task `{task}` does not match any TaskDef name")]
    UnknownStartTask {
        /// 不一致だった start_task 名。
        task: String,
    },
    /// TaskDef 名の重複 (名前空間が一意でないと lookup が曖昧になる)。
    #[error("duplicate task name `{name}`")]
    DuplicateTaskName {
        /// 重複していたタスク名。
        name: String,
    },
    /// next 参照が TaskDef 名前空間に存在しない。
    #[error("task `{task}`: next reference `{next}` does not match any TaskDef name")]
    UnresolvedNext {
        /// 参照元タスク名。
        task: String,
        /// 解決不能だった next 参照先。
        next: String,
    },
    /// Goal の不変量違反 (`Goal::validate` の委譲結果)。
    #[error("goal[{index}] `{goal_name}` invalid: {source}")]
    GoalInvalid {
        /// `goals` 内のインデックス。
        index: usize,
        /// ゴール名。
        goal_name: String,
        /// 委譲先 (`Goal::validate`) のエラー。
        #[source]
        source: GoalError,
    },
    /// ROI が画面外にはみ出す、または幅/高さが 0。
    #[error(
        "task `{task}`: roi {roi:?} exceeds screen {SCREEN_WIDTH}x{SCREEN_HEIGHT} or has zero size"
    )]
    RoiOutOfBounds {
        /// 対象タスク名。
        task: String,
        /// はみ出し/ゼロサイズだった ROI `[x, y, w, h]`。
        roi: [u32; 4],
    },
    /// 新規・リネーム TaskDef 名が MAA/MDA pipeline 命名規約 (PascalCase 強制・
    /// 連番禁止・過汎用名禁止) に違反 (Issue #160 UC-4 / 設計ノート C)。
    /// ロード済み既存名
    /// ([`crate::scenario_state::ScenarioEditorState::loaded_task_names`]) は対象外
    /// (変更しない限り警告しない)。
    #[error("task name `{name}` violates the pipeline node naming convention: {reason}")]
    TaskNaming {
        /// 違反したタスク名。
        name: String,
        /// 違反理由 ([`crate::scenario_load::task_name_issue`] 由来)。
        reason: String,
    },
}

impl ScenarioEditorState {
    /// 全バリデーション問題を収集して返す (フォームで全件一覧表示する用途)。
    ///
    /// 検査内容: シナリオ名非空・ディレクトリ名として安全・TaskDef 1 件以上・
    /// タスク名一意・start_task が TaskDef 名前空間に存在・next 参照が解決可能・
    /// 各 Goal の [`anaden_core::Goal::validate`] 委譲・ROI が
    /// [`SCREEN_WIDTH`]x[`SCREEN_HEIGHT`] 画面内で有効サイズ・新規/リネーム
    /// TaskDef 名が命名規約適合 ([`crate::scenario_load::task_name_issue`])。
    ///
    /// **UC-4 baseline ゲーティング**: 命名規約検査と ROI 画面内検査は
    /// [`Self::loaded_task_names`] に含まれない名前 (= 新規追加・リネーム後)
    /// にのみ適用する。既存 pipeline のタスクは (a) 名前は命名規約導入前の
    /// 資産である可能性、(b) 20:9 pipeline の ROI は 1280 基準座標系で
    /// PC 1258x708 空間の契約外 (例: `field_loop/tap_hud_tr.toml` の
    /// `[1080,150,180,150]` は x+w=1260)、の理由で作成時検査の対象外とし、
    /// 編集なき保存 (ロスレス往復) を妨げない。リネームすると両検査が再有効
    /// になる (新規名は命名規約に、ROI は保存先座標系契約に従うべき)。
    /// 出力順は決定論的 (名前 → TaskDef 存在 → 重複 → start_task → タスク毎 →
    /// ゴール毎)。
    #[must_use]
    pub fn validation_issues(&self) -> Vec<ScenarioValidationError> {
        let mut issues = Vec::new();

        if self.name.trim().is_empty() {
            issues.push(ScenarioValidationError::EmptyName);
        }
        // 保存先ディレクトリ名として安全か。file_name() が名前全体と一致すれば
        // パス区切りを含まない単一要素 (`.`/`..`/末尾区切りは不一致になる)。
        let name = self.name.trim();
        if !name.is_empty() && Path::new(name).file_name() != Some(std::ffi::OsStr::new(name)) {
            issues.push(ScenarioValidationError::UnsafeName {
                name: self.name.clone(),
            });
        }
        if self.tasks.is_empty() {
            issues.push(ScenarioValidationError::NoTasks);
        }

        // タスク名一意性 (名前空間整合)。二件目以降の出現を報告する。
        for (i, task) in self.tasks.iter().enumerate() {
            if self.tasks[..i].iter().any(|t| t.name == task.name) {
                issues.push(ScenarioValidationError::DuplicateTaskName {
                    name: task.name.clone(),
                });
            }
        }

        if self.start_task.trim().is_empty() {
            issues.push(ScenarioValidationError::EmptyStartTask);
        } else if !self.tasks.iter().any(|t| t.name == self.start_task) {
            issues.push(ScenarioValidationError::UnknownStartTask {
                task: self.start_task.clone(),
            });
        }

        for task in &self.tasks {
            let is_loaded = self.loaded_task_names.iter().any(|n| n == &task.name);
            if !is_loaded && let Some(reason) = crate::scenario_load::task_name_issue(&task.name) {
                issues.push(ScenarioValidationError::TaskNaming {
                    name: task.name.clone(),
                    reason: reason.to_string(),
                });
            }
            if let Some(nexts) = &task.next {
                for next in nexts {
                    if !self.tasks.iter().any(|t| &t.name == next) {
                        issues.push(ScenarioValidationError::UnresolvedNext {
                            task: task.name.clone(),
                            next: next.clone(),
                        });
                    }
                }
            }
            if let Some(roi) = task.roi
                && !is_loaded
                && !roi_within_screen(roi)
            {
                issues.push(ScenarioValidationError::RoiOutOfBounds {
                    task: task.name.clone(),
                    roi,
                });
            }
        }

        for (index, goal) in self.goals.iter().enumerate() {
            if let Err(source) = goal.validate() {
                issues.push(ScenarioValidationError::GoalInvalid {
                    index,
                    goal_name: goal.name.clone(),
                    source,
                });
            }
        }

        issues
    }

    /// 最初のバリデーション問題を返す ([`anaden_core::Goal::validate`] と同じ契約)。
    ///
    /// # Errors
    /// 1 つでも問題があればその最初の [`ScenarioValidationError`]。
    pub fn validate(&self) -> Result<(), ScenarioValidationError> {
        match self.validation_issues().into_iter().next() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}

/// ROI `[x, y, w, h]` が画面内で有効サイズか (pipeline.rs テストと同一契約)。
fn roi_within_screen(roi: [u32; 4]) -> bool {
    let [x, y, w, h] = roi;
    w > 0 && h > 0 && x.saturating_add(w) <= SCREEN_WIDTH && y.saturating_add(h) <= SCREEN_HEIGHT
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use anaden_core::{Goal, StopCondition};
    use anaden_vision::{Action, Algorithm};
    use std::path::PathBuf;

    /// テスト用 TaskDef (roi=[10,20,100,50]・threshold=0.8・click_self)。
    fn task_def(name: &str, next: Option<Vec<&str>>) -> anaden_vision::TaskDef {
        anaden_vision::TaskDef {
            name: name.to_string(),
            state: "Field".to_string(),
            algorithm: Algorithm::Ccoeff,
            template: PathBuf::from(format!("{}.png", name.to_lowercase())),
            roi: Some([10, 20, 100, 50]),
            threshold: 0.8,
            base: None,
            action: Some(Action::ClickSelf),
            next: next.map(|v| v.iter().map(|s| s.to_string()).collect()),
        }
    }

    fn loop_goal(name: &str, target: u64) -> Goal {
        Goal {
            name: name.to_string(),
            stop: StopCondition::LoopCount { target },
        }
    }

    #[test]
    fn validate_ok_for_complete_scenario() {
        let mut st = ScenarioEditorState::new("fishing2");
        st.add_task(task_def("Start", Some(vec!["Loop"])));
        // roi: None (= 全面) も有効。
        st.add_task(anaden_vision::TaskDef {
            roi: None,
            ..task_def("Loop", Some(vec!["Start"]))
        });
        st.add_goal(loop_goal("loop50", 50));
        st.add_goal(Goal {
            name: "any".to_string(),
            stop: StopCondition::Any {
                conditions: vec![StopCondition::Timeout { secs: 3600 }],
            },
        });
        assert!(st.validation_issues().is_empty());
        assert!(st.validate().is_ok());
    }

    #[test]
    fn validate_flags_empty_name_no_tasks_empty_start() {
        let st = ScenarioEditorState::new("   ");
        let issues = st.validation_issues();
        assert!(issues.contains(&ScenarioValidationError::EmptyName));
        assert!(issues.contains(&ScenarioValidationError::NoTasks));
        assert!(issues.contains(&ScenarioValidationError::EmptyStartTask));
    }

    #[test]
    fn validate_flags_unsafe_directory_name() {
        // パス区切り・`.`/`..` は保存先ディレクトリ名として不適。
        for bad in ["a/b", "..", ".", "a\\b", "abc/"] {
            let mut st = ScenarioEditorState::new(bad);
            st.add_task(task_def("A", None));
            assert!(
                st.validation_issues()
                    .contains(&ScenarioValidationError::UnsafeName {
                        name: bad.to_string()
                    }),
                "name {bad:?} must be flagged unsafe"
            );
        }
        // 通常の名前は UnsafeName を出さない。
        let mut ok = ScenarioEditorState::new("my_scenario");
        ok.add_task(task_def("A", None));
        assert!(
            !ok.validation_issues()
                .iter()
                .any(|i| matches!(i, ScenarioValidationError::UnsafeName { .. }))
        );
    }

    #[test]
    fn validate_flags_unknown_start_task() {
        let mut st = ScenarioEditorState::new("s");
        st.add_task(task_def("A", None));
        st.start_task = "Ghost".to_string();
        let issues = st.validation_issues();
        assert!(issues.contains(&ScenarioValidationError::UnknownStartTask {
            task: "Ghost".to_string()
        }));
    }

    #[test]
    fn validate_flags_unresolved_next_reference() {
        let mut st = ScenarioEditorState::new("s");
        st.add_task(task_def("Start", Some(vec!["Missing"])));
        let issues = st.validation_issues();
        assert!(issues.contains(&ScenarioValidationError::UnresolvedNext {
            task: "Start".to_string(),
            next: "Missing".to_string()
        }));
    }

    #[test]
    fn validate_flags_duplicate_task_names() {
        let mut st = ScenarioEditorState::new("s");
        st.add_task(task_def("A", None));
        st.add_task(task_def("A", None));
        let issues = st.validation_issues();
        assert!(
            issues.contains(&ScenarioValidationError::DuplicateTaskName {
                name: "A".to_string()
            })
        );
    }

    #[test]
    fn validate_delegates_to_goal_validate() {
        let mut st = ScenarioEditorState::new("s");
        st.add_task(task_def("A", None));
        st.add_goal(loop_goal("bad", 0));
        let issues = st.validation_issues();
        let expected = ScenarioValidationError::GoalInvalid {
            index: 0,
            goal_name: "bad".to_string(),
            source: GoalError::NonPositive { field: "target" },
        };
        assert!(issues.contains(&expected));
    }

    #[test]
    fn validate_flags_roi_out_of_bounds_and_zero_size() {
        let mut st = ScenarioEditorState::new("s");
        let mut edge = task_def("Edge", None);
        edge.roi = Some([1200, 600, 200, 200]);
        st.add_task(edge);
        let mut zero = task_def("Zero", None);
        zero.roi = Some([10, 10, 0, 40]);
        st.add_task(zero);
        let issues = st.validation_issues();
        assert!(issues.contains(&ScenarioValidationError::RoiOutOfBounds {
            task: "Edge".to_string(),
            roi: [1200, 600, 200, 200]
        }));
        assert!(issues.contains(&ScenarioValidationError::RoiOutOfBounds {
            task: "Zero".to_string(),
            roi: [10, 10, 0, 40]
        }));
    }

    // ---- UC-4 (Shard 5): baseline ゲーティング (既存 pipeline ロード編集は
    // scenario_panel のテストで担保) ----

    /// baseline (loaded_task_names) 外の名前 (= 新規追加・リネーム後) のみ
    /// 命名規約・ROI 画面内検査の対象。既存名は変更しない限り警告しない。
    #[test]
    fn naming_and_roi_checks_apply_only_to_non_baseline_tasks() {
        // 20:9 pipeline 相当: ROI [1080,150,180,150] は x+w=1260 > 1258
        // (1280 基準座標系 = PC 1258x708 契約外)。
        let mut st = ScenarioEditorState::new("field_loop");
        let mut loaded = task_def("TapHudTr", None);
        loaded.roi = Some([1080, 150, 180, 150]);
        st.add_task(loaded);
        st.loaded_task_names = vec!["TapHudTr".to_string()];
        assert!(
            st.validate().is_ok(),
            "baseline タスクは命名・ROI 検査の対象外: {:?}",
            st.validation_issues()
        );

        // リネーム (baseline 外の新名) は命名規約で拒否。
        st.task_mut("TapHudTr").unwrap().name = "tap_hud_v2".to_string();
        assert!(
            st.validation_issues()
                .iter()
                .any(|i| matches!(i, ScenarioValidationError::TaskNaming { .. }))
        );

        // PascalCase へ直しても baseline 外なので ROI 検査が再有効。
        st.task_mut("tap_hud_v2").unwrap().name = "FieldHudWidePc".to_string();
        assert!(
            st.validation_issues()
                .iter()
                .any(|i| matches!(i, ScenarioValidationError::RoiOutOfBounds { .. })),
            "baseline 外の ROI 超過は検出される"
        );
    }

    /// baseline 無し (新規シナリオ) は全タスクが命名規約の対象。
    #[test]
    fn new_scenario_names_are_all_validated() {
        let mut st = ScenarioEditorState::new("s");
        st.add_task(task_def("Start", None)); // PascalCase: OK
        st.add_task(task_def("confirm_now", None)); // snake_case: 拒否
        let issues = st.validation_issues();
        assert!(!issues.iter().any(|i| matches!(
            i,
            ScenarioValidationError::TaskNaming { name, .. } if name == "Start"
        )));
        assert!(
            issues.contains(&ScenarioValidationError::TaskNaming {
                name: "confirm_now".to_string(),
                reason: crate::scenario_load::task_name_issue("confirm_now")
                    .unwrap_or_default()
                    .to_string(),
            })
        );
    }
}
