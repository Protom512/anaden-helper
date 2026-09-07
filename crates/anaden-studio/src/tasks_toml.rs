//! タスク TOML の外科的行編集 (surgical line edit) 純粋関数群
//! (Issue #160 UC-3 / Issue #162 Shard 2: tasks.rs 分割)。
//!
//! implemented フリップ・pipeline_dir 書き戻しを、コメント行・インライン
//! コメント・キー順・CRLF 改行を保全する最小変更の行編集で適用する
//! (文字列 → 文字列の純粋変換のみ・本モジュール内に I/O 無し)。
//! ファイル I/O・load 検証 (fail-closed) のオーケストレーションは
//! [`crate::tasks::enable_task`] が担う。
//!
//! 本モジュールの関数は crate 内利用限定 (pub(crate) / private) で、
//! 外部公開しない (Issue #162 Shard 2 の facade 条件)。

use std::path::Path;

use crate::tasks::TaskError;

/// pipeline_dir 相対パスを TOML 値として書ける形へ正規化する。
///
/// - 前後の空白を除去し、backslash 区切りを TOML 規約の forward slash へ変える。
/// - 空文字・`"`・改行を含む場合は TOML 基本文字列に埋め込めないため
///   [`TaskError::InvalidPipelineDir`] で fail-closed。
pub(crate) fn normalize_pipeline_dir_rel(rel: &str) -> Result<String, TaskError> {
    let normalized = rel.trim().replace('\\', "/");
    if normalized.is_empty()
        || normalized.contains('"')
        || normalized.contains('\n')
        || normalized.contains('\r')
    {
        return Err(TaskError::InvalidPipelineDir {
            dir: rel.to_string(),
        });
    }
    Ok(normalized)
}

/// タスク TOML ソースへ「implemented = true フリップ」「pipeline_dir 書き戻し」を
/// 最小変更の行編集で適用する。
///
/// コメント行・未編集行はバイト単位で素通しし、対象キー行は値部分のみ置換する
/// (インラインコメント・行末改行を保存)。未宣言キーは `kind` 行をアンカーに
/// その直後へ (implemented → pipeline_dir の順で) 挿入する。
pub(crate) fn edit_task_toml_source(
    source: &str,
    pipeline_dir_rel: &str,
    path: &Path,
) -> Result<String, TaskError> {
    let newline = if source.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut lines: Vec<String> = Vec::new();
    let mut kind_at: Option<usize> = None;
    let mut implemented_at: Option<usize> = None;
    let mut pipeline_dir_at: Option<usize> = None;

    for line in source.split_inclusive('\n') {
        if kind_at.is_none() && is_key_line(line, "kind") {
            lines.push(line.to_string());
            kind_at = Some(lines.len() - 1);
        } else if implemented_at.is_none() && is_key_line(line, "implemented") {
            let replaced =
                replace_key_value(line, "implemented", "true").unwrap_or_else(|| line.to_string());
            lines.push(replaced);
            implemented_at = Some(lines.len() - 1);
        } else if pipeline_dir_at.is_none() && is_key_line(line, "pipeline_dir") {
            let replaced =
                replace_key_value(line, "pipeline_dir", &format!("\"{pipeline_dir_rel}\""))
                    .unwrap_or_else(|| line.to_string());
            lines.push(replaced);
            pipeline_dir_at = Some(lines.len() - 1);
        } else {
            lines.push(line.to_string());
        }
    }

    let Some(kind_index) = kind_at else {
        return Err(TaskError::EditFailed {
            path: path.to_path_buf(),
            reason: "no `kind` line to anchor the edit".to_string(),
        });
    };
    if implemented_at.is_none() {
        ensure_terminated(&mut lines, kind_index, newline);
        lines.insert(kind_index + 1, format!("implemented = true{newline}"));
        implemented_at = Some(kind_index + 1);
    }
    if pipeline_dir_at.is_none() {
        // implemented を挿入済みなら必ず Some (直上で代入)、既存行のみの
        // 場合も kind 行以降のアンカーへ挿入する。
        let anchor = implemented_at.unwrap_or(kind_index);
        ensure_terminated(&mut lines, anchor, newline);
        lines.insert(
            anchor + 1,
            format!("pipeline_dir = \"{pipeline_dir_rel}\"{newline}"),
        );
    }
    Ok(lines.concat())
}

/// 行がトップレベルの `key = ...` 行か (コメント行は除外・前方空白は許容)。
fn is_key_line(line: &str, key: &str) -> bool {
    let t = line.trim_start();
    !t.starts_with('#')
        && t.strip_prefix(key)
            .is_some_and(|rest| rest.trim_start().starts_with('='))
}

/// `key = <旧値> [# コメント]` 行の値部分のみを `new_value` へ置換する
/// (インデント・インラインコメント (直前空白込み)・行末改行を保存)。
/// `key = ...` 行でない場合は `None`。
fn replace_key_value(line: &str, key: &str, new_value: &str) -> Option<String> {
    let trimmed = line.trim_start();
    let indent = &line[..line.len() - trimmed.len()];
    let after_eq = trimmed.strip_prefix(key)?.trim_start().strip_prefix('=')?;
    let (terminator, value_part) = split_line_terminator(after_eq);
    let comment_start = match find_comment_start(value_part) {
        // コメント直前の空白 (整形) も含めて保全する。
        Some(i) => value_part[..i].trim_end().len(),
        None => value_part.trim_end().len(),
    };
    let comment = value_part.get(comment_start..).unwrap_or("");
    Some(format!("{indent}{key} = {new_value}{comment}{terminator}"))
}

/// 行末の改行 (CRLF/LF/無し) を分離する。
fn split_line_terminator(s: &str) -> (&str, &str) {
    if let Some(rest) = s.strip_suffix("\r\n") {
        ("\r\n", rest)
    } else if let Some(rest) = s.strip_suffix('\n') {
        ("\n", rest)
    } else {
        ("", s)
    }
}

/// 値部分内のインラインコメント開始位置 (`#` の位置) を返す。
/// TOML 基本文字列の引用符内の `#` は無視する。
fn find_comment_start(s: &str) -> Option<usize> {
    let mut in_quotes = false;
    let mut escaped = false;
    for (i, ch) in s.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' if in_quotes => escaped = true,
            '"' => in_quotes = !in_quotes,
            '#' if !in_quotes => return Some(i),
            _ => {}
        }
    }
    None
}

/// `lines[index]` が改行終端でない場合 (ファイル末尾行への挿入時) は改行を補う。
fn ensure_terminated(lines: &mut [String], index: usize, newline: &str) {
    if let Some(line) = lines.get_mut(index)
        && !line.ends_with('\n')
    {
        line.push_str(newline);
    }
}
