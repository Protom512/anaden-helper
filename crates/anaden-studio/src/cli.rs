#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CliArgs {
    pub target: crate::source::Target,
    pub exe: Option<String>,
    pub help: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchKind {
    Unified,
    Help,
}

pub fn launch_kind(args: &CliArgs) -> LaunchKind {
    if args.help {
        return LaunchKind::Help;
    }
    LaunchKind::Unified
}

pub const HELP_TEXT: &str = "\
anaden-studio — 統合GUI (作成 / バッチ評価 / pipeline 実行)

USAGE: anaden-studio [--exe <name>]

OPTIONS:
  --exe <name>                PC版(Windows)キャプチャの対象 exe 名
                              (既定: AnotherEden.exe)
  -h, --help                  このヘルプを表示
";

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

    #[test]
    fn no_flags_launches_unified() {
        let args = parse_args_from(Vec::<String>::new());
        assert_eq!(launch_kind(&args), LaunchKind::Unified);
    }

    #[test]
    fn pipeline_flag_is_now_unknown_arg() {
        let args = parse_args_from(["--pipeline".to_string()]);
        assert_eq!(launch_kind(&args), LaunchKind::Unified);
    }

    #[test]
    fn target_flag_is_now_unknown_arg() {
        let args = parse_args_from(["--target".to_string(), "windows".to_string()]);
        assert_eq!(args.target, crate::source::Target::Windows);
        assert_eq!(launch_kind(&args), LaunchKind::Unified);
    }

    #[test]
    fn exe_propagates_to_args() {
        let args = parse_args_from(["--exe".to_string(), "AnotherEden.exe".to_string()]);
        assert_eq!(args.exe.as_deref(), Some("AnotherEden.exe"));
        assert_eq!(args.target, crate::source::Target::Windows);
    }

    #[test]
    fn help_flag_requests_help() {
        let args = parse_args_from(["--help".to_string()]);
        assert_eq!(launch_kind(&args), LaunchKind::Help);
        let args_h = parse_args_from(["-h".to_string()]);
        assert_eq!(launch_kind(&args_h), LaunchKind::Help);
    }

    #[test]
    fn help_text_no_longer_mentions_pipeline_nor_target() {
        assert!(!HELP_TEXT.contains("--pipeline"));
        assert!(!HELP_TEXT.contains("--target"));
        assert!(!HELP_TEXT.contains("deprecated"));
    }

    #[test]
    fn unknown_arg_is_ignored_and_app_launches() {
        let args = parse_args_from(["--nonexistent".to_string()]);
        assert_eq!(launch_kind(&args), LaunchKind::Unified);
    }
}
