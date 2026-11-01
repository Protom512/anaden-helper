use std::path::Path;

use crate::tasks::TaskError;

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
        let anchor = implemented_at.unwrap_or(kind_index);
        ensure_terminated(&mut lines, anchor, newline);
        lines.insert(
            anchor + 1,
            format!("pipeline_dir = \"{pipeline_dir_rel}\"{newline}"),
        );
    }
    Ok(lines.concat())
}

fn is_key_line(line: &str, key: &str) -> bool {
    let t = line.trim_start();
    !t.starts_with('#')
        && t.strip_prefix(key)
            .is_some_and(|rest| rest.trim_start().starts_with('='))
}

fn replace_key_value(line: &str, key: &str, new_value: &str) -> Option<String> {
    let trimmed = line.trim_start();
    let indent = &line[..line.len() - trimmed.len()];
    let after_eq = trimmed.strip_prefix(key)?.trim_start().strip_prefix('=')?;
    let (terminator, value_part) = split_line_terminator(after_eq);
    let comment_start = match find_comment_start(value_part) {
        Some(i) => value_part[..i].trim_end().len(),
        None => value_part.trim_end().len(),
    };
    let comment = value_part.get(comment_start..).unwrap_or("");
    Some(format!("{indent}{key} = {new_value}{comment}{terminator}"))
}

fn split_line_terminator(s: &str) -> (&str, &str) {
    if let Some(rest) = s.strip_suffix("\r\n") {
        ("\r\n", rest)
    } else if let Some(rest) = s.strip_suffix('\n') {
        ("\n", rest)
    } else {
        ("", s)
    }
}

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

fn ensure_terminated(lines: &mut [String], index: usize, newline: &str) {
    if let Some(line) = lines.get_mut(index)
        && !line.ends_with('\n')
    {
        line.push_str(newline);
    }
}
