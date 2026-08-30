//! CLI 引数解析（Issue #119 shard1 タスク3: lib 側テスト可能な形への抽出）。
//!
//! 従来 `main.rs` にあった `parse_args` を純関数化した。起動分岐は
//! 「フラグなし → 統合GUI」「`--pipeline` → 同一の統合GUI（deprecated 警告付き）」
//! に一本化され、`--pipeline` は未知の引数として扱わない。

use crate::source::Target;

/// コマンドライン引数の解析結果。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CliArgs {
    /// キャプチャバックエンド(android|windows)。既定 android。
    pub target: Target,
    /// PC版(Windows)対象プロセスの exe 名。未指定時は GUI 既定値。
    pub exe: Option<String>,
    /// `--pipeline` フラグ（deprecated・後方互換。同一統合GUIを起動する）。
    pub pipeline: bool,
    /// `-h` / `--help` が指定されたか（main でヘルプ表示して終了する）。
    pub help: bool,
}

/// 起動するアプリの種別。
///
/// Issue #119: フラグの有無に関わらず起動するのは単一の統合GUI
/// （全タブ利用可）のみ。`--pipeline` は deprecated 警告の表示要否
/// だけを切り替える。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchKind {
    /// 統合GUI を起動する。
    /// `deprecated_pipeline == true` の場合、GUI 上に deprecated 警告を表示する。
    Unified {
        /// `--pipeline` 経由で起動された（deprecated 警告を表示する）か。
        deprecated_pipeline: bool,
    },
    /// ヘルプを表示して終了する。
    Help,
}

/// 解析済み CLI 引数から起動するアプリ種別を決める純関数。
pub fn launch_kind(args: &CliArgs) -> LaunchKind {
    if args.help {
        return LaunchKind::Help;
    }
    LaunchKind::Unified {
        deprecated_pipeline: args.pipeline,
    }
}

/// ヘルプ文言（`--pipeline` に deprecated 表記を含む）。
pub const HELP_TEXT: &str = "\
anaden-studio — 統合GUI (作成 / バッチ評価 / pipeline 実行)

USAGE: anaden-studio [--target android|windows] [--exe <name>] [--pipeline]

OPTIONS:
  --target <android|windows>  キャプチャバックエンド(既定: android)
      windows は Windows ビルドでのみ有効。Linux では無視されます。
  --exe <name>                Windows バックエンドの対象 exe 名
                              (既定: AnotherEden.exe)
  --pipeline                  [deprecated] 後方互換フラグ。フラグなし起動と
                              同一の統合GUIを起動します (Issue #119)
  -h, --help                  このヘルプを表示
";

/// コマンドライン引数列をパースする純関数（`main.rs` から抽出）。
///
/// - `--target <android|windows>`: キャプチャバックエンド。
/// - `--exe <name>`: Windows バックエンドの対象 exe 名。
/// - `--pipeline`: deprecated フラグ。同一統合GUIを起動（未知の引数扱いしない）。
/// - `-h` / `--help`: ヘルプ要求。
/// - 未知の引数・未知の `--target` 値は無視して継続（従来動作と同一）。
pub fn parse_args_from<I>(args: I) -> CliArgs
where
    I: IntoIterator<Item = String>,
{
    let mut out = CliArgs::default();
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--target" => {
                if let Some(v) = iter.next() {
                    match v.as_str() {
                        "android" => out.target = Target::Android,
                        "windows" => {
                            // Target::Windows は Windows ビルドでのみ存在。
                            #[cfg(windows)]
                            {
                                out.target = Target::Windows;
                            }
                            // Windows 以外のビルドでは android へフォールバック
                            //（バリアントが存在しないため）。
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
                    out.exe = Some(v);
                }
            }
            "--pipeline" => {
                // deprecated (Issue #119): 同一の統合GUIを起動するため、
                // 未知の引数として扱わずフラグのみ記録する。
                out.pipeline = true;
            }
            "-h" | "--help" => {
                out.help = true;
            }
            other => {
                eprintln!("anaden-studio: 未知の引数 \"{other}\" を無視します。");
            }
        }
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    /// UC-1: フラグなし起動 → 統合アプリ（deprecated 警告なし）。
    #[test]
    fn no_flags_launches_unified_without_deprecation() {
        let args = parse_args_from(Vec::<String>::new());
        assert_eq!(
            launch_kind(&args),
            LaunchKind::Unified {
                deprecated_pipeline: false
            }
        );
    }

    /// UC-3: `--pipeline` → 同一の統合アプリ + deprecated 警告表示。
    #[test]
    fn pipeline_flag_launches_same_unified_app_with_deprecation() {
        let args = parse_args_from(["--pipeline".to_string()]);
        assert!(
            args.pipeline,
            "--pipeline は未知の引数扱いではなくフラグ解析される"
        );
        assert_eq!(
            launch_kind(&args),
            LaunchKind::Unified {
                deprecated_pipeline: true
            }
        );
    }

    /// `--target` / `--exe` は従来通り StudioApp 初期値へ伝播される値として保持される。
    #[test]
    fn target_and_exe_propagate_to_args() {
        let args = parse_args_from([
            "--target".to_string(),
            "windows".to_string(),
            "--exe".to_string(),
            "AnotherEden.exe".to_string(),
        ]);
        #[cfg(windows)]
        assert_eq!(args.target, Target::Windows);
        #[cfg(not(windows))]
        assert_eq!(args.target, Target::Android);
        assert_eq!(args.exe.as_deref(), Some("AnotherEden.exe"));
    }

    /// `--target android` の明示指定。
    #[test]
    fn target_android_explicit() {
        let args = parse_args_from(["--target".to_string(), "android".to_string()]);
        assert_eq!(args.target, Target::Android);
    }

    /// ヘルプ指定 → Help 種別（アプリ起動しない）。
    #[test]
    fn help_flag_requests_help() {
        let args = parse_args_from(["--help".to_string()]);
        assert_eq!(launch_kind(&args), LaunchKind::Help);
        let args_h = parse_args_from(["-h".to_string()]);
        assert_eq!(launch_kind(&args_h), LaunchKind::Help);
    }

    /// ヘルプ文言は `--pipeline` を deprecated として案内する。
    #[test]
    fn help_text_marks_pipeline_deprecated() {
        assert!(HELP_TEXT.contains("--pipeline"));
        assert!(HELP_TEXT.contains("deprecated"));
    }

    /// 未知の引数は無視されて起動継続（従来動作）。
    #[test]
    fn unknown_arg_is_ignored_and_app_launches() {
        let args = parse_args_from(["--nonexistent".to_string()]);
        assert_eq!(
            launch_kind(&args),
            LaunchKind::Unified {
                deprecated_pipeline: false
            }
        );
    }
}
