//! Issue #248 (ticket_digest v0) のゲートテスト: 2026-11-01 実機キャプチャ
//! (Issue #260 E4 証跡) から生成した AD 選択画面・リザルト画面アンカーが、
//! 本番前処理 (ScreenScaler::normalize_capture) → roi 付き detect の経路で
//! 閾値以上にマッチすることを機械保証する (title_live_regression と同一形式)。
//!
//! ## 資産の由来
//!
//! - AD 選択画面: `.anaden-run/2026-11-01-e0-e7/e4_next.png` (1918x1048 PrintWindow)。
//!   意図せず AD に侵入した際のオート勝利後に表示された「アナザーダンジョン」
//!   選択モーダル (冥鬼王の領域・所持カードキー緑 0/9 赤 3/5 表示)。
//! - リザルト画面: `e4_after.png`。Congratulations バナー + 天/冥 数値行 + 報酬列。
//! - テンプレートは正規ツール (anaden-device/examples/resize_crop_template,
//!   `--letterbox 0` → raw-1258 リサイズ → ROI クロップ) で生成。
//!
//! ## 保証内容
//!
//! 1. 各タスクが live fixture 上で threshold (0.85) かつ品質バッファ 0.95 を超える。
//! 2. **誤対防止 (cross-fixture)**: AD 選択アンカーはリザルト fixture にマッチせず、
//!    その逆も同様 (画面分類アンカーの基本契約)。
//!
//! fixture は `templates/` バンク外 (`tests/fixtures/`) なので template_bank_audit
//! の対象外。実際のテンプレート資産は `templates/pipelines/ticket_digest/`。

use std::path::{Path, PathBuf};

use anaden_vision::{ScreenScaler, TaskDef, load_pipeline};

/// workspace ルート (crates/anaden-vision から `../../`)。
fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

fn ticket_digest_tasks() -> Vec<TaskDef> {
    let dir = workspace_root()
        .join("templates")
        .join("pipelines")
        .join("ticket_digest");
    load_pipeline(&dir)
        .unwrap_or_else(|e| panic!("load ticket_digest pipeline {}: {e}", dir.display()))
}

fn find_task<'a>(tasks: &'a [TaskDef], name: &str) -> &'a TaskDef {
    tasks
        .iter()
        .find(|t| t.name == name)
        .unwrap_or_else(|| panic!("{name} must exist in ticket_digest pipeline"))
}

/// タスクを正規化済み画像に detect し、(マッチしたら conf, 非マッチなら None) を返す。
fn detect_conf(task: &TaskDef, normalized: &image::DynamicImage) -> Option<f32> {
    task.detect(normalized, Path::new(""))
        .unwrap_or_else(|e| panic!("{} detect error: {e}", task.name))
        .map(|m| m.confidence.0)
}

/// 生キャプチャを本番経路 (Issue #212 単一化: 黒帯クロップ + 基準幅 1280 正規化) へ通す。
fn normalize_fixture(name: &str) -> image::DynamicImage {
    let path = fixture(name);
    let raw = image::open(&path)
        .unwrap_or_else(|e| panic!("open fixture {}: {e} (tracked file)", path.display()));
    assert_eq!(
        (raw.width(), raw.height()),
        (1918, 1048),
        "{name} must be the 2026-11-01 PrintWindow capture (Issue #260 E4 evidence)"
    );
    let (normalized, _crop) = ScreenScaler::new().normalize_capture(&raw);
    assert_eq!(
        normalized.width(),
        1280,
        "normalize must produce the 1280-base frame"
    );
    normalized
}

/// AD 選択画面アンカー 3 種が live fixture で 0.95 以上 (threshold 0.85 + 品質バッファ)
/// にマッチする。ad_select_title は半透明ヘッダだが自己一致 (同一キャプチャ由来) なので
/// 0.95 を下回るのは資産破損扱い。
///
/// なお「所持カードキー」パネルヘッダ anchor は試作したが輝度 stddev 5.97 で
/// template_bank_audit の無構造検出 (Issue #184, 閾値 20.0) に抵触するため採用を見送った
/// (半透明平坦文言 = 構造なし)。キー在庫読取は桁テンプレ (digit_0-9) 収集時に
/// icons 由来の構造ある anchor で再提案する。
#[test]
#[allow(clippy::unwrap_used)]
fn ad_select_anchors_match_live_fixture_through_production_preprocessing() {
    let normalized = normalize_fixture("ad_select_live_1918x1048.png");
    let tasks = ticket_digest_tasks();
    for name in ["AdSelectDetectPc", "HardRowDetectPc", "VeryHardRowDetectPc"] {
        let task = find_task(&tasks, name);
        let conf = detect_conf(task, &normalized).unwrap_or_else(|| {
            panic!(
                "{name} must MATCH the AD-select live fixture (roi={:?} thr={})",
                task.roi, task.threshold
            )
        });
        assert!(
            conf >= 0.95,
            "{name}: conf {conf:.4} below the 0.95 quality buffer (threshold {})",
            task.threshold
        );
    }
}

/// リザルト (Congratulations) アンカーがリザルト fixture で 0.95 以上にマッチする。
#[test]
#[allow(clippy::unwrap_used)]
fn result_congrats_anchor_matches_reward_fixture() {
    let normalized = normalize_fixture("reward_congrats_live_1918x1048.png");
    let tasks = ticket_digest_tasks();
    let task = find_task(&tasks, "ResultCongratsDetectPc");
    let conf = detect_conf(task, &normalized).unwrap_or_else(|| {
        panic!(
            "ResultCongratsDetectPc must MATCH the reward fixture (roi={:?} thr={})",
            task.roi, task.threshold
        )
    });
    assert!(
        conf >= 0.95,
        "ResultCongratsDetectPc: conf {conf:.4} below the 0.95 quality buffer"
    );
}

/// 誤対防止 (cross-fixture): AD 選択アンカーはリザルト画面にマッチせず、
/// リザルトアンカーは AD 選択画面にマッチしない (画面分類アンカーの基本契約)。
#[test]
#[allow(clippy::unwrap_used)]
fn anchors_do_not_cross_match_other_screen_fixture() {
    let ad_select = normalize_fixture("ad_select_live_1918x1048.png");
    let reward = normalize_fixture("reward_congrats_live_1918x1048.png");
    let tasks = ticket_digest_tasks();
    for name in ["AdSelectDetectPc", "HardRowDetectPc", "VeryHardRowDetectPc"] {
        let task = find_task(&tasks, name);
        assert!(
            detect_conf(task, &reward).is_none(),
            "{name} must NOT match the reward-screen fixture (cross-screen false positive)"
        );
    }
    let result_task = find_task(&tasks, "ResultCongratsDetectPc");
    assert!(
        detect_conf(result_task, &ad_select).is_none(),
        "ResultCongratsDetectPc must NOT match the AD-select fixture"
    );
}
