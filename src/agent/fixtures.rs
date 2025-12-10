//! Fixtures - Humanize tool calls into minimal markdown
//!
//! Converts verbose tool call JSON into concise, human-readable summaries.
//! Used to inject context between turns without token bloat.

use crate::events::{ExecutedToolCall, TurnResult};
use serde_json::Value;

/// Convert a TurnResult into a condensed markdown summary
/// Tool calls come first (in order), then final text response
pub fn humanize_turn(result: &TurnResult) -> String {
    let mut parts = Vec::new();

    // Add humanized tool calls first (preserves execution order)
    for call in &result.tool_calls {
        if let Some(summary) = humanize_tool_call(call) {
            parts.push(summary);
        }
    }

    // Add final text response last
    if let Some(ref text) = result.text {
        let trimmed = trim_text(text, 500);
        parts.push(trimmed);
    }

    parts.join("\n\n")
}

/// Humanize a single tool call
fn humanize_tool_call(call: &ExecutedToolCall) -> Option<String> {
    let args = &call.arguments;
    let output = &call.output;

    match call.name.as_str() {
        "terminal" | "bash" | "shell" => {
            let cmd = args.get("command").and_then(|v| v.as_str())?;
            let out = trim_output(output, 300);
            Some(format!("ran `{}`\n```\n{}\n```", cmd, out))
        }

        "read_file" | "read" => {
            let path = args.get("filePath")
                .or_else(|| args.get("path"))
                .and_then(|v| v.as_str())?;
            let lines = output.lines().count();
            Some(format!("read `{}` ({} lines)", path, lines))
        }

        "edit_file" | "edit" => {
            let path = args.get("path").and_then(|v| v.as_str())?;
            if call.is_error {
                Some(format!("failed to edit `{}`: {}", path, trim_output(output, 100)))
            } else {
                Some(format!("edited `{}`", path))
            }
        }

        "write_file" | "write" => {
            let path = args.get("path").and_then(|v| v.as_str())?;
            Some(format!("wrote `{}`", path))
        }

        "list_directory" | "ls" => {
            let path = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
            let count = output.lines().count();
            Some(format!("listed `{}` ({} items)", path, count))
        }

        "grep" | "search" => {
            let pattern = args.get("pattern").and_then(|v| v.as_str())?;
            let matches = output.lines().count();
            Some(format!("searched `{}` ({} matches)", pattern, matches))
        }

        "find_path" | "find" | "glob" => {
            let pattern = args.get("pattern").and_then(|v| v.as_str())?;
            let found = output.lines().count();
            Some(format!("found {} files matching `{}`", found, pattern))
        }

        "task_complete" => {
            let summary = args
                .get("summary")
                .and_then(|v| v.as_str())
                .unwrap_or(output.as_str());
            Some(format!("completed: {}", trim_text(summary, 200)))
        }

        "thinking" => {
            // Don't include thinking in summary - it's internal
            None
        }

        "fetch" | "web_fetch" => {
            let url = args.get("url").and_then(|v| v.as_str())?;
            Some(format!("fetched `{}`", url))
        }

        "web_search" => {
            let query = args.get("query").and_then(|v| v.as_str())?;
            Some(format!("searched web for `{}`", query))
        }

        "diagnostics" | "lsp_diagnostics" => {
            let count = output.lines().count();
            Some(format!("checked diagnostics ({} issues)", count))
        }

        "todo_write" | "todo_read" => {
            // Skip todo operations in summary
            None
        }

        "now" => {
            // Skip timestamp in summary
            None
        }

        _ => {
            // Unknown tool - generic format
            let args_preview = summarize_args(args);
            if call.is_error {
                Some(format!(
                    "{} failed: {}",
                    call.name,
                    trim_output(output, 100)
                ))
            } else {
                Some(format!("{}({})", call.name, args_preview))
            }
        }
    }
}

/// Summarize args into a short string
fn summarize_args(args: &Value) -> String {
    match args {
        Value::Object(map) => {
            let parts: Vec<String> = map
                .iter()
                .take(2) // max 2 args
                .map(|(k, v)| {
                    let val = match v {
                        Value::String(s) => trim_text(s, 30),
                        Value::Number(n) => n.to_string(),
                        Value::Bool(b) => b.to_string(),
                        _ => "...".to_string(),
                    };
                    format!("{}={}", k, val)
                })
                .collect();
            parts.join(", ")
        }
        _ => "...".to_string(),
    }
}

/// Trim text to max chars, adding ellipsis
fn trim_text(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
    }
}

/// Trim output, keeping first and last lines if too long
fn trim_output(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.len() <= max {
        return s.to_string();
    }

    let lines: Vec<&str> = s.lines().collect();
    if lines.len() <= 6 {
        return format!("{}...", &s[..max.min(s.len())]);
    }

    // Keep first 3 and last 2 lines
    let first: Vec<&str> = lines.iter().take(3).copied().collect();
    let last: Vec<&str> = lines.iter().rev().take(2).rev().copied().collect();

    format!(
        "{}\n  ... ({} lines) ...\n{}",
        first.join("\n"),
        lines.len() - 5,
        last.join("\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_humanize_terminal() {
        let call = ExecutedToolCall {
            id: "1".into(),
            name: "terminal".into(),
            arguments: json!({"command": "ls -la"}),
            output: "total 42\ndrwxr-xr-x 5 user".into(),
            is_error: false,
            duration_ms: 10,
        };
        let result = humanize_tool_call(&call).unwrap();
        assert!(result.starts_with("ran `ls -la`"));
    }

    #[test]
    fn test_humanize_read() {
        let call = ExecutedToolCall {
            id: "1".into(),
            name: "read_file".into(),
            arguments: json!({"path": "src/main.rs"}),
            output: "line1\nline2\nline3".into(),
            is_error: false,
            duration_ms: 5,
        };
        let result = humanize_tool_call(&call).unwrap();
        assert_eq!(result, "read `src/main.rs` (3 lines)");
    }

    #[test]
    fn test_humanize_edit() {
        let call = ExecutedToolCall {
            id: "1".into(),
            name: "edit_file".into(),
            arguments: json!({"path": "foo.rs"}),
            output: "ok".into(),
            is_error: false,
            duration_ms: 5,
        };
        let result = humanize_tool_call(&call).unwrap();
        assert_eq!(result, "edited `foo.rs`");
    }
}
