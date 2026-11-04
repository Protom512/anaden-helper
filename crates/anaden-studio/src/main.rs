use anaden_studio::cli::{CliArgs, HELP_TEXT, LaunchKind, launch_kind, parse_args_from};
use anaden_studio::shell::{UNIFIED_WINDOW_TITLE, UnifiedShell};

use eframe::egui;

fn parse_args() -> CliArgs {
    parse_args_from(std::env::args().skip(1))
}

fn main() -> eframe::Result {
    let cli = parse_args();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1200.0, 800.0]),
        ..Default::default()
    };
    match launch_kind(&cli) {
        LaunchKind::Help => {
            print!("{HELP_TEXT}");
            Ok(())
        }
        LaunchKind::Unified => eframe::run_native(
            UNIFIED_WINDOW_TITLE,
            options,
            Box::new(move |cc| {
                setup_japanese_fonts(&cc.egui_ctx);
                Ok(Box::new(UnifiedShell::new(cli.exe)))
            }),
        ),
    }
}

const FONT_NAME: &str = "NotoSansJP";

fn setup_japanese_fonts(ctx: &egui::Context) {
    let bytes: &'static [u8] = include_bytes!("../assets/NotoSansJP-Regular.ttf");

    let mut fonts = egui::FontDefinitions::default();

    fonts.font_data.insert(
        FONT_NAME.to_owned(),
        std::sync::Arc::new(egui::FontData::from_owned(bytes.to_vec())),
    );

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
            glyph_id.0, 0,
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
