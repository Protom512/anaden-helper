//! Issue #160 AC-6: 実アプリビューの headless E2E 証跡 PNG 生成テスト。
//!
//! 実窓 (ゲーム foreground 保持) の対話自動化が阻害されているため、AC-6 の
//! E2E 証跡は **実アプリコード** (`UnifiedShell` / `StudioApp::render_task_list` /
//! `StudioApp::render_body` / `ScenarioPanel::ui` / `ScenarioPanel::ui_task_link`)
//! を headless egui でフル描画し、ラスタライズした PNG 3 種で示す:
//!
//! 1. `home-view.png`            — ホーム (実 `templates/tasks/` 由来タスク一覧・
//!    未実装タスクのグレー表示込み)
//! 2. `tools-scenario-panel.png` — ツール (作成) ビュー + 「シナリオ作成」
//!    collapsing 展開状態
//! 3. `task-link-ui.png`         — 保存済みシナリオ (テンポラリ dir へ
//!    `save_scenario`) + 「タスク登録・有効化」UI
//!
//! ## ラスタライズ手法 (採用理由)
//!
//! egui_kittest 0.34 (公式テストツール) の snapshot PNG は wgpu (GPU) レンダラを
//! 要求するため、GPU 無し CI で常に走るテストには不採用。代わりに egui 公式 API
//! の描画出力 (`Context::run` → `FullOutput.shapes` → `Context::tessellate` →
//! `ClippedPrimitive` 三角形メッシュ) をテスト内のソフトウェアラスタライザで
//! 描画する。色空間・合成は egui の GPU レンダラと同一契約
//! (頂点色 = sRGBA premultiplied / ブレンド = gamma 空間 `src + dst*(1-src_a)` /
//! テクスチャは bilinear サンプリング) に従う。フォントは main.rs
//! `setup_japanese_fonts` と同一の埋め込み NotoSansJP を登録するため、
//! 日本語グリフが実描画される (豆腐にならない)。
//!
//! ## 証跡の出力先
//!
//! 環境変数 `ANADEN_E2E_EVIDENCE_DIR` (指定時) または `target/e2e-evidence/`
//! (既定・リポジトリを汚さない)。各 PNG は 10KB 超 (空白でない) を機械検証する。
//! 併せて AccessKit ツリーで「シナリオ作成」「タスク登録・有効化」ウィジェットの
//! bounds が画面内にあることを機械検証する (clipping された widget は bounds が
//! 画面外になるため、パネルが実際に見えていることの証明になる)。

#![allow(clippy::unwrap_used)]
#![allow(clippy::panic)]
#![allow(clippy::expect_used)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anaden_core::{Goal, ScreenRegion, StopCondition};
use anaden_studio::app::{PipelineActionKind, pipeline_task_spec};
use anaden_studio::scenario_task_link::{TaskLinkContext, stub_options};
use anaden_studio::scenario_ui::ScenarioPanel;
use anaden_studio::shell::{ToolsSection, UnifiedMode, UnifiedPane, UnifiedShell};
use anaden_studio::source::Target;
use anaden_studio::tasks::TaskListState;
use image::{DynamicImage, GrayImage, Luma, Rgba, RgbaImage};

/// main.rs `setup_japanese_fonts` と同一のフォント登録名。
const FONT_NAME: &str = "NotoSansJP";

/// PNG が「空白でない」ことを示す最小ファイルサイズ (AC-6 証跡の下限)。
const MIN_PNG_BYTES: u64 = 10_000;

/// workspace ルート (app.rs `StudioApp::workspace_root` と同一の解決)。
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

/// 実リポジトリのタスク定義ディレクトリ。
fn repo_tasks_dir() -> PathBuf {
    workspace_root().join("templates").join("tasks")
}

/// 証跡 PNG の出力ディレクトリ (`ANADEN_E2E_EVIDENCE_DIR` or `target/e2e-evidence`)。
fn evidence_dir() -> PathBuf {
    std::env::var_os("ANADEN_E2E_EVIDENCE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace_root().join("target").join("e2e-evidence"))
}

/// main.rs `setup_japanese_fonts` と同一手順で日本語フォントを登録する
/// (include_bytes! は tests/ 基準の相対パスで同一 TTF を埋め込む)。
fn setup_japanese_fonts(ctx: &egui::Context) {
    let bytes: &'static [u8] = include_bytes!("../assets/NotoSansJP-Regular.ttf");
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        FONT_NAME.to_owned(),
        std::sync::Arc::new(egui::FontData::from_owned(bytes.to_vec())),
    );
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        if let Some(list) = fonts.families.get_mut(&family) {
            list.insert(0, FONT_NAME.to_owned());
        }
    }
    ctx.set_fonts(fonts);
}

/// 1 ビュー分の headless 描画 + ソフトウェアラスタライズ。
struct Harness {
    ctx: egui::Context,
    /// マネージドテクスチャの実体 (フォントアトラス等。TexturesDelta を適用)。
    textures: HashMap<egui::TextureId, egui::ColorImage>,
    /// 画面サイズ (論理ポイント)。
    size: egui::Vec2,
    /// pixels per point (2.0 で高解像度証跡)。
    ppp: f32,
    /// 仮想時刻 (collapsing アニメーション完了保障のためパス毎に進める)。
    time: f64,
}

impl Harness {
    fn new(width: f32, height: f32, ppp: f32) -> Self {
        let ctx = egui::Context::default();
        setup_japanese_fonts(&ctx);
        // 証跡検証に AccessKit ツリーを使う (bounds は points 座標系)。
        ctx.enable_accesskit();
        Self {
            ctx,
            textures: HashMap::new(),
            size: egui::vec2(width, height),
            ppp,
            time: 0.0,
        }
    }

    /// 1 フレーム分の UI を構築してラスタライズ結果と egui 出力を返す。
    /// `draw` にはルート `Ui` (画面全域) が渡るので実アプリのパネル描画を呼ぶ。
    fn pass(&mut self, mut draw: impl FnMut(&mut egui::Ui)) -> (RgbaImage, egui::FullOutput) {
        self.time += 0.25;
        let mut input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, self.size)),
            time: Some(self.time),
            ..egui::RawInput::default()
        };
        input.viewports.insert(
            egui::ViewportId::ROOT,
            egui::ViewportInfo {
                native_pixels_per_point: Some(self.ppp),
                ..egui::ViewportInfo::default()
            },
        );
        let output = self.ctx.run_ui(input, |ui| draw(ui));
        self.apply_textures(&output.textures_delta);
        let png = self.rasterize(&output);
        (png, output)
    }

    /// テクスチャ差分 (新規全体 + 部分パッチ + 解放) を保持テクスチャへ適用する。
    fn apply_textures(&mut self, delta: &egui::TexturesDelta) {
        for (id, image_delta) in &delta.set {
            let egui::ImageData::Color(img) = &image_delta.image;
            match (self.textures.get_mut(id), image_delta.pos) {
                (Some(tex), Some([x, y])) => {
                    for row in 0..img.size[1] {
                        for col in 0..img.size[0] {
                            if let Some(dst) =
                                tex.pixels.get_mut((y + row) * tex.size[0] + (x + col))
                            {
                                *dst = img.pixels[row * img.size[0] + col];
                            }
                        }
                    }
                }
                _ => {
                    self.textures.insert(*id, (**img).clone());
                }
            }
        }
        for id in &delta.free {
            self.textures.remove(id);
        }
    }

    /// tessellate 後の三角形メッシュをソフトウェアラスタライズして PNG を得る。
    fn rasterize(&self, output: &egui::FullOutput) -> RgbaImage {
        let ppp = output.pixels_per_point;
        let clipped = self.ctx.tessellate(output.shapes.clone(), ppp);
        let w = (self.size.x * ppp).round() as usize;
        let h = (self.size.y * ppp).round() as usize;
        // 背景 = 実アプリの visual panel_fill (不透明 → 最終 alpha は全域 255)。
        let bg = self.ctx.global_style().visuals.panel_fill;
        let mut buf = vec![bg; w.saturating_mul(h)];
        for cp in &clipped {
            let egui::epaint::Primitive::Mesh(mesh) = &cp.primitive else {
                continue; // PaintCallback は本ビューでは使われない
            };
            let Some(tex) = self.textures.get(&mesh.texture_id) else {
                continue;
            };
            if tex.size[0] == 0 || tex.size[1] == 0 {
                continue;
            }
            let clip = (
                (cp.clip_rect.min.x * ppp).max(0.0),
                (cp.clip_rect.min.y * ppp).max(0.0),
                (cp.clip_rect.max.x * ppp).min(w as f32),
                (cp.clip_rect.max.y * ppp).min(h as f32),
            );
            for tri in mesh.indices.chunks_exact(3) {
                let vs = [
                    mesh.vertices[tri[0] as usize],
                    mesh.vertices[tri[1] as usize],
                    mesh.vertices[tri[2] as usize],
                ];
                draw_triangle(&mut buf, w, h, clip, vs, tex, ppp);
            }
        }
        let mut img = RgbaImage::new(w as u32, h as u32);
        for (px, c) in img.pixels_mut().zip(buf.iter()) {
            // 背景が不透明のため premultiplied == straight。
            *px = Rgba([c.r(), c.g(), c.b(), c.a()]);
        }
        img
    }
}

/// 1 三角形をラスタライズする (bilinear テクスチャ + premultiplied 合成)。
///
/// egui の GPU レンダラと同一契約: 頂点色は sRGBA premultiplied、ブレンドは
/// gamma 空間 `src + dst * (1 - src_a)`、テクスチャ乗算は premultiplied 同士。
fn draw_triangle(
    buf: &mut [egui::Color32],
    w: usize,
    h: usize,
    clip: (f32, f32, f32, f32),
    vs: [egui::epaint::Vertex; 3],
    tex: &egui::ColorImage,
    ppp: f32,
) {
    let p = vs.map(|v| egui::vec2(v.pos.x * ppp, v.pos.y * ppp));
    let min_x = p
        .iter()
        .map(|q| q.x)
        .fold(f32::INFINITY, f32::min)
        .floor()
        .max(clip.0);
    let max_x = p
        .iter()
        .map(|q| q.x)
        .fold(f32::NEG_INFINITY, f32::max)
        .ceil()
        .min(clip.2);
    let min_y = p
        .iter()
        .map(|q| q.y)
        .fold(f32::INFINITY, f32::min)
        .floor()
        .max(clip.1);
    let max_y = p
        .iter()
        .map(|q| q.y)
        .fold(f32::NEG_INFINITY, f32::max)
        .ceil()
        .min(clip.3);
    if min_x >= max_x || min_y >= max_y {
        return;
    }
    let x0 = min_x.max(0.0) as usize;
    let y0 = min_y.max(0.0) as usize;
    let x1 = (max_x.ceil() as usize).min(w);
    let y1 = (max_y.ceil() as usize).min(h);
    let area = (p[1].x - p[0].x) * (p[2].y - p[0].y) - (p[1].y - p[0].y) * (p[2].x - p[0].x);
    if area.abs() < f32::EPSILON {
        return; // 面積 0 の三角形 (egui は巻き順不定のため符号は問わない)
    }
    let [tw, th] = [tex.size[0] as f32, tex.size[1] as f32];
    for y in y0..y1 {
        for x in x0..x1 {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            if px < clip.0 || px >= clip.2 || py < clip.1 || py >= clip.3 {
                continue;
            }
            // 重心座標 (符号付き面積で割るため巻き順に依存しない)。
            let b0 = ((p[1].x - px) * (p[2].y - py) - (p[1].y - py) * (p[2].x - px)) / area;
            let b1 = ((p[2].x - px) * (p[0].y - py) - (p[2].y - py) * (p[0].x - px)) / area;
            let b2 = 1.0 - b0 - b1;
            if b0 < 0.0 || b1 < 0.0 || b2 < 0.0 {
                continue;
            }
            let uv = b0 * vs[0].uv.to_vec2() + b1 * vs[1].uv.to_vec2() + b2 * vs[2].uv.to_vec2();
            let rgba = |v: &egui::epaint::Vertex| {
                [
                    v.color.r() as f32,
                    v.color.g() as f32,
                    v.color.b() as f32,
                    v.color.a() as f32,
                ]
            };
            let (c0, c1, c2) = (rgba(&vs[0]), rgba(&vs[1]), rgba(&vs[2]));
            let t = [
                b0 * c0[0] + b1 * c1[0] + b2 * c2[0],
                b0 * c0[1] + b1 * c1[1] + b2 * c2[1],
                b0 * c0[2] + b1 * c1[2] + b2 * c2[2],
                b0 * c0[3] + b1 * c1[3] + b2 * c2[3],
            ];
            let s = sample_bilinear(tex, uv.x * tw, uv.y * th);
            // premultiplied 同士の乗算 (GPU シェーダの texture * v_color と同一)。
            let src = [
                s[0] * t[0] / 255.0,
                s[1] * t[1] / 255.0,
                s[2] * t[2] / 255.0,
                s[3] * t[3] / 255.0,
            ];
            let inv_a = 1.0 - src[3] / 255.0;
            let dst = buf[y * w + x];
            let d = [
                dst.r() as f32,
                dst.g() as f32,
                dst.b() as f32,
                dst.a() as f32,
            ];
            let out = [
                src[0] + d[0] * inv_a,
                src[1] + d[1] * inv_a,
                src[2] + d[2] * inv_a,
                src[3] + d[3] * inv_a,
            ];
            buf[y * w + x] = egui::Color32::from_rgba_premultiplied(
                out[0].clamp(0.0, 255.0) as u8,
                out[1].clamp(0.0, 255.0) as u8,
                out[2].clamp(0.0, 255.0) as u8,
                out[3].clamp(0.0, 255.0) as u8,
            );
        }
    }
}

/// テクセル中心が `i + 0.5` になるようオフセットした bilinear サンプリング。
fn sample_bilinear(tex: &egui::ColorImage, fx: f32, fy: f32) -> [f32; 4] {
    let x = (fx - 0.5).clamp(0.0, (tex.size[0] - 1) as f32);
    let y = (fy - 0.5).clamp(0.0, (tex.size[1] - 1) as f32);
    let (xi, yi) = (x.floor() as usize, y.floor() as usize);
    let (xj, yj) = ((xi + 1).min(tex.size[0] - 1), (yi + 1).min(tex.size[1] - 1));
    let (tx, ty) = (x - xi as f32, y - yi as f32);
    let at = |xx: usize, yy: usize| -> [f32; 4] {
        let c = tex.pixels[yy * tex.size[0] + xx];
        [c.r() as f32, c.g() as f32, c.b() as f32, c.a() as f32]
    };
    let (c00, c10) = (at(xi, yi), at(xj, yi));
    let (c01, c11) = (at(xi, yj), at(xj, yj));
    let mut out = [0.0; 4];
    for i in 0..4 {
        let a = c00[i] + (c10[i] - c00[i]) * tx;
        let b = c01[i] + (c11[i] - c01[i]) * tx;
        out[i] = a + (b - a) * ty;
    }
    out
}

/// PNG を証跡ディレクトリへ保存し、空白でない (10KB 超) ことを機械検証する。
fn save_evidence(img: &RgbaImage, name: &str) -> PathBuf {
    let dir = evidence_dir();
    std::fs::create_dir_all(&dir).expect("create evidence dir");
    let path = dir.join(name);
    img.save(&path).expect("save evidence PNG");
    let len = std::fs::metadata(&path)
        .expect("evidence PNG metadata")
        .len();
    assert!(
        len > MIN_PNG_BYTES,
        "{name} が空白の可能性がある (file size = {len} bytes, 要求 > {MIN_PNG_BYTES})"
    );
    path
}

/// AccessKit: 指定ラベルのウィジェットが存在し、かつ bounds が画面内にある
/// ことを機械検証する (clipping された widget は bounds が画面外になる)。
fn assert_widget_on_screen(output: &egui::FullOutput, label: &str, size: egui::Vec2) {
    let update = output
        .platform_output
        .accesskit_update
        .as_ref()
        .expect("accesskit update (ctx.enable_accesskit 済み)");
    let mut found = false;
    for (_, node) in &update.nodes {
        let name = node.label().or_else(|| node.value()).unwrap_or("");
        if !name.contains(label) {
            continue;
        }
        let Some(b) = node.bounds() else {
            continue;
        };
        assert!(
            b.x0 >= -0.5
                && b.y0 >= -0.5
                && b.x1 <= size.x as f64 + 0.5
                && b.y1 <= size.y as f64 + 0.5,
            "widget {label:?} が画面外に配置されている: bounds=({:.0},{:.0})-({:.0},{:.0}) 画面={}x{}",
            b.x0,
            b.y0,
            b.x1,
            b.y1,
            size.x,
            size.y
        );
        found = true;
    }
    assert!(found, "AccessKit ツリーに widget {label:?} が見つからない");
}

/// テスト用 TaskDef + 単色 crop PNG (`app::pipeline_task_spec` 経由 = Authoring
/// ペインの追加候補と同一導出。`scenario_editor_tests.rs` と同構成)。
fn gui_candidate(name: &str, roi: ScreenRegion) -> (anaden_vision::TaskDef, DynamicImage) {
    let task = pipeline_task_spec(
        name,
        "field",
        "ccoeff",
        roi,
        0.85,
        PipelineActionKind::ClickSelf,
    )
    .expect("valid spec");
    let crop = solid_image(roi.width, roi.height, 160);
    (task, crop)
}

/// 単色 GrayImage (テンプレート PNG の代替)。
fn solid_image(w: u32, h: u32, v: u8) -> DynamicImage {
    DynamicImage::ImageLuma8(GrayImage::from_pixel(w, h, Luma([v])))
}

// ---- 1. ホームビュー (タスク一覧・実 templates/tasks 由来) ----

#[test]
fn home_view_evidence_png() {
    // 前提: 実リポジトリのタスク定義に未実装 (グレー表示対象) が含まれる。
    let defs = TaskListState::load(&repo_tasks_dir()).expect("load real task defs");
    assert!(
        defs.definitions().iter().any(|d| !d.implemented),
        "未実装タスク (グレー表示対象) が実定義に含まれる必要がある"
    );

    let mut shell = UnifiedShell::new(Target::default(), None);
    assert_eq!(shell.mode(), UnifiedMode::Home, "既定はホーム");
    assert_eq!(shell.pane(), UnifiedPane::Tasks);

    let mut h = Harness::new(1280.0, 800.0, 2.0);
    // 2 パス: 1 パス目でタスク定義読込・レイアウト確定、2 パス目を証跡化。
    let _ = h.pass(|ui| {
        shell.render_modebar(ui);
        shell.render_content(ui);
    });
    let (png, output) = h.pass(|ui| {
        shell.render_modebar(ui);
        shell.render_content(ui);
    });
    assert_widget_on_screen(&output, "タスク一覧", h.size);
    let path = save_evidence(&png, "home-view.png");
    eprintln!("home view evidence: {}", path.display());
}

// ---- 2. ツールビュー + シナリオ作成パネル (default_open 展開状態) ----

#[test]
fn tools_scenario_panel_evidence_png() {
    let mut shell = UnifiedShell::new(Target::default(), None);
    shell.set_mode(UnifiedMode::Tools);
    assert_eq!(shell.tools_section(), ToolsSection::Authoring);
    assert_eq!(shell.pane(), UnifiedPane::Studio);

    // Authoring 左パネルの全コンテンツ (データ/エンジン/キャプチャ/ROI/識別力/
    // 保存/pipeline task 保存/シナリオ作成) が画面内に収まる高さ。
    let mut h = Harness::new(1500.0, 1350.0, 2.0);
    let mut last = None;
    for _ in 0..3 {
        last = Some(h.pass(|ui| {
            shell.render_modebar(ui);
            shell.render_tools_sectionbar(ui);
            shell.render_content(ui);
        }));
    }
    let (png, output) = last.expect("rendered");
    // 「シナリオ作成」collapsing ヘッダとその本体の保存ボタンが画面内にある
    // (= 展開状態で見えている) ことを機械保証。
    assert_widget_on_screen(&output, "シナリオ作成", h.size);
    assert_widget_on_screen(&output, "シナリオ保存", h.size);
    let path = save_evidence(&png, "tools-scenario-panel.png");
    eprintln!("tools scenario panel evidence: {}", path.display());
}

// ---- 3. タスク登録・有効化 UI (保存済みシナリオ + ui_task_link) ----

#[test]
fn task_link_ui_evidence_png() {
    let tmp = tempfile::tempdir().expect("tempdir");
    // テンポラリ workspace を組み、実リポジトリを一切書き換えない:
    // - templates/tasks/ に実定義 TOML をコピー (stub 一覧 = 実データ)
    // - templates/pipelines/ へ save_scenario (テンポラリ dir)
    let tasks_dir = tmp.path().join("templates").join("tasks");
    std::fs::create_dir_all(&tasks_dir).expect("mkdir tasks");
    for entry in std::fs::read_dir(repo_tasks_dir()).expect("read repo tasks dir") {
        let entry = entry.expect("dir entry");
        if entry.path().extension().and_then(|e| e.to_str()) == Some("toml") {
            std::fs::copy(entry.path(), tasks_dir.join(entry.file_name())).expect("copy task");
        }
    }
    let pipelines_root = tmp.path().join("templates").join("pipelines");

    // (a) シナリオ作成 → 保存 (保存実体が記録され ui_task_link が見える状態)。
    let mut panel = ScenarioPanel::new(pipelines_root.clone());
    panel.state.name = "fishing2".to_string();
    let (tap, tap_png) = gui_candidate("TapA", ScreenRegion::new(10, 20, 100, 50));
    panel.add_candidate(tap, tap_png).expect("add TapA");
    let (wait, wait_png) = gui_candidate("WaitB", ScreenRegion::new(5, 5, 40, 40));
    panel.add_candidate(wait, wait_png).expect("add WaitB");
    panel.state.task_mut("TapA").unwrap().next = Some(vec!["WaitB".to_string()]);
    panel.state.add_goal(Goal {
        name: "loop5".to_string(),
        stop: StopCondition::LoopCount { target: 5 },
    });
    let mut status = String::new();
    panel.save(&mut status);
    assert!(status.contains("シナリオ保存"), "status: {status}");
    assert!(
        panel.saved_pipeline().is_some(),
        "保存済み実体が記録されている (ui_task_link 表示条件)"
    );

    // stub 一覧はコピーした実定義から導出 (app.rs の配線と同一)。
    let defs = TaskListState::load(&tasks_dir).expect("load copied task defs");
    let stubs = stub_options(defs.definitions());
    assert!(!stubs.is_empty(), "紐付け先候補 (未実装タスク) が存在する");

    let mut h = Harness::new(1000.0, 1150.0, 2.0);
    let mut last = None;
    for _ in 0..3 {
        last = Some(h.pass(|ui| {
            // 実 ScenarioPanel (ScenarioPanel::ui + ui_task_link) の実レンダリング。
            // 見出しは文脈表示のみで、描画本体はすべて実アプリコード。
            ui.heading("anaden-studio ツール → 作成: シナリオ作成 (実 ScenarioPanel)");
            ui.separator();
            panel.ui(ui, None, &mut status);
            ui.separator();
            let link_ctx = TaskLinkContext {
                root: tmp.path(),
                tasks_dir: &tasks_dir,
                stubs: &stubs,
            };
            let _ = panel.ui_task_link(ui, &link_ctx, &mut status);
            ui.separator();
            ui.label(format!("status: {status}"));
        }));
    }
    let (png, output) = last.expect("rendered");
    assert_widget_on_screen(&output, "タスク登録・有効化", h.size);
    assert_widget_on_screen(&output, "新規タスクとして登録・有効化", h.size);
    let path = save_evidence(&png, "task-link-ui.png");
    eprintln!("task link ui evidence: {}", path.display());
}
