//! Edit file tool - Modify file contents with search/replace
//!
//! Ported from tools/edit_file.rs with the same 9 cascading fuzzy matchers.
//!
//! Supports three modes:
//! - `edit`: Modify existing file by replacing text (uses cascading fuzzy matchers)
//! - `create`: Create a new file (must not already exist)
//! - `overwrite`: Replace entire file contents

use crate::tool::{Tool, ToolContext, ToolDefinition, ToolResult};
use async_trait::async_trait;

use serde::Deserialize;
use serde_json::json;
use similar::{ChangeTag, TextDiff};
use std::path::PathBuf;

// Similarity thresholds for block anchor fallback matching
const SINGLE_CANDIDATE_SIMILARITY_THRESHOLD: f64 = 0.0;
const MULTIPLE_CANDIDATES_SIMILARITY_THRESHOLD: f64 = 0.3;

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum EditMode {
    #[default]
    Edit,
    Create,
    Overwrite,
}

#[derive(Debug, Deserialize)]
struct Args {
    /// File path - supports both filePath (OpenCode style) and path
    #[serde(alias = "filePath")]
    path: String,
    #[serde(default)]
    mode: EditMode,
    /// Text to find - supports both oldString (OpenCode style) and old_string
    #[serde(default, alias = "oldString")]
    old_string: Option<String>,
    /// Replacement text - supports both newString (OpenCode style) and new_string
    #[serde(default, alias = "newString")]
    new_string: Option<String>,
    #[serde(default)]
    content: Option<String>,
    /// Replace all occurrences - supports both replaceAll and replace_all
    #[serde(default, alias = "replaceAll")]
    replace_all: bool,
}

pub struct EditTool {
    working_dir: PathBuf,
}

impl EditTool {
    pub fn new(working_dir: PathBuf) -> Self {
        Self { working_dir }
    }

    fn resolve_path(&self, path: &str) -> Result<PathBuf, String> {
        let requested = PathBuf::from(path);
        let full_path = if requested.is_absolute() {
            requested
        } else {
            self.working_dir.join(&requested)
        };

        // For new files, check parent is within working dir
        if !full_path.exists() {
            if let Some(parent) = full_path.parent() {
                if parent.exists() {
                    let parent_canonical = parent
                        .canonicalize()
                        .map_err(|e| format!("Cannot resolve parent: {}", e))?;
                    let working_canonical = self
                        .working_dir
                        .canonicalize()
                        .map_err(|e| format!("Cannot resolve working dir: {}", e))?;

                    if !parent_canonical.starts_with(&working_canonical) {
                        return Err(format!("Path outside working directory: {}", path));
                    }
                }
            }
            return Ok(full_path);
        }

        let canonical = full_path
            .canonicalize()
            .map_err(|e| format!("Cannot resolve path: {}", e))?;
        let working_canonical = self
            .working_dir
            .canonicalize()
            .map_err(|e| format!("Cannot resolve working dir: {}", e))?;

        if !canonical.starts_with(&working_canonical) {
            return Err(format!("Path outside working directory: {}", path));
        }

        Ok(canonical)
    }

    fn atomic_write(path: &PathBuf, content: &str) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create directories: {}", e))?;
        }

        let temp_path = path.with_extension("tmp.crow_edit");
        std::fs::write(&temp_path, content)
            .map_err(|e| format!("Failed to write temp file: {}", e))?;

        std::fs::rename(&temp_path, path).map_err(|e| {
            let _ = std::fs::remove_file(&temp_path);
            format!("Failed to rename temp file: {}", e)
        })?;

        Ok(())
    }
}

#[async_trait]
impl Tool for EditTool {
    fn name(&self) -> &str {
        "edit"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "edit".to_string(),
            description: r#"Performs exact string replacements in files.

Usage:
- You must use `read_file` at least once before editing. This tool will error if you attempt an edit without reading the file first.
- When editing text from read_file output, preserve the exact indentation (tabs/spaces) as it appears in the file.
- The edit will FAIL if `oldString` is not found in the file.
- The edit will FAIL if `oldString` is found multiple times. Provide more surrounding context to make it unique, or use `replaceAll: true`.
- Use `replaceAll: true` to replace ALL occurrences (e.g., renaming a variable).

The tool uses fuzzy matching to handle minor whitespace/indentation differences gracefully."#.to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "filePath": {
                        "type": "string",
                        "description": "The path to the file to modify"
                    },
                    "oldString": {
                        "type": "string",
                        "description": "The text to replace"
                    },
                    "newString": {
                        "type": "string",
                        "description": "The text to replace it with (must be different from oldString)"
                    },
                    "replaceAll": {
                        "type": "boolean",
                        "description": "Replace all occurrences of oldString (default: false)",
                        "default": false
                    }
                },
                "required": ["filePath", "oldString", "newString"]
            }),
        }
    }

    async fn execute(&self, args_value: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        if ctx.is_cancelled() {
            return ToolResult::error("Cancelled");
        }

        let args: Args = match serde_json::from_value(args_value) {
            Ok(a) => a,
            Err(e) => return ToolResult::error(format!("Invalid arguments: {}", e)),
        };

        let path = match self.resolve_path(&args.path) {
            Ok(p) => p,
            Err(e) => return ToolResult::error(e),
        };

        match args.mode {
            EditMode::Create => {
                if path.exists() {
                    return ToolResult::error(format!(
                        "File already exists: {}. Use mode='edit' or mode='overwrite'",
                        args.path
                    ));
                }

                let content = args
                    .content
                    .or(args.new_string)
                    .unwrap_or_default();

                if let Err(e) = Self::atomic_write(&path, &content) {
                    return ToolResult::error(e);
                }

                let lines = content.lines().count();
                ToolResult::success(format!(
                    "Created {} ({} lines)",
                    args.path, lines
                ))
            }

            EditMode::Overwrite => {
                if !path.exists() {
                    return ToolResult::error(format!(
                        "File does not exist: {}. Use mode='create'",
                        args.path
                    ));
                }

                if !path.is_file() {
                    return ToolResult::error(format!("Not a file: {}", args.path));
                }

                let old_content = match std::fs::read_to_string(&path) {
                    Ok(c) => c,
                    Err(e) => return ToolResult::error(format!("Failed to read: {}", e)),
                };

                let new_content = args
                    .content
                    .or(args.new_string)
                    .unwrap_or_default();

                if let Err(e) = Self::atomic_write(&path, &new_content) {
                    return ToolResult::error(e);
                }

                let (additions, deletions) = count_diff(&old_content, &new_content);
                ToolResult::success(format!(
                    "Overwrote {} (+{} -{} lines)",
                    args.path, additions, deletions
                ))
            }

            EditMode::Edit => {
                if !path.exists() {
                    return ToolResult::error(format!("File not found: {}", args.path));
                }

                if !path.is_file() {
                    return ToolResult::error(format!("Not a file: {}", args.path));
                }

                let old_string = match args.old_string {
                    Some(s) => s,
                    None => return ToolResult::error("old_string required for mode=edit"),
                };

                let new_string = match args.new_string {
                    Some(s) => s,
                    None => return ToolResult::error("new_string required for mode=edit"),
                };

                if old_string == new_string {
                    return ToolResult::error("old_string and new_string must be different");
                }

                let old_content = match std::fs::read_to_string(&path) {
                    Ok(c) => c,
                    Err(e) => return ToolResult::error(format!("Failed to read: {}", e)),
                };

                let new_content = match replace(&old_content, &old_string, &new_string, args.replace_all) {
                    Ok(c) => c,
                    Err(e) => return ToolResult::error(e),
                };

                if let Err(e) = Self::atomic_write(&path, &new_content) {
                    return ToolResult::error(e);
                }

                let (additions, deletions) = count_diff(&old_content, &new_content);
                ToolResult::success(format!(
                    "Edited {} (+{} -{} lines)",
                    args.path, additions, deletions
                ))
            }
        }
    }
}

// ==================== Diff Helpers ====================

fn count_diff(old: &str, new: &str) -> (usize, usize) {
    let diff = TextDiff::from_lines(old, new);
    let mut additions = 0;
    let mut deletions = 0;
    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Insert => additions += 1,
            ChangeTag::Delete => deletions += 1,
            ChangeTag::Equal => {}
        }
    }
    (additions, deletions)
}

// ==================== Cascading Replacers ====================

/// Levenshtein distance
fn levenshtein(a: &str, b: &str) -> usize {
    if a.is_empty() || b.is_empty() {
        return a.len().max(b.len());
    }

    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let a_len = a_chars.len();
    let b_len = b_chars.len();

    let mut matrix = vec![vec![0; b_len + 1]; a_len + 1];

    for i in 0..=a_len {
        matrix[i][0] = i;
    }
    for j in 0..=b_len {
        matrix[0][j] = j;
    }

    for i in 1..=a_len {
        for j in 1..=b_len {
            let cost = if a_chars[i - 1] == b_chars[j - 1] { 0 } else { 1 };
            matrix[i][j] = std::cmp::min(
                std::cmp::min(matrix[i - 1][j] + 1, matrix[i][j - 1] + 1),
                matrix[i - 1][j - 1] + cost,
            );
        }
    }

    matrix[a_len][b_len]
}

/// 1. Simple exact string replacer
fn simple_replacer(content: &str, find: &str) -> Vec<String> {
    if content.contains(find) {
        vec![find.to_string()]
    } else {
        Vec::new()
    }
}

/// 2. Line-trimmed replacer
fn line_trimmed_replacer(content: &str, find: &str) -> Vec<String> {
    let original_lines: Vec<&str> = content.lines().collect();
    let mut search_lines: Vec<&str> = find.lines().collect();

    if search_lines.last() == Some(&"") {
        search_lines.pop();
    }

    for i in 0..=original_lines.len().saturating_sub(search_lines.len()) {
        let mut matches = true;
        for (j, search_line) in search_lines.iter().enumerate() {
            if original_lines[i + j].trim() != search_line.trim() {
                matches = false;
                break;
            }
        }

        if matches {
            let mut start = 0;
            for k in 0..i {
                start += original_lines[k].len() + 1;
            }
            let mut end = start;
            for k in 0..search_lines.len() {
                end += original_lines[i + k].len();
                if k < search_lines.len() - 1 {
                    end += 1;
                }
            }
            return vec![content[start..end].to_string()];
        }
    }
    Vec::new()
}

/// 3. Block anchor replacer - first/last lines as anchors with fuzzy middle
fn block_anchor_replacer(content: &str, find: &str) -> Vec<String> {
    let original_lines: Vec<&str> = content.lines().collect();
    let mut search_lines: Vec<&str> = find.lines().collect();

    if search_lines.len() < 3 {
        return Vec::new();
    }

    if search_lines.last() == Some(&"") {
        search_lines.pop();
    }

    let first = search_lines[0].trim();
    let last = search_lines[search_lines.len() - 1].trim();

    let mut candidates = Vec::new();
    for i in 0..original_lines.len() {
        if original_lines[i].trim() != first {
            continue;
        }
        for j in (i + 2)..original_lines.len() {
            if original_lines[j].trim() == last {
                candidates.push((i, j));
                break;
            }
        }
    }

    if candidates.is_empty() {
        return Vec::new();
    }

    let threshold = if candidates.len() == 1 {
        SINGLE_CANDIDATE_SIMILARITY_THRESHOLD
    } else {
        MULTIPLE_CANDIDATES_SIMILARITY_THRESHOLD
    };

    let mut best: Option<(usize, usize)> = None;
    let mut max_sim = -1.0;

    for &(start, end) in &candidates {
        let actual_size = end - start + 1;
        let lines_to_check = (search_lines.len() - 2).min(actual_size - 2);

        let similarity = if lines_to_check > 0 {
            let mut sum = 0.0;
            for j in 1..(search_lines.len() - 1).min(actual_size - 1) {
                let orig = original_lines[start + j].trim();
                let search = search_lines[j].trim();
                let max_len = orig.len().max(search.len());
                if max_len > 0 {
                    let dist = levenshtein(orig, search);
                    sum += 1.0 - dist as f64 / max_len as f64;
                }
            }
            sum / lines_to_check as f64
        } else {
            1.0
        };

        if similarity > max_sim {
            max_sim = similarity;
            best = Some((start, end));
        }
    }

    if max_sim >= threshold {
        if let Some((start, end)) = best {
            let mut idx = 0;
            for k in 0..start {
                idx += original_lines[k].len() + 1;
            }
            let start_idx = idx;
            for k in start..=end {
                idx += original_lines[k].len();
                if k < end {
                    idx += 1;
                }
            }
            return vec![content[start_idx..idx].to_string()];
        }
    }

    Vec::new()
}

/// 4. Whitespace-normalized replacer
fn whitespace_normalized_replacer(content: &str, find: &str) -> Vec<String> {
    let normalize = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let normalized_find = normalize(find);

    for line in content.lines() {
        if normalize(line) == normalized_find {
            return vec![line.to_string()];
        }
    }

    let find_lines: Vec<&str> = find.lines().collect();
    let content_lines: Vec<&str> = content.lines().collect();

    if find_lines.len() > 1 {
        for i in 0..=content_lines.len().saturating_sub(find_lines.len()) {
            let block = content_lines[i..i + find_lines.len()].join("\n");
            if normalize(&block) == normalized_find {
                return vec![block];
            }
        }
    }

    Vec::new()
}

/// 5. Indentation-flexible replacer
fn indentation_flexible_replacer(content: &str, find: &str) -> Vec<String> {
    let remove_indent = |text: &str| {
        let lines: Vec<&str> = text.lines().collect();
        let min_indent = lines
            .iter()
            .filter(|l| !l.trim().is_empty())
            .map(|l| l.find(|c: char| !c.is_whitespace()).unwrap_or(0))
            .min()
            .unwrap_or(0);

        lines
            .iter()
            .map(|l| {
                if l.trim().is_empty() {
                    l.to_string()
                } else {
                    l.chars().skip(min_indent).collect()
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    let normalized_find = remove_indent(find);
    let content_lines: Vec<&str> = content.lines().collect();
    let find_lines: Vec<&str> = find.lines().collect();

    for i in 0..=content_lines.len().saturating_sub(find_lines.len()) {
        let block = content_lines[i..i + find_lines.len()].join("\n");
        if remove_indent(&block) == normalized_find {
            return vec![block];
        }
    }

    Vec::new()
}

/// 6. Escape-normalized replacer
fn escape_normalized_replacer(content: &str, find: &str) -> Vec<String> {
    let unescape = |s: &str| {
        s.replace("\\n", "\n")
            .replace("\\t", "\t")
            .replace("\\r", "\r")
            .replace("\\'", "'")
            .replace("\\\"", "\"")
            .replace("\\\\", "\\")
    };

    let unescaped = unescape(find);
    if content.contains(&unescaped) {
        return vec![unescaped];
    }

    Vec::new()
}

/// 7. Trimmed boundary replacer
fn trimmed_boundary_replacer(content: &str, find: &str) -> Vec<String> {
    let trimmed = find.trim();
    if trimmed == find {
        return Vec::new();
    }

    if content.contains(trimmed) {
        return vec![trimmed.to_string()];
    }

    Vec::new()
}

/// 8. Context-aware replacer (50% middle line match)
fn context_aware_replacer(content: &str, find: &str) -> Vec<String> {
    let mut find_lines: Vec<&str> = find.lines().collect();
    if find_lines.len() < 3 {
        return Vec::new();
    }

    if find_lines.last() == Some(&"") {
        find_lines.pop();
    }

    let content_lines: Vec<&str> = content.lines().collect();
    let first = find_lines[0].trim();
    let last = find_lines[find_lines.len() - 1].trim();

    for i in 0..content_lines.len() {
        if content_lines[i].trim() != first {
            continue;
        }

        for j in (i + 2)..content_lines.len() {
            if content_lines[j].trim() == last {
                let block_lines = &content_lines[i..=j];

                if block_lines.len() == find_lines.len() {
                    let mut matching = 0;
                    let mut total = 0;

                    for k in 1..block_lines.len() - 1 {
                        if !block_lines[k].trim().is_empty() || !find_lines[k].trim().is_empty() {
                            total += 1;
                            if block_lines[k].trim() == find_lines[k].trim() {
                                matching += 1;
                            }
                        }
                    }

                    if total == 0 || (matching as f64 / total as f64) >= 0.5 {
                        return vec![block_lines.join("\n")];
                    }
                }
                break;
            }
        }
    }

    Vec::new()
}

/// 9. Multi-occurrence replacer
fn multi_occurrence_replacer(content: &str, find: &str) -> Vec<String> {
    let mut matches = Vec::new();
    let mut start = 0;

    while let Some(idx) = content[start..].find(find) {
        matches.push(find.to_string());
        start += idx + find.len();
    }

    matches
}

/// Main replace function with cascading replacers
fn replace(content: &str, old: &str, new: &str, replace_all: bool) -> Result<String, String> {
    if old == new {
        return Err("old_string and new_string must be different".to_string());
    }

    let replacers: [fn(&str, &str) -> Vec<String>; 9] = [
        simple_replacer,
        line_trimmed_replacer,
        block_anchor_replacer,
        whitespace_normalized_replacer,
        indentation_flexible_replacer,
        escape_normalized_replacer,
        trimmed_boundary_replacer,
        context_aware_replacer,
        multi_occurrence_replacer,
    ];

    let mut not_found = true;

    for replacer in &replacers {
        for search in replacer(content, old) {
            let Some(idx) = content.find(&search) else {
                continue;
            };
            not_found = false;

            if replace_all {
                return Ok(content.replace(&search, new));
            }

            // Check uniqueness
            if content.rfind(&search) != Some(idx) {
                continue; // Multiple matches, try next replacer
            }

            return Ok(format!(
                "{}{}{}",
                &content[..idx],
                new,
                &content[idx + search.len()..]
            ));
        }
    }

    if not_found {
        Err("old_string not found in file".to_string())
    } else {
        Err("old_string found multiple times. Provide more context to identify the correct match.".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;
    use tokio_util::sync::CancellationToken;

    fn test_ctx() -> ToolContext {
        ToolContext::new(PathBuf::from("/tmp"), CancellationToken::new())
    }

    fn setup_dir() -> TempDir {
        tempfile::tempdir().unwrap()
    }

    #[tokio::test]
    async fn test_create_file() {
        let dir = setup_dir();
        let tool = EditTool::new(dir.path().to_path_buf());

        let result = tool
            .execute(
                json!({
                    "path": "new.txt",
                    "mode": "create",
                    "content": "Hello World\n"
                }),
                &test_ctx(),
            )
            .await;

        assert!(!result.is_error);
        assert!(result.output.contains("Created"));

        let content = std::fs::read_to_string(dir.path().join("new.txt")).unwrap();
        assert_eq!(content, "Hello World\n");
    }

    #[tokio::test]
    async fn test_edit_simple() {
        let dir = setup_dir();
        std::fs::write(dir.path().join("test.rs"), "fn old() {}\n").unwrap();

        let tool = EditTool::new(dir.path().to_path_buf());

        let result = tool
            .execute(
                json!({
                    "path": "test.rs",
                    "old_string": "fn old()",
                    "new_string": "fn new()"
                }),
                &test_ctx(),
            )
            .await;

        assert!(!result.is_error);

        let content = std::fs::read_to_string(dir.path().join("test.rs")).unwrap();
        assert!(content.contains("fn new()"));
    }

    #[tokio::test]
    async fn test_edit_fuzzy_indentation() {
        let dir = setup_dir();
        std::fs::write(dir.path().join("test.rs"), "    let x = 1;\n").unwrap();

        let tool = EditTool::new(dir.path().to_path_buf());

        let result = tool
            .execute(
                json!({
                    "path": "test.rs",
                    "old_string": "let x = 1;",
                    "new_string": "let x = 42;"
                }),
                &test_ctx(),
            )
            .await;

        assert!(!result.is_error);

        let content = std::fs::read_to_string(dir.path().join("test.rs")).unwrap();
        assert!(content.contains("    let x = 42;"));
    }

    #[tokio::test]
    async fn test_replace_all() {
        let dir = setup_dir();
        std::fs::write(dir.path().join("test.txt"), "foo bar foo baz foo\n").unwrap();

        let tool = EditTool::new(dir.path().to_path_buf());

        let result = tool
            .execute(
                json!({
                    "path": "test.txt",
                    "old_string": "foo",
                    "new_string": "qux",
                    "replace_all": true
                }),
                &test_ctx(),
            )
            .await;

        assert!(!result.is_error);

        let content = std::fs::read_to_string(dir.path().join("test.txt")).unwrap();
        assert_eq!(content, "qux bar qux baz qux\n");
    }

    #[test]
    fn test_replace_simple() {
        let content = "Hello World";
        let result = replace(content, "World", "Rust", false).unwrap();
        assert_eq!(result, "Hello Rust");
    }

    #[test]
    fn test_replace_not_found() {
        let content = "Hello World";
        let result = replace(content, "xyz", "abc", false);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    #[test]
    fn test_replace_multiple_needs_context() {
        let content = "foo bar\nfoo baz";
        let result = replace(content, "foo", "qux", false);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("multiple"));
    }

    #[test]
    fn test_levenshtein() {
        assert_eq!(levenshtein("kitten", "sitting"), 3);
        assert_eq!(levenshtein("", "abc"), 3);
        assert_eq!(levenshtein("same", "same"), 0);
    }
}
