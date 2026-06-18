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
use crate::source::Target;

/// コマンドライン引数。
struct CliArgs {
    /// キャプチャバックエンド(android|windows)。既定 android。
    target: Target,
    /// PC版(Windows)対象プロセスの exe 名。未指定時は GUI 既定値(AnotherEden.exe)。
    exe: Option<String>,
}

/// 手動でコマンドライン引数をパースする(clap 依存を避けるため)。
///
/// 対応フラグ:
/// - `--target <android|windows>`: キャプチャバックエンド(既定 android)。
/// - `--exe <name>`: Windows バックエンドの対象 exe 名。
/// - `-h` / `--help`: ヘルプを表示して終了。
///
/// 後方互換: 引数未指定時は target=android で従来通り。
fn parse_args() -> CliArgs {
    let mut args = CliArgs {
        target: Target::default(),
        exe: None,
    };
    let mut iter = std::env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--target" => {
                if let Some(v) = iter.next() {
                    match v.as_str() {
                        "android" => args.target = Target::Android,
                        "windows" => {
                            // Target::Windows は Windows ビルドでのみ存在。
                            #[cfg(windows)]
                            {
                                args.target = Target::Windows;
                            }
                            // Windows 以外のビルドで --target windows が渡された場合は
                            // android へフォールバック(Target::Windows バリアントが無い)。
                            #[cfg(not(windows))]
                            {
                                eprintln!(
                                    "anaden-studio: このビルドでは windows バックエンドを利用できません。android を使用します。"
                                );
                            }
                        }
                        other => {
                            eprintln!(
                                "anaden-studio: 未知の --target 値 \"{other}\" です。android を使用します。"
                            );
                        }
                    }
                }
            }
            "--exe" => {
                if let Some(v) = iter.next() {
                    args.exe = Some(v);
                }
            }
            "-h" | "--help" => {
                println!("anaden-studio — テンプレート作成GUI");
                println!();
                println!("USAGE: anaden-studio [--target android|windows] [--exe <name>]");
                println!();
                println!("OPTIONS:");
                println!("  --target <android|windows>  キャプチャバックエンド(既定: android)");
                println!("      windows は Windows ビルドでのみ有効。Linux では無視されます。");
                println!("  --exe <name>                Windows バックエンドの対象 exe 名");
                println!("                              (既定: AnotherEden.exe)");
                println!("  -h, --help                  このヘルプを表示");
                std::process::exit(0);
            }
            other => {
                eprintln!("anaden-studio: 未知の引数 \"{other}\" を無視します。");
            }
        }
    }
    args
}

fn main() -> eframe::Result {
    let cli = parse_args();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1200.0, 800.0]),
        ..Default::default()
    };
    eframe::run_native(
        "anaden-studio — テンプレート作成",
        options,
        Box::new(move |cc| {
            // egui のデフォルトフォントは日本語グリフを含まないため文字化け（□豆腐）する。
            // かつて .ttc（フォントコレクション）の面選択で実機文字化けが残った実績があるため、
            // 日本語グリフを含むシングルフェースの .ttf をバンドルして include_bytes! で読む。
            setup_japanese_fonts(&cc.egui_ctx);
            // CLI で指定された target/exe を初期値として StudioApp へ渡す。
            Ok(Box::new(StudioApp::with_initial_target(
                cli.target, cli.exe,
            )))
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
