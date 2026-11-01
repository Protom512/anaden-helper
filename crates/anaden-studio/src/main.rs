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

    #[test]
    fn bundled_font_has_japanese_glyph_for_hiragana_a() {
        let bytes = include_bytes!("../assets/NotoSansJP-Regular.ttf");

        let font = FontRef::try_from_slice(bytes).expect("bundled TTF parse failed");

        let glyph_id = font.glyph_id('あ');
        assert_ne!(
            glyph_id.0, 0,
            "フォントが「あ」(U+3042) のグリフを持ちません (glyph_id={glyph_id:?})。\
             日本語未対応フォントの可能性があります。"
        );

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
