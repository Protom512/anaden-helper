//! Issue #208 回帰テスト: live タイトルキャプチャの本番前処理 → roi 付き detect を
//! 実ショット fixture で機械保証する。
//!
//! ## 背景 (Issue #208 — login pipeline live 実行が全サイクル NoMatch)
//!
//! 2026-09-18 の live 実行 (`.omc/logs/routine-e2e-0918c/`・12/12 NoMatch) で発見された
//! 回帰。オフライン `anaden-tool match` (roi なし全文探索) は 99% MATCH するのに、
//! live 経路 (capture → crop_to_content → normalize → roi 付き detect) は全滅する、と
//! いう座標系/前処理系の疑いがかけられたが、段階診断 (example issue208_staged による
//! A:raw / B:crop / C:normalize / D:本番経路 の差分計測) の結果、根因は **テンプレート
//! 資産の由来** だった:
//!
//! - `version_label.png` (Issue #182 で run-180 のエンジンキャプチャから再生成) は、
//!   生成元フレームがタイトル画面ではなく区切り線つきのオーバーレイ画面
//!   (「プレイヤー情報を検索」) だったため、テンプレート下端 2 行 (相対 y=18..19) に
//!   **画面固有の全幅ダーク区切り線** を含んでいた。
//! - 純タイトル画面にはこの区切り線が存在しない。TM_CCOEFF_NORMED は平均除去相関の
//!   ため、テンプレの高分散行 (区切り線) が Haystack 側に無いだけで conf が
//!   0.94 → 0.47 に崩壊し、roi 内の最良一致も区切り線を探して y 方向に外れた位置
//!   (y≈11) へ流れた。→ 12/12 NoMatch。
//! - 一方 run-180 当時 (2026-09-08) に発火していたのは、当時のゲーム画面がまさに
//!   区切り線つきオーバーレイ画面だったため (同一コード・同一 roi で conf 0.82-0.94)。
//!
//! 修正 (2 段階): まず実タイトルキャプチャ (routine-e2e-0918c の live PrintWindow
//! 1952x1098) から正規ツール (`anaden-device/examples/resize_crop_template`・letterbox
//! 10 → raw-1258 resize → roi [8,2,138,20]) でテンプレートを再生成した — これで
//! 09-19 実キャプチャには一致するが、07-07 probe (title_pc_probe.png) には依然 None。
//! ID 表示帯はバージョン/ID 文字列が変わる**内容可変資産**で cross-capture 安定性が
//! 原理的にないため、恒久修正として login/nav パイプライン (login/tap_title,
//! nav_to_field_pc/tap_to_start) の検出アンカーを title_logo_corner (ロゴ固定小特徴・
//! probe 実測 conf 0.9848) へ切替えた。version_label は scenes/title_pc 検出専用として
//! 残置する。さらにアンカー切替時の診断で、needle==roi 幅のぴったり ROI
//! [624,263,120,120] はキャプチャ間のコンテンツ配置ドリフト (実測 (+2,-1) px) で
//! 最良一致位置を roi 外に弾き conf 0.99→0.78 に崩壊することが判明したため、
//! 消費者 roi は原点中心 ±10px のスラック窓 [614,253,140,140] へ拡張した
//! (live 実測 conf 0.9944 が立つ)。
//!
//! ## 本テストの保証内容
//!
//! 1. [`title_live_shot_matches_login_task_through_production_preprocessing`]:
//!    実ショット fixture を live 経路と **同一の前処理** (crop_to_content_with_info →
//!    ScreenScaler::normalize) に通し、実 TOML の LoginTapTitlePc が roi 付き detect
//!    で閾値以上にマッチすることを機械保証する。前処理 (crop/normalize)・roi 座標系・
//!    テンプレート資産のいずれかが壊れたら RED。
//! 2. [`version_label_template_is_free_of_full_width_dark_rows`]:
//!    version_label テンプレートが「全幅ダーク行 (画面固有の区切り線)」を含まない
//!    ことを保証する。Issue #208 の再発 (区切り線つき画面からの再生成) を資産側で
//!    検出する (version_label は scenes 検出専用だが資産契約は継続 pin)。
//!
//! fixture は `templates/` バンク外 (`tests/fixtures/`) なので template_bank_audit
//! (templates/pipelines + templates/scenes の TOML 参照のみ走査) の対象外。

use std::path::{Path, PathBuf};

use anaden_vision::{ScreenScaler, TaskDef, crop_to_content_with_info, load_pipeline};

/// workspace ルート (crates/anaden-vision から `../../`)。
fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

/// Issue #208 の live 実ショット fixture (routine-e2e-0918c/uc2-shot-000.png 由来)。
///
/// 実 PrintWindow キャプチャ生データ (1952x1098 RGBA・左端 8px 黒帯込み)。タイトル
/// 画面 (左上ブランドバー = version_label アンカー位置・中央ロゴ = title_logo_corner
/// アンカー位置、ともに存在・区切り線なし) を写す。テンプレ `version_label.png` の
/// 再生成元と同一キャプチャだが、本テストは検出経路 (前処理 + roi + threshold) の全体を
/// fixture 上で実測するため、資産差し替えや前処理変更をまたいだ回帰を検出できる
/// (テンプレ再生成時に本テストが共に更新されるべき契約)。
fn title_live_fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("title_live_1952x1098.png")
}

/// Issue #208 の live 実ショットが本番経路 (crop_to_content → normalize) を通って
/// LoginTapTitlePc の roi 付き detect でマッチすることを保証する。
///
/// 期待値は実測 (2026-09-19): crop offset=(10,0) (左 8px 黒帯 + MARGIN_PX 2)、
/// normalize 1280x724、detect MATCH conf=0.9087 at [8,2] (threshold 0.80)。
/// 固定 fixture に対する決定論的画像処理なので、これらの値が変わるのは
/// 前処理・needle スケール・テンプレ資産のいずれかが変わったとき = 回帰。
#[test]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
fn title_live_shot_matches_login_task_through_production_preprocessing() {
    let fixture = title_live_fixture();
    let raw = image::open(&fixture).unwrap_or_else(|e| {
        panic!(
            "open live title fixture {}: {e} (tests/fixtures/title_live_1952x1098.png is \
             tracked — a fresh clone must contain it)",
            fixture.display()
        )
    });

    // 生キャプチャ不変量: live PrintWindow 1952x1098 (ウィンドウサイズ依存・Issue #208
    // 実測)。この寸法が変わったら取得ウィンドウ状態が変化しており、正規化前提の再検証が必要。
    assert_eq!(
        (raw.width(), raw.height()),
        (1952, 1098),
        "live title fixture must be the 1952x1098 PrintWindow capture (Issue #208 evidence)"
    );

    // 本番経路と同一の前処理: 黒帯クロップ → 基準幅 1280 へ正規化。
    // (PipelineDriver::run_once と同じ crop_to_content_with_info + ScreenScaler::normalize)
    let (cropped, crop_info) = crop_to_content_with_info(&raw);
    // 左端に 8px の黒帯 (ゲーム描画の左インセット) があり、crop が 8+MARGIN_PX(2)=10px
    // 除去する。この crop が働いていること自体が本番経路の前提 (黒帯ごとマッチすると
    // スケールがズレる — letterbox.rs の設計)。
    assert_eq!(
        (crop_info.offset_x, crop_info.offset_y),
        (10, 0),
        "letterbox crop must remove the 10px left inset (8px black bar + MARGIN_PX 2) \
         on this fixture"
    );
    assert_eq!((cropped.width(), cropped.height()), (1942, 1098));
    let normalized = ScreenScaler::new().normalize(&cropped);
    assert_eq!(
        (normalized.width(), normalized.height()),
        (1280, 724),
        "normalize must produce the 1280-base frame for this fixture"
    );

    // 実 TOML から LoginTapTitlePc を読む (roi/threshold 単一情報源は TOML)。
    let login_dir = workspace_root()
        .join("templates")
        .join("pipelines")
        .join("login");
    let tasks = load_pipeline(&login_dir)
        .unwrap_or_else(|e| panic!("load login pipeline {}: {e}", login_dir.display()));
    let task: &TaskDef = tasks
        .iter()
        .find(|t| t.name == "LoginTapTitlePc")
        .expect("LoginTapTitlePc must exist in templates/pipelines/login");

    // roi 付き detect (本番と同一シグネチャ)。Issue #208 恒久修正後のアンカーは
    // title_logo_corner (needle 120x120) + スラック roi [614,253,140,140] (再クロップ
    // 原点 [624,263,120,120] 中心 ±10px)。スラックはキャプチャ間のコンテンツ配置
    // ドリフト (07-07 probe → 本 live fixture で (+2,-1) px) を吸収する — ぴったり
    // ROI だと最良一致位置が roi 外に弾かれ conf 0.99→0.78 に崩壊する (#208 実測)。
    let matched = task
        .detect(&normalized, Path::new(""))
        .unwrap_or_else(|e| panic!("LoginTapTitlePc detect error: {e}"))
        .unwrap_or_else(|| {
            panic!(
                "LoginTapTitlePc must MATCH the live title fixture through the production \
                 preprocessing (crop_to_content -> normalize -> roi detect). Issue #208 \
                 regression: roi={:?} threshold={}",
                task.roi, task.threshold
            )
        });
    assert!(
        matched.confidence.0 >= task.threshold,
        "confidence {} must exceed the TOML threshold {} on the live title fixture",
        matched.confidence.0,
        task.threshold
    );
    // 実測 0.9944 に対する早期劣化検知マージン (threshold 0.80 は本番発火条件、
    // 0.95 は資産・前処理の品質バッファ)。
    assert!(
        matched.confidence.0 >= 0.95,
        "confidence {} degraded well below the measured 0.9944 — the template asset or \
         preprocessing may have regressed (Issue #208)",
        matched.confidence.0
    );
    // 最良一致位置 (スラック roi 内の needle 最良配置。決定論的画像処理なので固定値)。
    assert_eq!(
        (matched.region.x, matched.region.y),
        (637, 268),
        "best placement of the logo_corner needle on this fixture must stay at (637,268) \
         (measured Issue #208); got ({},{})",
        matched.region.x,
        matched.region.y
    );
    // マッチ領域はスラック roi (正規化後空間) に収まること。roi [614,253,140,140] は
    // 1280x724 へ sx=1280/1258, sy=724/708 でスケールされ [625,259,142,143] になる。
    assert_eq!(
        (matched.region.width, matched.region.height),
        (122, 123),
        "needle 120x120 scales to 122x123 in the 1280x724 normalized space"
    );
}

/// version_label テンプレートが「全幅ダーク行」(画面固有の区切り線) を含まないこと。
///
/// Issue #208 の根因: 旧テンプレは生成元画面 (区切り線つきオーバーレイ) の下端 2 行に
/// 全幅ダーク行を含み、純タイトル画面 (区切り線なし) で conf が 0.94 → 0.47 に崩壊
/// した。本テストはテンプレ資産が再び区切り線つき画面から再生成されるのを検出する
/// (title 画面に存在しない UI 要素をアンカーに混ぜない、という資産契約の pin)。
#[test]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
fn version_label_template_is_free_of_full_width_dark_rows() {
    let tpl_path = workspace_root()
        .join("templates")
        .join("scenes")
        .join("title_pc")
        .join("version_label.png");
    let tpl = image::open(&tpl_path).unwrap_or_else(|e| panic!("open {}: {e}", tpl_path.display()));
    let luma = tpl.to_luma8();
    let w = luma.width();

    // 行ごとに「暗い画素 (luma < 100) の割合」を測る。旧テンプレの区切り線行は
    // 138px 全幅がダーク (割合 ~1.0)。現行テンプレの brand バー行は散在するテキスト
    // 程度 (< 0.2)。0.90 のしきい値は区切り線 (連続全幅) とグリフ散在を弁別する。
    const DARK_LUMA: u8 = 100;
    const MAX_DARK_ROW_FRACTION: f64 = 0.90;
    for y in 0..luma.height() {
        let dark = (0..w)
            .filter(|&x| luma.get_pixel(x, y)[0] < DARK_LUMA)
            .count();
        let fraction = dark as f64 / w as f64;
        assert!(
            fraction < MAX_DARK_ROW_FRACTION,
            "version_label.png row {y} is {:.0}% dark pixels — the template appears to \
             contain a full-width dark row (a screen-specific separator line). Issue #208: \
             such rows exist only on non-title screens and collapse ccoeff on the pure \
             title screen (0.94 -> 0.47). Regenerate the template from a real title \
             capture (resize_crop_template --letterbox 10 --roi 8,2,138,20)",
            fraction * 100.0
        );
    }
}
