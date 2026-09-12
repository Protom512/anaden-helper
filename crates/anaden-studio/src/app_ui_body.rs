//! StudioApp の描画エントリ (Issue #174: app_ui.rs 分割)。
//!
//! eframe::App impl・modebar (モード切替バー)・モード本体
//! ([`StudioApp::render_body`]) のディスパッチを持つ。Authoring 本体は
//! [`crate::app_ui_authoring`]、Tasks ペインは [`crate::app_ui_tasks`]、
//! Batch は [`crate::app_ui_batch`]。

use eframe::egui;

use crate::app_state::{AppMode, StudioApp};

impl eframe::App for StudioApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.render_modebar(ui);
        self.render_body(ui);
    }
}

impl StudioApp {
    /// ウィンドウ上限のモード切替バー（modebar）を描画する。
    ///
    /// 親レイアウト内への埋め込み（単一ウィンドウ統合 GUI, Issue #119）を想定した
    /// 公開パネル描画 API。単体テストからは [`Self::mode`] / [`Self::set_mode`]
    /// 経由で振る舞いを検証する。
    pub fn render_modebar(&mut self, ui: &mut egui::Ui) {
        // モード切替バー
        egui::Panel::top("modebar")
            .exact_size(30.0)
            .show_inside(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.mode, AppMode::Authoring, "作成");
                    ui.selectable_value(&mut self.mode, AppMode::LiveAuthoring, "実演オーサリング");
                    ui.selectable_value(&mut self.mode, AppMode::Batch, "バッチ評価");
                });
            });
    }

    /// モード本体（Authoring / Batch）を親レイアウト内に描画する埋め込み用 API。
    ///
    /// modebar は含まない。呼び出し前に [`Self::render_modebar`] を実行するか、
    /// 親シェル側でタブ切替してもよい（mode は [`Self::set_mode`] で制御）。
    ///
    /// スクリーンショットのテクスチャ生成はここ（描画パスの入口）で行う。
    /// かつて [`Self::render_modebar`] 内にあったが、統合GUI シェル
    /// （Issue #119 `UnifiedShell`）は [`Self::render_body`] のみを呼ぶため、
    /// テクスチャが生成されずキャンバスが永久に空になる欠陥があった
    /// （作成タブで画像を開いても何も表示されない）。
    pub fn render_body(&mut self, ui: &mut egui::Ui) {
        // スクリーンショットのテクスチャ生成（未生成時）— 描画パスの入口で必ず走る。
        // 統合GUIシェル (UnifiedShell) 経由でも render_body は呼ばれるため、
        // どの起動経路でもキャンバスに画像が表示される (Issue #120 欠陥1修正)。
        if self.screenshot_tex.is_none()
            && let Some(img) = &self.screenshot
        {
            let rgba = img.to_rgba8();
            let size = [rgba.width() as usize, rgba.height() as usize];
            let color_image = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
            self.screenshot_tex = Some(ui.ctx().load_texture(
                "studio-screenshot",
                color_image,
                egui::TextureOptions::default(),
            ));
        }

        if matches!(self.mode, AppMode::Authoring) {
            self.render_authoring(ui);
        } else if matches!(self.mode, AppMode::LiveAuthoring) {
            self.render_live_authoring(ui);
        } else {
            self.batch_ui(ui);
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    /// ヘッドレス egui コンテキストを用意し、その中に子 Ui を作る。
    /// GUI バックエンド不要でパネル描画を単体テストできる。
    fn child_ui(ctx: &egui::Context) -> egui::Ui {
        egui::Ui::new(
            ctx.clone(),
            egui::Id::new("test-area"),
            egui::UiBuilder::new().max_rect(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800.0, 600.0),
            )),
        )
    }

    /// 埋め込み描画 API（render_modebar + render_body）が Authoring モードで
    /// パニックせず完了することを検証する（Issue #119 shard 1 task 2）。
    #[test]
    fn embed_render_authoring_mode_completes_without_panic() {
        let ctx = egui::Context::default();
        let mut app = StudioApp::default();
        assert_eq!(app.mode(), AppMode::Authoring);
        ctx.begin_pass(egui::RawInput::default());
        app.render_modebar(&mut child_ui(&ctx));
        app.render_body(&mut child_ui(&ctx));
        let _ = ctx.end_pass();
    }

    /// 埋め込み描画 API が Batch モード（混同行列 UI 含む）でも壊れないことを検証する。
    #[test]
    fn embed_render_batch_mode_completes_without_panic() {
        let ctx = egui::Context::default();
        let mut app = StudioApp::default();
        app.set_mode(AppMode::Batch);
        assert_eq!(app.mode(), AppMode::Batch);
        ctx.begin_pass(egui::RawInput::default());
        app.render_modebar(&mut child_ui(&ctx));
        app.render_body(&mut child_ui(&ctx));
        let _ = ctx.end_pass();
    }

    /// 接続バッジ・チェックボタン・エラー理由パネルを含む Authoring 描画が
    /// パニックせず完了すること (Issue #139 T3)。
    #[test]
    fn embed_render_connection_panel_completes_without_panic() {
        let ctx = egui::Context::default();
        let mut app = StudioApp::default();
        app.run_connection_check();
        ctx.begin_pass(egui::RawInput::default());
        app.render_modebar(&mut child_ui(&ctx));
        app.render_body(&mut child_ui(&ctx));
        let _ = ctx.end_pass();
    }

    /// 実演オーサリングモード (Issue #190 Shard 2) の埋め込み描画が
    /// パニックせず完了すること (セッション未開始 + スクリーンショット無しの
    /// 初期状態。ライブビューのガイド表示へ到達する)。
    #[test]
    fn embed_render_live_authoring_mode_completes_without_panic() {
        let ctx = egui::Context::default();
        let mut app = StudioApp::default();
        app.set_mode(AppMode::LiveAuthoring);
        ctx.begin_pass(egui::RawInput::default());
        app.render_modebar(&mut child_ui(&ctx));
        app.render_body(&mut child_ui(&ctx));
        let _ = ctx.end_pass();
    }

    /// UI のボタンラベルに Unicode 絵文字が残っていないこと (豆腐排除・機械検証)。
    #[test]
    fn app_button_labels_contain_no_emoji() {
        let labels = [
            "作成",
            "実演オーサリング",
            "バッチ評価",
            "スクリーンショットを開く",
            "正例フォルダ",
            "負例フォルダ",
            "停止（この画面で固定）",
            "ライブ開始",
            "ROI候補を提案",
            "テンプレート保存",
            "保存先変更",
            "記録開始",
            "元に戻す",
            "シナリオ保存",
            "実行",
        ];
        for l in labels {
            assert!(
                l.chars().all(|c| c < '\u{1F300}'),
                "label must not contain emoji: {l}"
            );
        }
    }
}
