//! Issue #260 E5 (ねこにっき ヒント文言) のゲートテスト: 2026-11-03 実機キャプチャ
//! から生成したヒント文言アンカーが、本番前処理 (ScreenScaler::normalize_capture)
//! → roi 付き detect の経路で閾値以上にマッチすることを機械保証する
//! (ticket_digest_live_regression と同一形式)。
//!
//! ## 資産の由来
//!
//! - ねこにっき画面: 2026-11-03 live セッション (2880x1620 PrintWindow、左黒帯 10px)。
//!   記録 (Records) → にっき タブ遷移直後のフレーム。No.01 行のヒント冒頭
//!   「ミ・パル」グリフ群 (x585-678, y164-194, raw-1258 空間)。
//!   このフレームは同セッションのゲームソフトフリーズ発生フレームでもある
//!   (以降 18 分間 MD5 不変 — .anaden-run/2026-11-03-e5e7-atlas3-r2/)。
//! - テンプレートは正規ツール (anaden-device/examples/resize_crop_template,
//!   `--letterbox 10` → raw-1258 リサイズ → ROI クロップ) で生成。
//!   2880x1620 キャプチャは左 10px 黒帯を持つため normalize_capture の
//!   検出挙動 (offset 10,0) と揃えるため --letterbox 10 が必須
//!   (--letterbox 0 だと約 4px の横ずれが発生する)。
//!
//! ## 保証内容
//!
//! 1. ヒント文言アンカーが live fixture 上で threshold (0.90) かつ品質バッファ
//!    0.95 を超える (E5「文字照合」実証の機械保証)。
//! 2. 行識別: 同一画面の別行 (No.02) の文言帯 ROI にはマッチしない
//!    (73 種ヒント識別の基本契約 — 隣接行との誤対排除)。
//! 3. 画面識別: 異画面 fixture (AD 選択画面) にマッチしない。
//!
//! fixture は `templates/` バンク外 (`tests/fixtures/`) なので template_bank_audit
//! の対象外。実際のテンプレート資産は `templates/scenes/neko_pc/`。

use std::path::{Path, PathBuf};

use anaden_vision::{ScreenScaler, TaskDef, load_pipeline};

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

fn neko_tasks() -> Vec<TaskDef> {
    let dir = workspace_root()
        .join("templates")
        .join("scenes")
        .join("neko_pc");
    load_pipeline(&dir).unwrap_or_else(|e| panic!("load neko_pc templates {}: {e}", dir.display()))
}

fn find_task<'a>(tasks: &'a [TaskDef], name: &str) -> &'a TaskDef {
    tasks
        .iter()
        .find(|t| t.name == name)
        .unwrap_or_else(|| panic!("{name} must exist in neko_pc templates"))
}

fn detect_conf(task: &TaskDef, normalized: &image::DynamicImage) -> Option<f32> {
    task.detect(normalized, Path::new(""))
        .unwrap_or_else(|e| panic!("{} detect error: {e}", task.name))
        .map(|m| m.confidence.0)
}

fn normalize_diary_fixture() -> image::DynamicImage {
    let path = fixture("neko_diary_live_2880x1620.png");
    let raw = image::open(&path)
        .unwrap_or_else(|e| panic!("open fixture {}: {e} (tracked file)", path.display()));
    assert_eq!(
        (raw.width(), raw.height()),
        (2880, 1620),
        "fixture must be the 2026-11-03 PrintWindow capture (Issue #260 E5 evidence)"
    );
    let (normalized, crop) = ScreenScaler::new().normalize_capture(&raw);
    assert_eq!(
        (crop.offset_x, crop.offset_y),
        (10, 0),
        "2026-11-03 captures carry a 10px left black band; template generation depends on it"
    );
    assert_eq!(normalized.width(), 1280);
    normalized
}

/// E5 実証: ヒント文言アンカー「ミ・パル」が本番前処理経路で 0.95 以上にマッチする。
#[test]
#[allow(clippy::unwrap_used)]
fn hint_text_anchor_matches_live_diary_fixture_through_production_preprocessing() {
    let normalized = normalize_diary_fixture();
    let tasks = neko_tasks();
    let task = find_task(&tasks, "NekoHintMiparuPc");
    let conf = detect_conf(task, &normalized).unwrap_or_else(|| {
        panic!(
            "NekoHintMiparuPc must MATCH the diary fixture (roi={:?} thr={})",
            task.roi, task.threshold
        )
    });
    assert!(
        conf >= 0.95,
        "NekoHintMiparuPc: conf {conf:.4} below the 0.95 quality buffer (threshold {})",
        task.threshold
    );
}

/// 行識別: 同じ画面の No.02 行文言帯 ROI にはマッチしてはならない。73 種の
/// ヒント識別は「行位置で ROI を切り替えて全ヒントテンプレを照合する」運用
/// (ui-atlas §2) なので、隣接行の別文言への誤対があれば運用全体が破綻する。
#[test]
#[allow(clippy::unwrap_used)]
fn hint_text_anchor_does_not_match_second_diary_row() {
    let normalized = normalize_diary_fixture();
    let tasks = neko_tasks();
    let mut row2 = find_task(&tasks, "NekoHintMiparuPc").clone();
    row2.roi = Some([470, 205, 340, 100]);
    let conf = detect_conf(&row2, &normalized);
    assert!(
        conf.is_none(),
        "NekoHintMiparuPc must NOT match diary row 2 text band (got conf {conf:?})"
    );
}

/// 画面識別: ねこにっきの文言アンカーは AD 選択画面 fixture にマッチしない。
#[test]
#[allow(clippy::unwrap_used)]
fn hint_text_anchor_does_not_match_ad_select_fixture() {
    let raw = image::open(fixture("ad_select_live_1918x1048.png"))
        .unwrap_or_else(|e| panic!("open fixture: {e}"));
    let (normalized, _crop) = ScreenScaler::new().normalize_capture(&raw);
    let tasks = neko_tasks();
    let task = find_task(&tasks, "NekoHintMiparuPc");
    let conf = detect_conf(task, &normalized);
    assert!(
        conf.is_none(),
        "NekoHintMiparuPc must NOT match the AD-select fixture (got conf {conf:?})"
    );
}
