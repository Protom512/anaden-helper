//! Issue #268 (menu_pc アンカー陳腐化) のゲートテスト: ゲーム 3.17.50 がメニューバー
//! UI を変更したことで旧アンカー7件が全件非マッチになったため、2026-11-03 実機キャプチャ
//! から再導出した 6 アンカー (party/grasta/board/bag/record/info) が本番前処理
//! (ScreenScaler::normalize_capture) → roi 付き detect の経路で閾値以上にマッチすることを
//! 機械保証する (neko_nikki_live_regression と同一形式)。
//!
//! ## 資産の由来
//!
//! - メニュー画面: 2026-11-03 live セッション (r2) の 2880x1620 PrintWindow キャプチャ
//!   (左黒帯 10px)。3.17.50 の新メニューバーは 110px ピッチのタイル列
//!   (パーティ/グラスタ/ボード/夢見/持ち物/記録/お知らせ/その他 + 右端小タイル
//!   ショップ/ねこ/食事/ボックス、スクロール矢印なし)。
//! - テンプレートは正規ツール (anaden-device/examples/resize_crop_template,
//!   `--letterbox 10` → raw-1258 リサイズ → ROI クロップ) で生成。生成コマンドは各 TOML
//!   コメントに記録 (絶対命令: 生成コマンドをログに残す)。
//! - MenuPcGacha は新バーに「ガチャ」タイルが存在しないため再導出対象外 (旧テンプレの
//!   まま stale 保持 — Issue #268 の live フォローアップ項目)。
//!
//! ## 保証内容
//!
//! 1. 再導出 6 アンカーが live メニュー fixture 上で threshold (0.95) かつ品質バッファ
//!    0.95 を超える (自己一致 — テンプレは同一キャプチャの自己クロップ)。
//! 2. タイル識別: 隣接タイルの ROI にはマッチしない (全タイルが同一石枠フレームを共有
//!    するため、アイコン+ラベル差で識別されることが bar 内誤対排除の契約)。
//! 3. 画面識別: 異画面 fixture (記録/ワールドマップ/フィールド/ねこにっき) にマッチしない。
//!
//! fixture は `templates/` バンク外 (`tests/fixtures/`) なので template_bank_audit
//! の対象外。実際のテンプレート資産は `templates/scenes/menu_pc/`。

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

fn menu_pc_tasks() -> Vec<TaskDef> {
    let dir = workspace_root()
        .join("templates")
        .join("scenes")
        .join("menu_pc");
    load_pipeline(&dir).unwrap_or_else(|e| panic!("load menu_pc templates {}: {e}", dir.display()))
}

fn find_task<'a>(tasks: &'a [TaskDef], name: &str) -> &'a TaskDef {
    tasks
        .iter()
        .find(|t| t.name == name)
        .unwrap_or_else(|| panic!("{name} must exist in menu_pc templates"))
}

/// Issue #268 で再導出した 6 アンカー (gacha は新バーにタイル不在のため対象外)。
const REBUILT: &[&str] = &[
    "MenuPcParty",
    "MenuPcGrasta",
    "MenuPcBoard",
    "MenuPcBag",
    "MenuPcRecord",
    "MenuPcInfo",
];

/// タスクを正規化済み画像に detect し、(マッチしたら conf, 非マッチなら None) を返す。
fn detect_conf(task: &TaskDef, normalized: &image::DynamicImage) -> Option<f32> {
    task.detect(normalized, Path::new(""))
        .unwrap_or_else(|e| panic!("{} detect error: {e}", task.name))
        .map(|m| m.confidence.0)
}

/// 生キャプチャ (2026-11-03 r2 セッション、2880x1620・左黒帯 10px) を本番経路
/// (黒帯クロップ + 基準幅 1280 正規化) へ通す。
fn normalize_live_fixture(name: &str) -> image::DynamicImage {
    let path = fixture(name);
    let raw = image::open(&path)
        .unwrap_or_else(|e| panic!("open fixture {}: {e} (tracked file)", path.display()));
    assert_eq!(
        (raw.width(), raw.height()),
        (2880, 1620),
        "{name} must be a 2026-11-03 PrintWindow capture (Issue #268 evidence)"
    );
    let (normalized, crop) = ScreenScaler::new().normalize_capture(&raw);
    assert_eq!(
        (crop.offset_x, crop.offset_y),
        (10, 0),
        "{name}: 2026-11-03 captures carry a 10px left black band; template generation depends on it"
    );
    assert_eq!(normalized.width(), 1280);
    normalized
}

/// 再導出 6 アンカーが live メニュー fixture 上で 0.95 以上 (threshold 0.95 + 品質バッファ)
/// にマッチする。テンプレは同一キャプチャ由来の自己クロップのため、0.95 未満は資産破損扱い。
#[test]
#[allow(clippy::unwrap_used)]
fn rebuilt_anchors_match_live_menu_fixture_through_production_preprocessing() {
    let normalized = normalize_live_fixture("menu_panel_live_2880x1620.png");
    let tasks = menu_pc_tasks();
    for name in REBUILT {
        let task = find_task(&tasks, name);
        let conf = detect_conf(task, &normalized).unwrap_or_else(|| {
            panic!(
                "{name} must MATCH the menu fixture (roi={:?} thr={})",
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

/// タイル識別: 全タイルが同一の石枠フレームを共有するため、アンカーが隣接タイルの ROI に
/// 誤対してはならない (バー内 12 タイルの識別はアイコン+ラベル差のみに載る)。
#[test]
#[allow(clippy::unwrap_used)]
fn rebuilt_anchors_do_not_cross_match_neighbor_tile_rois() {
    let normalized = normalize_live_fixture("menu_panel_live_2880x1620.png");
    let tasks = menu_pc_tasks();
    for name in REBUILT {
        let task = find_task(&tasks, name);
        for other in REBUILT {
            if other == name {
                continue;
            }
            let neighbor_roi = find_task(&tasks, other).roi;
            let mut swapped = task.clone();
            swapped.roi = neighbor_roi;
            let conf = detect_conf(&swapped, &normalized);
            assert!(
                conf.is_none(),
                "{name} must NOT match the tile ROI of {other:?} (got conf {conf:?})"
            );
        }
    }
}

/// 画面識別: メニューバーのタイルアンカーは、メニューを開いていない実機画面
/// (記録 / ワールドマップ / フィールド / フィールドHUD / ねこにっき) にマッチしない。
#[test]
#[allow(clippy::unwrap_used)]
fn rebuilt_anchors_do_not_match_other_live_screens() {
    let tasks = menu_pc_tasks();
    for fixture_name in [
        "record_open_live_2880x1620.png",
        "world_map_live_2880x1620.png",
        "field_live_2880x1620.png",
        "field_hud_live_2880x1620.png",
        "neko_diary_live_2880x1620.png",
    ] {
        let normalized = normalize_live_fixture(fixture_name);
        for name in REBUILT {
            let task = find_task(&tasks, name);
            assert!(
                detect_conf(task, &normalized).is_none(),
                "{name} must NOT match the {fixture_name} fixture (cross-screen false positive)"
            );
        }
    }
}
