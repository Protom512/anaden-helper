//! anaden-studio: テンプレート作成GUI。
//!
//! スクリーンショット上でROIを選び、正例/負例画面に対する識別力を
//! リアルタイムに検証しながらテンプレートを作成するツール。
//! 認識方式は VisionEngine trait（現状は正規化SSE）で差し替え可能。

mod app;
mod batch;
mod canvas;
mod library;
mod proposals;
mod scoring;
mod source;

use eframe::egui;

use crate::app::StudioApp;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1200.0, 800.0]),
        ..Default::default()
    };
    eframe::run_native(
        "anaden-studio — テンプレート作成",
        options,
        Box::new(|cc| {
            // egui のデフォルトフォントは日本語グリフを含まないため文字化け（□豆腐）する。
            // かつて .ttc（フォントコレクション）の面選択で実機文字化けが残った実績があるため、
            // 日本語グリフを含むシングルフェースの .ttf をバンドルして include_bytes! で読む。
            setup_japanese_fonts(&cc.egui_ctx);
            Ok(Box::new(StudioApp::default()))
        }),
    )
}

/// フォントの登録名。include_bytes! は文字列リテラルしか受け付けないため、
/// パスは setup_japanese_fonts 内にリテラルで直接記述し、定数側は名前のみ持つ。
const FONT_NAME: &str = "NotoSansJP";

/// egui に日本語フォントを登録する。
///
/// 設計意図（egui 0.34.3 のグリフ処理は skrifa + vello_cpu）:
/// - `include_bytes!` でバイナリにフォントを埋め込み、OS のシステムフォントに依存しない。
/// - シングルフェース .ttf を使う（.ttc は面選択 index 指定で動くが、バンドルは .ttf が単純）。
/// - `FontDefinitions::default()` を起点に「追加」し、内蔵ラテン/絵文字フォントを保持する。
/// - 日本語フォントを Proportional / Monospace の両方で **先頭**（最高優先）に置くことで、
///   日本語が優先され、足りない絵文字等は既存フォントへフォールバックする。
///   Monospace を忘れるとコードブロック内の日本語が □ になる。
fn setup_japanese_fonts(ctx: &egui::Context) {
    let bytes: &'static [u8] = include_bytes!("../assets/NotoSansJP-Regular.ttf");

    let mut fonts = egui::FontDefinitions::default();

    // (1) フォント実体を名前付きで登録。index=0 はシングルフェース .ttf の規約。
    fonts.font_data.insert(
        FONT_NAME.to_owned(),
        std::sync::Arc::new(egui::FontData::from_owned(bytes.to_vec())),
    );

    // (2) Proportional / Monospace 両方で先頭（最高優先）に置く。
    fonts
        .families
        .get_mut(&egui::FontFamily::Proportional)
        .expect("default FontDefinitions must have Proportional")
        .insert(0, FONT_NAME.to_owned());

    fonts
        .families
        .get_mut(&egui::FontFamily::Monospace)
        .expect("default FontDefinitions must have Monospace")
        .insert(0, FONT_NAME.to_owned());

    // (3) 登録。set_fonts は完全上書きなので default() 起点で内蔵フォントを失わない。
    ctx.set_fonts(fonts);
}

#[cfg(test)]
mod tests {
    use ab_glyph::{Font, FontRef};

    // 注意: include_bytes! は文字列リテラルしか受け付けない（定数変数不可）。
    // したがって main.rs の setup_japanese_fonts と同じパスをリテラルで記述する。
    // パスは setup_japanese_fonts 内の include_bytes! と同一であること。

    /// バンドルした .ttf が「あ」(U+3042) のグリフを実際に持つことを検証する。
    /// NOTDEF(0) 以外の glyph_id が返れば、そのフォントで日本語が描画可能。
    /// これが「文字化け解消」の技術的証拠（グリフID != 0）。ビルド成功だけでは判定しない。
    #[test]
    fn bundled_font_has_japanese_glyph_for_hiragana_a() {
        let bytes = include_bytes!("../assets/NotoSansJP-Regular.ttf");

        // .ttf 単体をパース（面選択 index 指定は不要）。
        let font = FontRef::try_from_slice(bytes).expect("bundled TTF parse failed");

        let glyph_id = font.glyph_id('あ');
        assert_ne!(
            glyph_id.0,
            0,
            "フォントが「あ」(U+3042) のグリフを持ちません (glyph_id={glyph_id:?})。\
             日本語未対応フォントの可能性があります。"
        );

        // ひらがな/カタカナ/漢字/長音の代表点も確認。
        for c in ['あ', 'ア', '漢', '字', 'ー'] {
            let id = font.glyph_id(c);
            assert_ne!(
                id.0, 0,
                "フォントが文字 {c:?} (U+{:04X}) のグリフを持ちません (glyph_id={id:?})。",
                c as u32
            );
        }
    }
}
