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
    /// 書き出し TaskDef ファイル (stem または name の .toml) が複数タスクで
    /// 衝突する。1 バイトも書かない。
    ///
    /// Issue #180 (minor-3): stem ≠ name のロード済みタスク (stem `<X>.toml`) と
    /// 新規タスク (name `<X>`) が同じファイルを指すと後書きが前者を上書きし、
    /// 再 load でタスクがサイレント消失する。保存前の事前検査で拒否する
    /// (検査はケースインセンシティブ — Windows FS では大文字小文字違いも
    /// 同一ファイル)。
    #[error(
        "task file path collision: {path} is the write target of tasks `{first}` and `{second}` (stem 衝突。stem ≠ name の既存タスクと衝突する名前は使えません)"
    )]
    TaskFileCollision {
        /// 衝突した書き出しファイルパス。
        path: PathBuf,
        /// 先に書き出すタスク (TaskDef name)。
        first: String,
        /// 同じパスを指すもう一方のタスク (TaskDef name)。
        second: String,
    },
}

/// [`save_scenario_with_warnings`] の保存結果 (Issue #184)。
///
/// `warnings` は無構造テンプレートへの fail-visible 警告。保存自体は
/// ブロックしていない (警告は保存成功後の付加情報)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScenarioSaveOutcome {
    /// 保存先 pipeline ディレクトリ。
    pub dir: PathBuf,
    /// 無構造テンプレート (輝度 stddev < [`anaden_vision::TEMPLATE_MIN_LUMA_STDDEV`])
    /// への警告文リスト (各要素は対象タスク名を接頭に持つ)。全テンプレートが
    /// 構造ありなら空 (偽陽性ゼロ)。
    pub warnings: Vec<String>,
}

/// 検証済みシナリオを `<pipelines_root>/<シナリオ名>/` へ保存する (UC-1 保存経路)。
///
/// [`save_scenario`] の警告付き版 (Issue #184): 書き出し構成・fail-closed 挙動は
/// 同一で、加えて保存しようとしているテンプレート PNG のうち無構造
/// (stddev < 閾値) のものへの警告を [`ScenarioSaveOutcome::warnings`] として返す
/// (pending crop と UC-2 参照の両方)。
///
/// # Errors
/// [`save_scenario`] と同一 (バリデーション不合格・所有権ガード・書き出し失敗。
/// いずれも 1 バイトも書かない / 部分状態で警告を返さない)。
pub fn save_scenario_with_warnings(
    state: &ScenarioEditorState,
    pngs: &[(String, DynamicImage)],
    pipelines_root: &Path,
) -> Result<ScenarioSaveOutcome, ScenarioSaveError> {
    state.validate()?;
    let dir = pipelines_root.join(state.name.trim());
    // UC-4: ロード済みタスクは元ファイル (stem) へ、新規タスクは <name>.toml へ。
    let out_paths = task_out_paths(state, &dir);
    // stem 衝突検査 (Issue #180 minor-3・fail-closed): 書き出しパスの一意性。
    // stem ≠ name のロード済みタスクと新規タスク名が同じファイルを指すと
    // 後書きが前者を上書きしてタスクがサイレント消失するため、1 バイトも
    // 書く前に拒否する (既存バリデーションはタスク *名* の一意性しか見ない
    // ため、この衝突は検出できない)。
    check_unique_out_paths(state, &out_paths)?;
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
    for (task, path) in state.tasks.iter().zip(&out_paths) {
        anaden_vision::save_task_def(task, path)?;
    }
    sweep_removed_taskdefs(&dir, &out_paths);
    // Issue #184: 無構造テンプレート警告 (fail-visible)。保存済みのためブロックは
    // しない — 呼出側 (ScenarioPanel) が status へ表示する。
    let warnings = unstructured_template_warnings(state, pngs, &dir);
    Ok(ScenarioSaveOutcome { dir, warnings })
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
/// 警告付き版 ([`save_scenario_with_warnings`]) の薄いラッパ (警告を破棄して
/// 保存先 dir のみ返す。既存呼出側の互換維持)。GUI 保存経路は警告表示のため
/// [`save_scenario_with_warnings`] を使う (Issue #184)。
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
    save_scenario_with_warnings(state, pngs, pipelines_root).map(|outcome| outcome.dir)
}

/// 保存対象タスクのテンプレート PNG のうち無構造 (stddev < 閾値) のものへの
/// 警告リスト (Issue #184・fail-visible: 保存はブロックしない)。
///
/// 検証対象 (save 時点で検証可能な範囲):
/// - `pngs`: ROI 追加タスクの crop (インメモリ画像)。名前空間に残るタスクのみ
///   (削除済みタスクの crop は保存されないため検証しない)。
/// - 各タスクの `template` 参照 (UC-2 ライブラリ割当): `dir` 基準で解決し、
///   実在ファイルのみ検証する。参照先不在・デコード不能は本警告のスコープ外
///   (存在検証は実行時の detect が担う — 警告はあくまで認識品質の予見)。
///
/// stddev 計算・閾値判定は [`crate::library::template_structure_warning`]
/// (anaden-vision の単一実装経由) に委譲する。
fn unstructured_template_warnings(
    state: &ScenarioEditorState,
    pngs: &[(String, DynamicImage)],
    dir: &Path,
) -> Vec<String> {
    let mut warnings = Vec::new();
    for task in &state.tasks {
        // ROI 追加タスク: 保存しようとしている crop 本体を検証する。
        if let Some((_, img)) = pngs.iter().find(|(name, _)| name == &task.name) {
            if let Some(warning) = crate::library::template_structure_warning(img) {
                warnings.push(format!("{}: {warning}", task.name));
            }
            continue;
        }
        // UC-2 参照割当タスク: 参照先ファイルを実読みして検証する。
        let path = if task.template.is_absolute() {
            task.template.clone()
        } else {
            dir.join(&task.template)
        };
        if !path.is_file() {
            continue;
        }
        let Ok(img) = image::open(&path) else {
            continue;
        };
        if let Some(warning) = crate::library::template_structure_warning(&img) {
            warnings.push(format!("{}: {warning}", task.name));
        }
    }
    warnings
}

/// 各 TaskDef の書き出し先 `<dir>/<stem-or-name>.toml` を導出する (UC-4:
/// ロード済みタスクは元ファイル stem、対応エントリの無い新規タスクは
/// TaskDef name)。順序は `state.tasks` と一致する。
fn task_out_paths(state: &ScenarioEditorState, dir: &Path) -> Vec<PathBuf> {
    state
        .tasks
        .iter()
        .map(|task| {
            let stem = state
                .loaded_task_files
                .iter()
                .find(|(n, _)| n == &task.name)
                .map(|(_, stem)| stem.as_str())
                .unwrap_or(task.name.as_str());
            dir.join(format!("{stem}.toml"))
        })
        .collect()
}

/// 書き出しパスの一意性検査 (Issue #180 minor-3)。
///
/// 比較は [`path_eq_ignore_case`] (Windows FS では大文字小文字違いも同一
/// ファイル)。最初の衝突 (タスク順) を [`ScenarioSaveError::TaskFileCollision`]
/// で返す。
fn check_unique_out_paths(
    state: &ScenarioEditorState,
    out_paths: &[PathBuf],
) -> Result<(), ScenarioSaveError> {
    for (i, path_a) in out_paths.iter().enumerate() {
        for (j, path_b) in out_paths.iter().enumerate().skip(i + 1) {
            if path_eq_ignore_case(path_a, path_b) {
                let name = |idx: usize| {
                    state
                        .tasks
                        .get(idx)
                        .map_or_else(String::new, |t| t.name.clone())
                };
                return Err(ScenarioSaveError::TaskFileCollision {
                    path: path_b.clone(),
                    first: name(i),
                    second: name(j),
                });
            }
        }
    }
    Ok(())
}

/// パスのケースインセンシティブ比較 (ASCII — Windows FS の同一ファイル判定)。
fn path_eq_ignore_case(a: &Path, b: &Path) -> bool {
    a.to_string_lossy().to_ascii_lowercase() == b.to_string_lossy().to_ascii_lowercase()
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
/// 書き出しパスとの比較はケースインセンシティブ ([`path_eq_ignore_case`] —
/// Issue #180 minor-4): Windows FS では大文字小文字違いも同一ファイルのため、
/// バイト比較だとケース違い stem 衝突のファイルを「未書き出し」と誤認して
/// 書いた直後のファイルを sweep しうる (本来は保存前の stem 衝突検査
/// [`ScenarioSaveError::TaskFileCollision`] で拒否される組み合わせの二重防御)。
///
/// 保存した全 TaskDef の書き出しに成功した後にのみ呼ぶ (失敗時の部分状態で
/// 旧ファイルを消さない)。削除自体は best-effort — 失敗しても再 load 時に
/// 旧タスクが復活するだけでデータ破損にはならないためエラーにはしない。
fn sweep_removed_taskdefs(dir: &Path, written: &[PathBuf]) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let written_lower: Vec<String> = written
        .iter()
        .map(|w| w.to_string_lossy().to_ascii_lowercase())
        .collect();
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
        let is_written = written_lower.contains(&path.to_string_lossy().to_ascii_lowercase());
        if is_toml && !is_manifest && !is_written {
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

    /// dir 内の (ファイル名, バイト列) の決定論的スナップショット
    /// (バイト不変アサーション用)。
    fn dir_snapshot(dir: &Path) -> Vec<(String, Vec<u8>)> {
        let mut out: Vec<_> = fs::read_dir(dir)
            .expect("read_dir")
            .flatten()
            .map(|e| {
                let p = e.path();
                let name = p.file_name().unwrap().to_string_lossy().to_string();
                (name, fs::read(&p).unwrap())
            })
            .collect();
        out.sort();
        out
    }

    /// minor-3 (Issue #180): stem ≠ name の既存 pipeline へ stem と同名の新規
    /// タスクを追加すると書き出しパスが衝突する → 保存前に専用エラーで拒否し
    /// 1 バイトも書かない (バイト不変)。
    #[test]
    fn save_refuses_stem_collision_writing_nothing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let pipelines_root = tmp.path().join("pipelines");

        // (1) 既存 pipeline を作る: ファイル FieldHudTr.toml の TaskDef name は
        //     FieldHudWide (stem ≠ name — UC-4 の往復契約と同構成)。
        let mut st = ScenarioEditorState::new("field_hud");
        st.add_task(task_def("FieldHudWide", Some(vec![])));
        st.loaded_task_names = vec!["FieldHudWide".to_string()];
        st.loaded_task_files = vec![("FieldHudWide".to_string(), "FieldHudTr".to_string())];
        st.add_goal(loop_goal("loop3", 3));
        let dir = save_scenario(&st, &[], &pipelines_root).expect("initial save");
        let before = dir_snapshot(&dir);

        // (2) ロード → 新規タスク FieldHudTr (= 既存 stem) を追加。
        //     既存バリデーション (タスク名一意性・命名規約) では検出できない。
        let mut st2 = crate::scenario_load::load_scenario_from_dir(&dir).expect("load");
        st2.add_task(task_def("FieldHudTr", Some(vec![])));
        assert!(
            st2.validate().is_ok(),
            "stem 衝突は既存バリデーションでは検出できないこと: {:?}",
            st2.validation_issues()
        );

        // (3) 保存拒否 (TaskFileCollision) + dir は 1 バイトも変わらない。
        let err = save_scenario(&st2, &[], &pipelines_root).expect_err("must refuse");
        assert!(
            matches!(
                err,
                ScenarioSaveError::TaskFileCollision { ref path, .. }
                    if path.file_name().is_some_and(|n| n == "FieldHudTr.toml")
            ),
            "err: {err:?}"
        );
        assert_eq!(dir_snapshot(&dir), before, "1 バイトも書かない");
    }

    /// minor-3 (ケース違い): ロード済み 2 タスクの stem がケース違いのみ
    /// (StartPc / startpc) でも Windows では同一ファイル → 衝突として拒否。
    #[test]
    fn save_refuses_case_variant_stem_collision() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let pipelines_root = tmp.path().join("pipelines");

        let mut st = ScenarioEditorState::new("s");
        st.add_task(task_def("StartPc", Some(vec![])));
        st.add_task(task_def("StartPcSub", Some(vec![])));
        st.loaded_task_names = vec!["StartPc".to_string(), "StartPcSub".to_string()];
        st.loaded_task_files = vec![
            ("StartPc".to_string(), "StartPc".to_string()),
            ("StartPcSub".to_string(), "startpc".to_string()),
        ];
        st.add_goal(loop_goal("g", 1));
        assert!(st.validate().is_ok());

        let err = save_scenario(&st, &[], &pipelines_root).expect_err("must refuse");
        assert!(
            matches!(err, ScenarioSaveError::TaskFileCollision { .. }),
            "err: {err:?}"
        );
        assert!(
            !pipelines_root.join("s").exists(),
            "保存先ディレクトリすら作らない"
        );
    }

    /// minor-4 (Issue #180): sweep の書き出しパス比較はケースインセンシティブ。
    /// ケース違い stem (beta.toml vs Beta.toml) は同一ファイル扱いし sweep され
    /// ない (保存前の stem 衝突検査で本来拒否される組み合わせの二重防御)。
    #[test]
    fn sweep_keeps_case_variant_of_written_task_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().join("p");
        fs::create_dir_all(&dir).expect("mkdir");
        fs::write(dir.join("Alpha.toml"), b"a").expect("write");
        fs::write(dir.join("Beta.toml"), b"b").expect("write");

        // 書き出しパス beta.toml は Beta.toml とケース違いのみ → 同一ファイル扱い。
        sweep_removed_taskdefs(&dir, &[dir.join("beta.toml")]);

        assert!(
            dir.join("Beta.toml").exists(),
            "ケース違い stem は sweep されない"
        );
        assert!(
            !dir.join("Alpha.toml").exists(),
            "完全不一致の旧ファイルは sweep される"
        );
    }

    // ---- Issue #184: 無構造テンプレート警告 (保存経路 3・fail-visible) ----
    //
    // pending PNG (ROI 追加 crop) と UC-2 参照 PNG の両方を検証し、警告は
    // ScenarioSaveOutcome::warnings として返る。保存自体はブロックしない。
    // 判定は anaden-vision の単一実装経由 (library::template_structure_warning)。

    /// 構造ありテンプレート用のグラデーション画像 (stddev 約 57 > 閾値 20)。
    fn gradient_image(w: u32, h: u32) -> DynamicImage {
        let mut img = image::GrayImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let v = ((x * 2 + y * 3) % 200) as u8;
                img.put_pixel(x, y, image::Luma([v]));
            }
        }
        DynamicImage::ImageLuma8(img)
    }

    /// ほぼ単色画像 (stddev = 0 — Issue #182 の旧テンプレート相当)。
    fn flat_image(w: u32, h: u32) -> DynamicImage {
        DynamicImage::ImageLuma8(image::GrayImage::from_pixel(w, h, image::Luma([250u8])))
    }

    /// 無構造 pending PNG (ROI 追加 crop) を含む保存は成功し、警告 1 件
    /// (対象タスク名 + 認識不能の恐れ) を返す。PNG 自体は書かれる (ブロックしない)。
    #[test]
    fn save_scenario_with_warnings_flags_flat_pending_png_but_saves() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("pipelines");

        let mut st = ScenarioEditorState::new("warn_flat");
        st.add_task(task_def("Start", Some(vec![])));
        st.add_goal(loop_goal("g", 3));
        let pngs = vec![("Start".to_string(), flat_image(100, 50))];

        let outcome = save_scenario_with_warnings(&st, &pngs, &root).expect("save must succeed");
        assert_eq!(outcome.dir, root.join("warn_flat"));
        assert_eq!(
            outcome.warnings.len(),
            1,
            "無構造 pending PNG に警告 1 件: {:?}",
            outcome.warnings
        );
        assert!(
            outcome.warnings[0].contains("Start"),
            "警告は対象タスク名を含む: {}",
            outcome.warnings[0]
        );
        assert!(
            outcome.warnings[0].contains("認識不能"),
            "警告は認識不能の恐れを明示: {}",
            outcome.warnings[0]
        );
        // 保存はブロックしない → PNG 実体が書かれている。
        assert!(outcome.dir.join("Start.png").exists());
    }

    /// 構造あり pending PNG のみなら警告なし (偽陽性ゼロ)。
    #[test]
    fn save_scenario_with_warnings_structured_png_has_no_warning() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("pipelines");

        let mut st = ScenarioEditorState::new("warn_ok");
        st.add_task(task_def("Start", Some(vec![])));
        st.add_goal(loop_goal("g", 3));
        let pngs = vec![("Start".to_string(), gradient_image(100, 50))];

        let outcome = save_scenario_with_warnings(&st, &pngs, &root).expect("save must succeed");
        assert!(
            outcome.warnings.is_empty(),
            "構造ありテンプレートに警告を出さない: {:?}",
            outcome.warnings
        );
    }

    /// UC-2 参照割当 (ライブラリ PNG ファイル参照) も検証される: 無構造参照は
    /// 警告、構造あり参照は警告なし。絶対パス参照 (assign_template 後の保存状態)。
    #[test]
    fn save_scenario_with_warnings_flags_flat_uc2_reference_png() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("pipelines");

        // 参照先ライブラリ PNG (無構造 / 構造あり) を作る。
        let lib_dir = tmp.path().join("lib");
        fs::create_dir_all(&lib_dir).expect("mkdir lib");
        let flat_ref = lib_dir.join("flat_ref.png");
        flat_image(64, 32).save(&flat_ref).expect("save flat ref");
        let structured_ref = lib_dir.join("structured_ref.png");
        gradient_image(64, 32)
            .save(&structured_ref)
            .expect("save structured ref");

        let mut st = ScenarioEditorState::new("warn_uc2");
        let mut flat_task = task_def("FlatRef", Some(vec![]));
        flat_task.template = flat_ref;
        let mut ok_task = task_def("OkRef", Some(vec![]));
        ok_task.template = structured_ref;
        st.add_task(flat_task);
        st.add_task(ok_task);
        st.add_goal(loop_goal("g", 1));

        let outcome = save_scenario_with_warnings(&st, &[], &root).expect("save must succeed");
        assert_eq!(
            outcome.warnings.len(),
            1,
            "無構造参照のみ警告 (構造あり参照は警告なし): {:?}",
            outcome.warnings
        );
        assert!(
            outcome.warnings[0].contains("FlatRef"),
            "警告は対象タスク名を含む: {}",
            outcome.warnings[0]
        );
    }

    /// 参照先ファイルが存在しないタスクは警告なし (存在検証は detect の責務 —
    /// 本警告は認識品質の予見のみ。fail-visible であり fail-closed ではない)。
    /// 従来の save_scenario ラッパも引き続き dir のみ返す (互換)。
    #[test]
    fn save_scenario_with_warnings_skips_missing_reference_and_wrapper_keeps_dir() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("pipelines");

        let mut st = ScenarioEditorState::new("warn_ghost");
        let mut ghost = task_def("Ghost", Some(vec![]));
        ghost.template = PathBuf::from("does_not_exist.png");
        st.add_task(ghost);
        st.add_goal(loop_goal("g", 1));

        let outcome = save_scenario_with_warnings(&st, &[], &root).expect("save must succeed");
        assert!(
            outcome.warnings.is_empty(),
            "参照先不在は警告しない: {:?}",
            outcome.warnings
        );

        // 従来ラッパ (save_scenario) は PathBuf を返すまま (呼出元互換)。
        // 連続保存は panel と同様に loaded_from 所有権を設定してから。
        st.loaded_from = Some(outcome.dir.clone());
        let dir = save_scenario(&st, &[], &root).expect("wrapper save");
        assert_eq!(dir, root.join("warn_ghost"));
    }
}
