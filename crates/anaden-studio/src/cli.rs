//! CLI 引数解析（Issue #119 shard1 タスク3: lib 側テスト可能な形への抽出）。
//!
//! 従来 `main.rs` にあった `parse_args` を純関数化した。
//! Issue #123 (shard 2): `--pipeline` deprecated フラグは完全削除され、
//! 未知の引数として扱われる。起動はフラグなしの統合GUI のみ。
//! Issue #188: `--target android|windows` も Android 削除により廃止
//! （キャプチャは Win32 固定。未知の引数として扱われる）。

/// コマンドライン引数の解析結果。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CliArgs {
    /// キャプチャバックエンド (Issue #188 で Windows 固定。呼出元互換で残置)。
    pub target: crate::source::Target,
    /// PC版(Windows)対象プロセスの exe 名。未指定時は GUI 既定値。
    pub exe: Option<String>,
    /// `-h` / `--help` が指定されたか（main でヘルプ表示して終了する）。
    pub help: bool,
}

/// 起動するアプリの種別。
///
/// Issue #119/#123: フラグの有無に関わらず起動するのは単一の統合GUI
/// （全タブ利用可）のみ。`--pipeline` は完全削除済み（未知の引数扱い）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchKind {
    /// 統合GUI を起動する。
    Unified,
    /// ヘルプを表示して終了する。
    Help,
}

/// 解析済み CLI 引数から起動するアプリ種別を決める純関数。
pub fn launch_kind(args: &CliArgs) -> LaunchKind {
    if args.help {
        return LaunchKind::Help;
    }
    LaunchKind::Unified
}

/// ヘルプ文言（Issue #123: `--pipeline` 記述は削除済み。Issue #188: `--target` も削除）。
pub const HELP_TEXT: &str = "\
anaden-studio — 統合GUI (作成 / バッチ評価 / pipeline 実行)

USAGE: anaden-studio [--exe <name>]

OPTIONS:
  --exe <name>                PC版(Windows)キャプチャの対象 exe 名
                              (既定: AnotherEden.exe)
  -h, --help                  このヘルプを表示
";

/// コマンドライン引数列をパースする純関数（`main.rs` から抽出）。
///
/// - `--exe <name>`: PC版(Windows)キャプチャの対象 exe 名。
/// - `-h` / `--help`: ヘルプ要求。
/// - 未知の引数（旧 `--pipeline` / `--target` 含む）は無視して継続（従来動作と同一）。
pub fn parse_args_from<I>(args: I) -> CliArgs
where
    I: IntoIterator<Item = String>,
{
    let mut out = CliArgs::default();
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--exe" => {
                if let Some(v) = iter.next() {
                    out.exe = Some(v);
                }
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

    /// UC-1: フラグなし起動 → 統合アプリ。
    #[test]
    fn no_flags_launches_unified() {
        let args = parse_args_from(Vec::<String>::new());
        assert_eq!(launch_kind(&args), LaunchKind::Unified);
    }

    /// Issue #123 (shard 2): `--pipeline` は未知の引数として無視され、
    /// CliArgs には反映されない（フラグ完全削除）。
    #[test]
    fn pipeline_flag_is_now_unknown_arg() {
        let args = parse_args_from(["--pipeline".to_string()]);
        assert_eq!(launch_kind(&args), LaunchKind::Unified);
    }

    /// Issue #188: `--target` も未知の引数として無視される（Android 削除・Win32 固定）。
    #[test]
    fn target_flag_is_now_unknown_arg() {
        let args = parse_args_from(["--target".to_string(), "windows".to_string()]);
        assert_eq!(args.target, crate::source::Target::Windows);
        assert_eq!(launch_kind(&args), LaunchKind::Unified);
    }

    /// `--exe` は StudioApp 初期値へ伝播される値として保持される。
    #[test]
    fn exe_propagates_to_args() {
        let args = parse_args_from(["--exe".to_string(), "AnotherEden.exe".to_string()]);
        assert_eq!(args.exe.as_deref(), Some("AnotherEden.exe"));
        assert_eq!(args.target, crate::source::Target::Windows);
    }

    /// ヘルプ指定 → Help 種別（アプリ起動しない）。
    #[test]
    fn help_flag_requests_help() {
        let args = parse_args_from(["--help".to_string()]);
        assert_eq!(launch_kind(&args), LaunchKind::Help);
        let args_h = parse_args_from(["-h".to_string()]);
        assert_eq!(launch_kind(&args_h), LaunchKind::Help);
    }

    /// Issue #123: ヘルプ文言に `--pipeline` はもう現れない
    /// （Issue #188: `--target` 記述も削除済み）。
    #[test]
    fn help_text_no_longer_mentions_pipeline_nor_target() {
        assert!(!HELP_TEXT.contains("--pipeline"));
        assert!(!HELP_TEXT.contains("--target"));
        assert!(!HELP_TEXT.contains("deprecated"));
    }

    /// 未知の引数は無視されて起動継続（従来動作）。
    #[test]
    fn unknown_arg_is_ignored_and_app_launches() {
        let args = parse_args_from(["--nonexistent".to_string()]);
        assert_eq!(launch_kind(&args), LaunchKind::Unified);
    }
}
