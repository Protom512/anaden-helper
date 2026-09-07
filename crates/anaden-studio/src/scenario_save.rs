//! シナリオ保存フロー (Issue #160 Shard 3 / T3: UC-1・Issue #173 で
//! scenario_editor.rs から分割)。
//!
//! 検証済み [`ScenarioEditorState`] を `anaden-vision` の保存ヘルパー
//! (`save_pipeline_manifest` / `save_task_def`) で `<pipelines_root>/<name>/`
//! へ書き出す純 IO 層。既存 pipeline ディレクトリへの無警告上書き防止
//! (所有権ガード) と、削除・リネーム済み旧 TaskDef ファイルの掃除を含む。
//! バリデーションは [`crate::scenario_validate`]、状態操作は
//! [`crate::scenario_state`] (egui パネルは scenario_panel.rs)。

use std::path::{Path, PathBuf};

use image::DynamicImage;

use crate::scenario_state::ScenarioEditorState;
use crate::scenario_validate::ScenarioValidationError;

/// シナリオ保存 (manifest + TaskDef 群 + ROI 由来テンプレート PNG) のエラー。
#[derive(Debug, thiserror::Error)]
pub enum ScenarioSaveError {
    /// エディタ状態のバリデーション不合格
    /// ([`crate::scenario_state::ScenarioEditorState::validate`])。
    #[error("scenario invalid: {0}")]
    Invalid(#[from] ScenarioValidationError),
    /// pipeline ディレクトリ作成失敗。
    #[error("pipeline dir create failed")]
    DirCreate(#[source] std::io::Error),
    /// ROI 追加タスクのテンプレート PNG 書き出し失敗。
    #[error("template PNG write failed")]
    PngWrite(#[source] image::ImageError),
    /// manifest / TaskDef TOML の保存失敗 (anaden-vision save ヘルパー)。
    #[error("pipeline save failed: {0}")]
    Vision(#[from] anaden_vision::TaskDefError),
    /// 保存先ディレクトリが既存だが、この編集状態の所有対象ではない
    /// (ロード元でも直近の保存先でもない)。1 バイトも書かない。
    ///
    /// Issue #160 レビュー M-1: 既存 pipeline への無警告上書き (新 TaskDef 群で
    /// 置換 + `sweep_removed_taskdefs` による旧 TaskDef 削除) を防ぐ fail-closed。
    /// task 登録経路の `TaskAlreadyExists` 拒否と対称。
    #[error(
        "pipeline dir already exists: {dir} (既存 pipeline の無警告上書きは禁止。ロードして編集するか別名を指定してください)"
    )]
    PipelineDirAlreadyExists {
        /// 拒否された保存先ディレクトリ。
        dir: PathBuf,
    },
}

/// 検証済みシナリオを `<pipelines_root>/<シナリオ名>/` へ保存する (UC-1 保存経路)。
///
/// 書き出し構成 (既存 pipeline ディレクトリと完全互換):
/// - `pipeline.toml` — manifest (start_task + goals)。[`anaden_vision::save_pipeline_manifest`]
/// - `<task>.toml` — 各 TaskDef。[`anaden_vision::save_task_def`]
/// - `<task>.png` — ROI から追加したタスクのテンプレート PNG 本体
///
/// `pngs` は「タスク追加時に確保した crop」のリストで、TaskDef 名前空間に残る
/// タスクのみ書き出す (削除済みタスクの crop は無視)。ROI 追加タスクの
/// `template` は追加時に `<name>.png` (pipeline dir 基準の裸相対) が設定済みの
/// ため、保存 TOML の template 参照と PNG 実体が一致する。
///
/// UC-4 (Shard 5): [`crate::scenario_state::ScenarioEditorState::loaded_task_files`]
/// に対応する (ロード済み) タスクは **元のファイル名 (stem)** へ書き戻す。既存
/// pipeline は stem ≠ name (`tap_bottom.toml` ↔ `TapBottomStable`) のため、
/// name で保存すると同一 TaskDef の重複ファイルができ再 load でタスクが倍化する。
/// 対応エントリの無い新規タスクは `<name>.toml` へ保存される。また、削除・
/// リネーム済みタスクの旧 TaskDef ファイルを保存成功後に掃除する
/// (`sweep_removed_taskdefs`) — 残ると `load_pipeline` が再読込時に旧タスクを
/// 復活させるため。
///
/// # Errors
/// - [`ScenarioSaveError::Invalid`]: バリデーション不合格 (1 バイトも書かない)。
/// - それ以外: 各書き出し段階の失敗 ([`ScenarioSaveError::DirCreate`] /
///   [`ScenarioSaveError::PngWrite`] / [`ScenarioSaveError::Vision`])。
///
/// 保存 → [`anaden_vision::load_pipeline`] / [`load_pipeline_manifest` 往復は
/// `tests/scenario_editor_tests.rs` (AC-1) と
/// `tests/scenario_uc4_roundtrip_tests.rs` (UC-4・既存 8 pipeline ロスレス往復)
/// で機械保証されている。
///
/// [`load_pipeline_manifest`]: anaden_vision::load_pipeline_manifest
pub fn save_scenario(
    state: &ScenarioEditorState,
    pngs: &[(String, DynamicImage)],
    pipelines_root: &Path,
) -> Result<PathBuf, ScenarioSaveError> {
    state.validate()?;
    let dir = pipelines_root.join(state.name.trim());
    // 既存 dir 上書きガード (レビュー M-1): 保存先 dir が既に存在する場合、
    // この編集状態の所有対象 (ロード元 / 直近の保存先 = loaded_from) と同一の
    // ときのみ書き込みを許可する。それ以外 (新規シナリオ名が既存 pipeline と
    // 衝突、ロード編集のリネーム先が別 pipeline と衝突) は 1 バイトも書かない。
    if dir.is_dir() && !is_owned_dir(state, &dir) {
        return Err(ScenarioSaveError::PipelineDirAlreadyExists { dir: dir.clone() });
    }
    std::fs::create_dir_all(&dir).map_err(ScenarioSaveError::DirCreate)?;
    for (name, img) in pngs {
        if !state.tasks.iter().any(|t| &t.name == name) {
            continue; // 削除済みタスクの crop は書かない
        }
        img.save(dir.join(format!("{name}.png")))
            .map_err(ScenarioSaveError::PngWrite)?;
    }
    anaden_vision::save_pipeline_manifest(&state.to_manifest(), &dir)?;
    // UC-4: ロード済みタスクは元ファイル (stem) へ、新規タスクは <name>.toml へ。
    let mut written: Vec<PathBuf> = Vec::new();
    for task in &state.tasks {
        let stem = state
            .loaded_task_files
            .iter()
            .find(|(n, _)| n == &task.name)
            .map(|(_, stem)| stem.as_str())
            .unwrap_or(task.name.as_str());
        let path = dir.join(format!("{stem}.toml"));
        anaden_vision::save_task_def(task, &path)?;
        written.push(path);
    }
    sweep_removed_taskdefs(&dir, &written);
    Ok(dir)
}

/// `dir` が編集状態の所有対象 (loaded_from) と同一か。
///
/// 比較は canonicalize 後のパスで行う (大文字小文字違い・区切り文字違いの
/// 同一ディレクトリ表現を同値扱い)。canonicalize 失敗時は素のパス比較へ
/// フォールバックする (不一致 = 拒否方向の fail-closed)。
fn is_owned_dir(state: &ScenarioEditorState, dir: &Path) -> bool {
    state.loaded_from.as_deref().is_some_and(|src| {
        match (std::fs::canonicalize(src), std::fs::canonicalize(dir)) {
            (Ok(a), Ok(b)) => a == b,
            _ => src == dir,
        }
    })
}

/// 今回の保存で書き出さなかった旧 TaskDef ファイル (削除・リネーム前) を
/// 掃除する (UC-4)。manifest 慣例ファイル (`pipeline.toml`) は対象外。
///
/// 保存した全 TaskDef の書き出しに成功した後にのみ呼ぶ (失敗時の部分状態で
/// 旧ファイルを消さない)。削除自体は best-effort — 失敗しても再 load 時に
/// 旧タスクが復活するだけでデータ破損にはならないためエラーにはしない。
fn sweep_removed_taskdefs(dir: &Path, written: &[PathBuf]) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let is_toml = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("toml"));
        let is_manifest = path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n == anaden_vision::PIPELINE_MANIFEST_FILENAME);
        if is_toml && !is_manifest && !written.contains(&path) {
            let _ = std::fs::remove_file(&path);
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use anaden_core::{Goal, StopCondition};
    use anaden_vision::{Action, Algorithm, TaskDef};
    use std::fs;

    /// テスト用 TaskDef (roi=[10,20,100,50]・threshold=0.8・click_self)。
    fn task_def(name: &str, next: Option<Vec<&str>>) -> TaskDef {
        TaskDef {
            name: name.to_string(),
            state: "Field".to_string(),
            algorithm: Algorithm::Ccoeff,
            template: std::path::PathBuf::from(format!("{}.png", name.to_lowercase())),
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

    // AC-1 機械保証: エディタ状態 -> anaden-vision save -> load 往復。
    #[test]
    fn saved_scenario_roundtrips_through_anaden_vision_load() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().join("my_pipeline");
        fs::create_dir_all(&dir).expect("mkdir");

        let mut st = ScenarioEditorState::new("my_pipeline");
        st.add_task(task_def("Start", Some(vec!["End"])));
        st.add_task(TaskDef {
            roi: None,
            ..task_def("End", Some(vec![]))
        });
        st.add_goal(loop_goal("loop3", 3));
        st.add_goal(Goal {
            name: "combo".to_string(),
            stop: StopCondition::Any {
                conditions: vec![
                    StopCondition::TemplateMatch {
                        task: "End".to_string(),
                        confidence: 0.85,
                    },
                    StopCondition::Timeout { secs: 600 },
                ],
            },
        });
        st.validate().expect("scenario must be valid");

        let manifest = st.to_manifest();
        anaden_vision::save_pipeline_manifest(&manifest, &dir).expect("save manifest");
        for t in &st.tasks {
            anaden_vision::save_task_def(t, &dir.join(format!("{}.toml", t.name)))
                .expect("save task");
        }

        let loaded_manifest = anaden_vision::load_pipeline_manifest(&dir).expect("load manifest");
        assert_eq!(loaded_manifest, manifest);
        let defs = anaden_vision::load_pipeline(&dir).expect("load tasks");
        assert_eq!(defs.len(), 2, "pipeline.toml (manifest) must be skipped");
        let start = defs.iter().find(|d| d.name == "Start").expect("Start");
        assert_eq!(
            start.next.as_deref(),
            Some(&["End".to_string()][..]),
            "next chain survives round-trip"
        );
        assert!(start.template.is_absolute());
        assert!(
            start.template.ends_with("start.png"),
            "relative template preserved: {:?}",
            start.template
        );
        let end = defs.iter().find(|d| d.name == "End").expect("End");
        assert_eq!(end.roi, None);
        assert_eq!(end.action, Some(Action::ClickSelf));
    }
}
