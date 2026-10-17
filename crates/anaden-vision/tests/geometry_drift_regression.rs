//! Issue #212 回帰テスト: capture 幾何ドリフト時の roi 座標系保全を機械保証する。
//!
//! ## 背景 (Issue #212 — roi 座標系が幾何ドリフトで不整合)
//!
//! 2026-09-24 の live ループ (issue210-verify3: 8 iters fired=0) と offline 再現
//! (`anaden-tool run-pipeline capture_probe.png templates/pipelines/login
//! LoginTapTitlePc` → NoMatch) から調査した結果、障害は 2 層だった:
//!
//! 1. **前処理の経路乖離**: `anaden-tool run-pipeline` は normalize のみ (黒帯クロップ
//!    なし) で engine live 経路 (crop_to_content → normalize) と異なる前処理を使って
//!    いた → `ScreenScaler::normalize_capture` (クロップ + アスペクト保護 + 正規化)
//!    に単一化した。engine/tool 両経路が同一関数を呼ぶ。
//! 2. **キャンバス内部の黒による幾何破壊** (本テストの主題): capture_probe.png は
//!    起動途中のゲームウィンドウ (1952x1098 = 16:9) のうち上半分 579 行にしか描画が
//!    なく、下 519 行が**純黒** (行平均輝度 0.00) の過渡フレームだった。
//!    `crop_to_content_with_info` はこの内部黒を黒帯と誤判定して 1952x577 に crop →
//!    normalize 1280x378 (アスペクト 3.39 = 16:9 比 +90%) → `roi_to_normalized` の
//!    Y スケールが 378/708 = 0.53 倍に圧縮され roi 窓・needle の両方が崩壊。
//!    修正: `crop_to_canvas_with_info` はクロップ結果のアスペクト比が 16:9 ±5% 内に
//!    収まるときのみクロップを受理し、外れる場合は元画像全体 (= キャンバス) を使う。
//!
//! ## 本テストの保証内容
//!
//! - [`degenerate_boot_frame_preserves_geometry_and_reports_nomatch`]:
//!   過渡フレーム fixture で (a) 素 crop_to_content が幾何を壊すこと (罠の pin)、
//!   (b) normalize_capture が 1280x720 を維持すること (幾何保全)、(c) roi 付き
//!   detect が正しく NoMatch を返すこと (needle が存在しないフレームに対する誤マッチ
//!   抑制) を機械保証する。
//! - [`healthy_title_geometry_and_degenerate_geometry_both_green`]:
//!   健全 fixture (1942x1098 content・#208 pin) は MATCH・過渡 fixture は幾何保全の上
//!   NoMatch — 複数幾何で座標系契約が保たれることを 1 テストで対比保証する。
//!
//! fixture は `templates/` バンク外 (`tests/fixtures/`) なので template_bank_audit
//! の対象外 (title_live_regression.rs と同じ配置契約)。

use std::path::{Path, PathBuf};

use anaden_vision::{CropInfo, ScreenScaler, TaskDef, crop_to_content_with_info, load_pipeline};

/// workspace ルート (crates/anaden-vision から `../../`)。
fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

/// Issue #212 の過渡フレーム fixture (capture_probe.png・2026-09-24 由来)。
///
/// 実 PrintWindow キャプチャ (1952x1098 RGBA)。起動途中のゲームウィンドウで、
/// 上 579 行のみバイナリな黑白描画 (行平均 126.45・行内 stdev 127.5 = 5:5 の黑白)、
/// 下 519 行は純黒 (行平均 0.00)。title_logo_corner needle は存在しない
/// (全文探索最良 conf 0.31 << threshold 0.80)。
fn degenerate_fixture() -> PathBuf {
    fixtures_dir().join("title_boot_degenerate_1952x1098.png")
}

fn healthy_fixture() -> PathBuf {
    fixtures_dir().join("title_live_1952x1098.png")
}

fn login_tap_title_task() -> TaskDef {
    let login_dir = workspace_root()
        .join("templates")
        .join("pipelines")
        .join("login");
    let tasks = load_pipeline(&login_dir)
        .unwrap_or_else(|e| panic!("load login pipeline {}: {e}", login_dir.display()));
    tasks
        .into_iter()
        .find(|t| t.name == "LoginTapTitlePc")
        .expect("LoginTapTitlePc must exist in templates/pipelines/login")
}

/// 過渡ブートフレーム: (a) 素 crop は幾何を壊す、(b) normalize_capture は幾何を保全、
/// (c) roi 付き detect は正しく NoMatch。
#[test]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
fn degenerate_boot_frame_preserves_geometry_and_reports_nomatch() {
    let raw = image::open(degenerate_fixture()).unwrap_or_else(|e| {
        panic!(
            "open degenerate fixture {}: {e} (tests/fixtures/title_boot_degenerate_1952x1098.png \
             is tracked — a fresh clone must contain it)",
            degenerate_fixture().display()
        )
    });
    assert_eq!(
        (raw.width(), raw.height()),
        (1952, 1098),
        "degenerate fixture must be the 1952x1098 PrintWindow capture (Issue #212 evidence)"
    );

    // (a) 罠の pin: 素 crop_to_content はキャンバス内部の未描画黒 (下 519 行) を黒帯と
    //     誤判定して 1952x577 (アスペクト 3.39) に crop し幾何を破壊する。
    //     これが Issue #212 の数値的根拠 (normalize 1280x378 → roi Y 圧縮 0.53 倍)。
    let (raw_cropped, _) = crop_to_content_with_info(&raw);
    assert_eq!(
        (raw_cropped.width(), raw_cropped.height()),
        (1952, 577),
        "unguarded crop eats the unpainted in-canvas black (the Issue #212 trap)"
    );

    // (b) 修正後の本番前処理: アスペクト保護がクロップを棄却 → 全面 1952x1098 が
    //     そのまま 16:9 幾何で 1280x720 に正規化される (roi/needle の Y スケール保全)。
    let (normalized, crop_info) = ScreenScaler::new().normalize_capture(&raw);
    assert_eq!(
        crop_info,
        CropInfo::full(1952, 1098),
        "aspect guard must reject the geometry-destroying crop"
    );
    assert_eq!(
        (normalized.width(), normalized.height()),
        (1280, 720),
        "geometry must be preserved: full 16:9 canvas normalizes to 1280x720, not 1280x378"
    );

    // (c) 座標系が正しい状態での roi 付き detect: needle が存在しないフレームなので
    //     NoMatch が正。幾何破壊 (旧 1280x378) との違いは「roi 窓が正しい位置にある」
    //     ことで、これは (b) の寸法 pin が担保する。
    let task = login_tap_title_task();
    let matched = task
        .detect(&normalized, Path::new(""))
        .expect("LoginTapTitlePc detect must not error");
    assert!(
        matched.is_none(),
        "degenerate boot frame must honestly report NoMatch (needle absent: full-scan best \
         conf 0.31 < threshold {}). A match here would be a false positive.",
        task.threshold
    );
}

/// 複数幾何の対比保証: 健全 fixture は MATCH、過渡 fixture は幾何保全 + NoMatch。
/// 両者が同じ前処理 (normalize_capture) と同じ TaskDef で評価されること自体が
/// Issue #212「幾何が変化しても座標系が壊れない構造」の契約。
#[test]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
fn healthy_title_geometry_and_degenerate_geometry_both_green() {
    let task = login_tap_title_task();

    // 健全幾何 (#208 fixture): 左 8px 黒帯 → crop (10,0) 1942x1098 (16:9 比 +0.52%)
    // → アスペクト保護は受理 → 1280x724 → roi 付き detect は閾値以上に MATCH。
    let healthy = image::open(healthy_fixture())
        .unwrap_or_else(|e| panic!("open healthy fixture {}: {e}", healthy_fixture().display()));
    let (healthy_norm, healthy_info) = ScreenScaler::new().normalize_capture(&healthy);
    assert_eq!((healthy_info.offset_x, healthy_info.offset_y), (10, 0));
    assert_eq!((healthy_info.width, healthy_info.height), (1942, 1098));
    assert_eq!((healthy_norm.width(), healthy_norm.height()), (1280, 724));
    let matched = task
        .detect(&healthy_norm, Path::new(""))
        .expect("detect on healthy fixture must not error")
        .expect("healthy title fixture must MATCH through normalize_capture");
    assert!(
        matched.confidence.0 >= task.threshold,
        "healthy fixture confidence {} must exceed threshold {}",
        matched.confidence.0,
        task.threshold
    );

    // 過渡幾何 (Issue #212 fixture): 幾何保全 (1280x720) + 正しい NoMatch。
    let degenerate = image::open(degenerate_fixture()).unwrap_or_else(|e| {
        panic!(
            "open degenerate fixture {}: {e}",
            degenerate_fixture().display()
        )
    });
    let (degenerate_norm, degenerate_info) = ScreenScaler::new().normalize_capture(&degenerate);
    assert_eq!(degenerate_info, CropInfo::full(1952, 1098));
    assert_eq!(
        (degenerate_norm.width(), degenerate_norm.height()),
        (1280, 720),
        "degenerate geometry must stay 16:9-normalized (not squeezed)"
    );
    assert!(
        task.detect(&degenerate_norm, Path::new(""))
            .expect("detect on degenerate fixture must not error")
            .is_none(),
        "degenerate frame must be NoMatch (needle absent)"
    );
}
