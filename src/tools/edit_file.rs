//! Edit file tool - Modify file contents with search/replace
//!
//! Supports three modes:
//! - `edit`: Modify existing file by replacing text (uses cascading fuzzy matchers)
//! - `create`: Create a new file (must not already exist)
//! - `overwrite`: Replace entire file contents
//!
//! The edit mode uses 9 cascading replacers based on OpenCode's approach:
//! 1. simple_replacer - Exact match
//! 2. line_trimmed_replacer - Trim whitespace per line
//! 3. block_anchor_replacer - First/last line anchors + Levenshtein
//! 4. whitespace_normalized_replacer - Collapse whitespace
//! 5. indentation_flexible_replacer - Normalize indentation
//! 6. escape_normalized_replacer - Handle \n, \t, etc.
//! 7. trimmed_boundary_replacer - Trim block boundaries
//! 8. context_aware_replacer - 50% middle line match
//! 9. multi_occurrence_replacer - All exact matches

use regex::Regex;
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;
use similar::{ChangeTag, TextDiff};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum EditFileError {
    #[error("File not found: {0}")]
    NotFound(String),
    #[error("Permission denied: {0}")]
    PermissionDenied(String),
    #[error("Not a file: {0}")]
    NotAFile(String),
    #[error("IO error: {0}")]
    Io(String),
    #[error("Path is outside working directory: {0}")]
    OutsideWorkDir(String),
    #[error("Old string not found in file")]
    OldStringNotFound,
    #[error("Old string found multiple times. Provide more surrounding lines to identify the correct match.")]
    MultipleMatches,
    #[error("File already exists: {0}. Use mode='edit' or mode='overwrite' instead.")]
    FileAlreadyExists(String),
    #[error("File does not exist: {0}. Use mode='create' instead.")]
    FileDoesNotExist(String),
    #[error("Invalid mode: {0}. Must be 'edit', 'create', or 'overwrite'.")]
    InvalidMode(String),
    #[error("Missing required field: {0}")]
    MissingField(String),
    #[error("old_string and new_string must be different")]
    SameStrings,
}

// Similarity thresholds for block anchor fallback matching
const SINGLE_CANDIDATE_SIMILARITY_THRESHOLD: f64 = 0.0;
const MULTIPLE_CANDIDATES_SIMILARITY_THRESHOLD: f64 = 0.3;

/// Edit mode determines how the file operation is performed
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum EditMode {
    /// Modify existing file by replacing text
    #[default]
    Edit,
    /// Create a new file (must not exist)
    Create,
    /// Replace entire file contents
    Overwrite,
}

#[derive(Debug, Deserialize)]
pub struct EditFileArgs {
    /// The path to the file to edit (relative to working directory)
    pub path: String,

    /// The mode of operation: 'edit', 'create', or 'overwrite'
    #[serde(default)]
    pub mode: EditMode,

    /// A brief description of the edit (for documentation/logging)
    #[serde(default)]
    pub description: Option<String>,

    /// For 'edit' mode: The text to find and replace
    #[serde(default)]
    pub old_string: Option<String>,

    /// For 'edit' mode: The text to replace it with
    #[serde(default)]
    pub new_string: Option<String>,

    /// For 'create' or 'overwrite' mode: The full content to write
    #[serde(default)]
    pub content: Option<String>,

    /// For 'edit' mode: Replace all occurrences (default: false, requires unique match)
    #[serde(default)]
    pub replace_all: bool,
}

// Re-export EditOutput as EditFileOutput for backwards compatibility
pub use super::output::{EditMode as OutputEditMode, EditOutput as EditFileOutput};

// ==================== Cascading Replacers ====================

/// Levenshtein distance algorithm implementation
fn levenshtein(a: &str, b: &str) -> usize {
    // Handle empty strings
    if a.is_empty() || b.is_empty() {
        return a.len().max(b.len());
    }

    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let a_len = a_chars.len();
    let b_len = b_chars.len();

    let mut matrix = vec![vec![0; b_len + 1]; a_len + 1];

    // Initialize first row and column
    for i in 0..=a_len {
        matrix[i][0] = i;
    }
    for j in 0..=b_len {
        matrix[0][j] = j;
    }

    // Fill the matrix
    for i in 1..=a_len {
        for j in 1..=b_len {
            let cost = if a_chars[i - 1] == b_chars[j - 1] {
                0
            } else {
                1
            };
            matrix[i][j] = std::cmp::min(
                std::cmp::min(
                    matrix[i - 1][j] + 1,     // deletion
                    matrix[i][j - 1] + 1,     // insertion
                ),
                matrix[i - 1][j - 1] + cost, // substitution
            );
        }
    }

    matrix[a_len][b_len]
}

/// Simple exact string replacer
fn simple_replacer(content: &str, find: &str) -> Vec<String> {
    if content.contains(find) {
        vec![find.to_string()]
    } else {
        Vec::new()
    }
}

/// Line-trimmed replacer - matches lines ignoring leading/trailing whitespace
fn line_trimmed_replacer(content: &str, find: &str) -> Vec<String> {
    let original_lines: Vec<&str> = content.lines().collect();
    let mut search_lines: Vec<&str> = find.lines().collect();

    // Remove trailing empty line if present
    if search_lines.last() == Some(&"") {
        search_lines.pop();
    }

    for i in 0..=original_lines.len().saturating_sub(search_lines.len()) {
        let mut matches = true;

        for (j, search_line) in search_lines.iter().enumerate() {
            let original_trimmed = original_lines[i + j].trim();
            let search_trimmed = search_line.trim();

            if original_trimmed != search_trimmed {
                matches = false;
                break;
            }
        }

        if matches {
            // Calculate the actual match indices
            let mut match_start_index = 0;
            for k in 0..i {
                match_start_index += original_lines[k].len() + 1; // +1 for newline
            }

            let mut match_end_index = match_start_index;
            for k in 0..search_lines.len() {
                match_end_index += original_lines[i + k].len();
                if k < search_lines.len() - 1 {
                    match_end_index += 1; // Add newline except for last line
                }
            }

            return vec![content[match_start_index..match_end_index].to_string()];
        }
    }

    Vec::new()
}

/// Block anchor replacer - uses first and last lines as anchors with fuzzy middle matching
fn block_anchor_replacer(content: &str, find: &str) -> Vec<String> {
    let original_lines: Vec<&str> = content.lines().collect();
    let mut search_lines: Vec<&str> = find.lines().collect();

    if search_lines.len() < 3 {
        return Vec::new();
    }

    // Remove trailing empty line if present
    if search_lines.last() == Some(&"") {
        search_lines.pop();
    }

    let first_line_search = search_lines[0].trim();
    let last_line_search = search_lines[search_lines.len() - 1].trim();
    let search_block_size = search_lines.len();

    // Collect all candidate positions where both anchors match
    let mut candidates = Vec::new();
    for i in 0..original_lines.len() {
        if original_lines[i].trim() != first_line_search {
            continue;
        }

        // Look for the matching last line after this first line
        for j in (i + 2)..original_lines.len() {
            if original_lines[j].trim() == last_line_search {
                candidates.push((i, j));
                break; // Only match the first occurrence of the last line
            }
        }
    }

    // Return immediately if no candidates
    if candidates.is_empty() {
        return Vec::new();
    }

    // Handle single candidate scenario (using relaxed threshold)
    if candidates.len() == 1 {
        let (start_line, end_line) = candidates[0];
        let actual_block_size = end_line - start_line + 1;

        let mut similarity = 0.0;
        let lines_to_check = (search_block_size - 2).min(actual_block_size - 2); // Middle lines only

        if lines_to_check > 0 {
            for j in 1..(search_block_size - 1).min(actual_block_size - 1) {
                let original_line = original_lines[start_line + j].trim();
                let search_line = search_lines[j].trim();
                let max_len = original_line.len().max(search_line.len());
                if max_len == 0 {
                    continue;
                }
                let distance = levenshtein(original_line, search_line);
                similarity += (1.0 - distance as f64 / max_len as f64) / lines_to_check as f64;

                // Exit early when threshold is reached
                if similarity >= SINGLE_CANDIDATE_SIMILARITY_THRESHOLD {
                    break;
                }
            }
        } else {
            // No middle lines to compare, just accept based on anchors
            similarity = 1.0;
        }

        if similarity >= SINGLE_CANDIDATE_SIMILARITY_THRESHOLD {
            let mut match_start_index = 0;
            for k in 0..start_line {
                match_start_index += original_lines[k].len() + 1;
            }
            let mut match_end_index = match_start_index;
            for k in start_line..=end_line {
                match_end_index += original_lines[k].len();
                if k < end_line {
                    match_end_index += 1; // Add newline except for last line
                }
            }
            return vec![content[match_start_index..match_end_index].to_string()];
        }
        return Vec::new();
    }

    // Calculate similarity for multiple candidates
    let mut best_match: Option<(usize, usize)> = None;
    let mut max_similarity = -1.0;

    for &(start_line, end_line) in &candidates {
        let actual_block_size = end_line - start_line + 1;

        let mut similarity = 0.0;
        let lines_to_check = (search_block_size - 2).min(actual_block_size - 2); // Middle lines only

        if lines_to_check > 0 {
            for j in 1..(search_block_size - 1).min(actual_block_size - 1) {
                let original_line = original_lines[start_line + j].trim();
                let search_line = search_lines[j].trim();
                let max_len = original_line.len().max(search_line.len());
                if max_len == 0 {
                    continue;
                }
                let distance = levenshtein(original_line, search_line);
                similarity += 1.0 - distance as f64 / max_len as f64;
            }
            similarity /= lines_to_check as f64; // Average similarity
        } else {
            // No middle lines to compare, just accept based on anchors
            similarity = 1.0;
        }

        if similarity > max_similarity {
            max_similarity = similarity;
            best_match = Some((start_line, end_line));
        }
    }

    // Threshold judgment
    if max_similarity >= MULTIPLE_CANDIDATES_SIMILARITY_THRESHOLD {
        if let Some((start_line, end_line)) = best_match {
            let mut match_start_index = 0;
            for k in 0..start_line {
                match_start_index += original_lines[k].len() + 1;
            }
            let mut match_end_index = match_start_index;
            for k in start_line..=end_line {
                match_end_index += original_lines[k].len();
                if k < end_line {
                    match_end_index += 1;
                }
            }
            return vec![content[match_start_index..match_end_index].to_string()];
        }
    }

    Vec::new()
}

/// Whitespace-normalized replacer - collapses multiple spaces into single spaces
fn whitespace_normalized_replacer(content: &str, find: &str) -> Vec<String> {
    let normalize_whitespace =
        |text: &str| text.replace(char::is_whitespace, " ").trim().to_string();
    let normalized_find = normalize_whitespace(find);

    // Handle single line matches
    let lines: Vec<&str> = content.lines().collect();
    for line in &lines {
        if normalize_whitespace(line) == normalized_find {
            return vec![line.to_string()];
        } else {
            // Only check for substring matches if the full line doesn't match
            let normalized_line = normalize_whitespace(line);
            if normalized_line.contains(&normalized_find) {
                // Find the actual substring in the original line that matches
                let words: Vec<&str> = find.trim().split_whitespace().collect();
                if !words.is_empty() {
                    let pattern = words
                        .iter()
                        .map(|word| regex::escape(word))
                        .collect::<Vec<_>>()
                        .join("\\s+");
                    if let Ok(regex) = Regex::new(&pattern) {
                        if let Some(matched) = regex.find(line) {
                            return vec![matched.as_str().to_string()];
                        }
                    }
                }
            }
        }
    }

    // Handle multi-line matches
    let find_lines: Vec<&str> = find.lines().collect();
    if find_lines.len() > 1 {
        for i in 0..=lines.len().saturating_sub(find_lines.len()) {
            let block: String = lines[i..i + find_lines.len()].join("\n");
            if normalize_whitespace(&block) == normalized_find {
                return vec![block];
            }
        }
    }

    Vec::new()
}

/// Indentation-flexible replacer - ignores leading indentation
fn indentation_flexible_replacer(content: &str, find: &str) -> Vec<String> {
    let remove_indentation = |text: &str| {
        let lines: Vec<&str> = text.lines().collect();
        let non_empty_lines: Vec<&str> = lines
            .iter()
            .filter(|line| !line.trim().is_empty())
            .copied()
            .collect();
        if non_empty_lines.is_empty() {
            return text.to_string();
        }

        let min_indent = non_empty_lines
            .iter()
            .map(|line| line.find(|c: char| !c.is_whitespace()).unwrap_or(0))
            .min()
            .unwrap_or(0);

        lines
            .iter()
            .map(|line| {
                if line.trim().is_empty() {
                    line.to_string()
                } else {
                    line.chars().skip(min_indent).collect()
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    let normalized_find = remove_indentation(find);
    let content_lines: Vec<&str> = content.lines().collect();
    let find_lines: Vec<&str> = find.lines().collect();

    for i in 0..=content_lines.len().saturating_sub(find_lines.len()) {
        let block: String = content_lines[i..i + find_lines.len()].join("\n");
        if remove_indentation(&block) == normalized_find {
            return vec![block];
        }
    }

    Vec::new()
}

/// Escape-normalized replacer - handles escaped characters
fn escape_normalized_replacer(content: &str, find: &str) -> Vec<String> {
    let unescape_string = |str_: &str| {
        str_.replace("\\n", "\n")
            .replace("\\t", "\t")
            .replace("\\r", "\r")
            .replace("\\'", "'")
            .replace("\\\"", "\"")
            .replace("\\`", "`")
            .replace("\\\\", "\\")
            .replace("\\\n", "\n")
            .replace("\\$", "$")
    };

    let unescaped_find = unescape_string(find);

    // Try direct match with unescaped find string
    if content.contains(&unescaped_find) {
        return vec![unescaped_find];
    }

    // Also try finding escaped versions in content that match unescaped find
    let lines: Vec<&str> = content.lines().collect();
    let find_lines: Vec<&str> = unescaped_find.lines().collect();

    for i in 0..=lines.len().saturating_sub(find_lines.len()) {
        let block: String = lines[i..i + find_lines.len()].join("\n");
        let unescaped_block = unescape_string(&block);

        if unescaped_block == unescaped_find {
            return vec![block];
        }
    }

    Vec::new()
}

/// Trimmed boundary replacer - tries trimmed versions
fn trimmed_boundary_replacer(content: &str, find: &str) -> Vec<String> {
    let trimmed_find = find.trim();

    if trimmed_find == find {
        // Already trimmed, no point in trying
        return Vec::new();
    }

    let mut results = Vec::new();

    // Try to find the trimmed version
    if content.contains(trimmed_find) {
        results.push(trimmed_find.to_string());
    }

    // Also try finding blocks where trimmed content matches
    let lines: Vec<&str> = content.lines().collect();
    let find_lines: Vec<&str> = find.lines().collect();

    for i in 0..=lines.len().saturating_sub(find_lines.len()) {
        let block: String = lines[i..i + find_lines.len()].join("\n");

        if block.trim() == trimmed_find {
            results.push(block);
        }
    }

    results
}

/// Context-aware replacer - uses first and last lines as context anchors
fn context_aware_replacer(content: &str, find: &str) -> Vec<String> {
    let mut find_lines: Vec<&str> = find.lines().collect();
    if find_lines.len() < 3 {
        // Need at least 3 lines to have meaningful context
        return Vec::new();
    }

    // Remove trailing empty line if present
    if find_lines.last() == Some(&"") {
        find_lines.pop();
    }

    let content_lines: Vec<&str> = content.lines().collect();

    // Extract first and last lines as context anchors
    let first_line = find_lines[0].trim();
    let last_line = find_lines[find_lines.len() - 1].trim();

    // Find blocks that start and end with the context anchors
    for i in 0..content_lines.len() {
        if content_lines[i].trim() != first_line {
            continue;
        }

        // Look for the matching last line
        for j in (i + 2)..content_lines.len() {
            if content_lines[j].trim() == last_line {
                // Found a potential context block
                let block_lines = &content_lines[i..=j];
                let block = block_lines.join("\n");

                // Check if the middle content has reasonable similarity
                // (simple heuristic: at least 50% of non-empty lines should match when trimmed)
                if block_lines.len() == find_lines.len() {
                    let mut matching_lines = 0;
                    let mut total_non_empty_lines = 0;

                    for k in 1..block_lines.len() - 1 {
                        let block_line = block_lines[k].trim();
                        let find_line = find_lines[k].trim();

                        if !block_line.is_empty() || !find_line.is_empty() {
                            total_non_empty_lines += 1;
                            if block_line == find_line {
                                matching_lines += 1;
                            }
                        }
                    }

                    if total_non_empty_lines == 0
                        || (matching_lines as f64 / total_non_empty_lines as f64) >= 0.5
                    {
                        return vec![block];
                    }
                }
                break;
            }
        }
    }

    Vec::new()
}

/// Multi-occurrence replacer - finds all exact matches
fn multi_occurrence_replacer(content: &str, find: &str) -> Vec<String> {
    let mut matches = Vec::new();
    let mut start_index = 0;

    while let Some(index) = content[start_index..].find(find) {
        let absolute_index = start_index + index;
        matches.push(find.to_string());
        start_index = absolute_index + find.len();
    }

    matches
}

/// Main replace function that tries all replacers in cascade
fn replace(
    content: &str,
    old_string: &str,
    new_string: &str,
    replace_all: bool,
) -> Result<String, EditFileError> {
    if old_string == new_string {
        return Err(EditFileError::SameStrings);
    }

    let mut not_found = true;

    // Try all replacers in order
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

    for replacer in &replacers {
        for search in replacer(content, old_string) {
            let index = content.find(&search);
            if index.is_none() {
                continue;
            }
            not_found = false;

            if replace_all {
                return Ok(content.replace(&search, new_string));
            }

            let last_index = content.rfind(&search);
            if index != last_index {
                continue; // Multiple matches, try to be more specific
            }

            let index = index.unwrap();
            return Ok(content[..index].to_string()
                + new_string
                + &content[index + search.len()..]);
        }
    }

    if not_found {
        return Err(EditFileError::OldStringNotFound);
    }
    Err(EditFileError::MultipleMatches)
}

// ==================== Tool Implementation ====================

/// Tool for editing files via search and replace
#[derive(Debug, Clone)]
pub struct EditFile {
    working_dir: Arc<PathBuf>,
}

impl EditFile {
    pub fn new(working_dir: PathBuf) -> Self {
        Self {
            working_dir: Arc::new(working_dir),
        }
    }

    fn resolve_path(&self, path: &str) -> Result<PathBuf, EditFileError> {
        let requested = PathBuf::from(path);

        let full_path = if requested.is_absolute() {
            requested
        } else {
            self.working_dir.join(&requested)
        };

        // For new files, we can't canonicalize yet, but we need to check the parent
        if !full_path.exists() {
            // Check parent directory exists and is within working dir
            if let Some(parent) = full_path.parent() {
                if parent.exists() {
                    let parent_canonical = parent.canonicalize().map_err(|e| {
                        EditFileError::Io(format!("Cannot resolve parent directory: {}", e))
                    })?;
                    let working_canonical = self.working_dir.canonicalize().map_err(|e| {
                        EditFileError::Io(format!("Cannot resolve working directory: {}", e))
                    })?;

                    if !parent_canonical.starts_with(&working_canonical) {
                        return Err(EditFileError::OutsideWorkDir(path.to_string()));
                    }
                    return Ok(full_path);
                }
            }
            // Parent doesn't exist - we'll create it for 'create' mode
            return Ok(full_path);
        }

        let canonical = full_path
            .canonicalize()
            .map_err(|e| EditFileError::Io(e.to_string()))?;

        let working_canonical = self
            .working_dir
            .canonicalize()
            .map_err(|e| EditFileError::Io(format!("Cannot resolve working directory: {}", e)))?;

        if !canonical.starts_with(&working_canonical) {
            return Err(EditFileError::OutsideWorkDir(path.to_string()));
        }

        Ok(canonical)
    }

    /// Write content atomically (write to temp file, then rename)
    fn atomic_write(path: &PathBuf, content: &str) -> Result<(), EditFileError> {
        // Create parent directories if needed
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                EditFileError::Io(format!("Failed to create parent directories: {}", e))
            })?;
        }

        // Write to temp file in same directory (for atomic rename)
        let temp_path = path.with_extension("tmp.crow_edit");

        std::fs::write(&temp_path, content).map_err(|e| match e.kind() {
            std::io::ErrorKind::PermissionDenied => {
                EditFileError::PermissionDenied(path.display().to_string())
            }
            _ => EditFileError::Io(e.to_string()),
        })?;

        // Atomic rename
        std::fs::rename(&temp_path, path).map_err(|e| {
            // Clean up temp file on failure
            let _ = std::fs::remove_file(&temp_path);
            EditFileError::Io(format!("Failed to rename temp file: {}", e))
        })?;

        Ok(())
    }
}

impl Serialize for EditFile {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_unit()
    }
}

impl<'de> Deserialize<'de> for EditFile {
    fn deserialize<D>(_deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Ok(Self::new(
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        ))
    }
}

impl Tool for EditFile {
    const NAME: &'static str = "edit_file";

    type Error = EditFileError;
    type Args = EditFileArgs;
    type Output = EditFileOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "edit_file".to_string(),
            description: r#"Performs exact string replacements in files with fuzzy matching fallbacks.

Before using this tool:
1. Use the `read_file` tool to understand the file's contents and context
2. Verify the directory path is correct (only for new files)

The tool uses 9 cascading replacers to handle LLM mistakes gracefully:
- Exact match, line-trimmed, block anchors, whitespace-normalized
- Indentation-flexible, escape-normalized, trimmed boundaries
- Context-aware (50% middle match), multi-occurrence

Usage notes:
- The edit will FAIL if `old_string` is not found in the file
- The edit will FAIL if `old_string` is found multiple times. Provide more context.
- Use `replace_all` to replace all occurrences of a string"#
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the file (relative to project root)"
                    },
                    "old_string": {
                        "type": "string",
                        "description": "The text to find and replace (fuzzy matching enabled)"
                    },
                    "new_string": {
                        "type": "string",
                        "description": "The new text to replace old_string with"
                    },
                    "replace_all": {
                        "type": "boolean",
                        "description": "Replace all occurrences (default: false)"
                    },
                    "mode": {
                        "type": "string",
                        "enum": ["edit", "create", "overwrite"],
                        "description": "Operation mode (default: edit)"
                    },
                    "content": {
                        "type": "string",
                        "description": "Full file content (only for mode='create' or mode='overwrite')"
                    }
                },
                "required": ["path", "old_string", "new_string"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let path = self.resolve_path(&args.path)?;

        match args.mode {
            EditMode::Create => {
                // Create mode: file must not exist
                if path.exists() {
                    return Err(EditFileError::FileAlreadyExists(args.path.clone()));
                }

                // For create mode, content can come from 'content' param or 'new_string' as fallback
                let content = args.content.or(args.new_string.clone()).ok_or_else(|| {
                    EditFileError::MissingField("content (required for mode='create')".to_string())
                })?;

                Self::atomic_write(&path, &content)?;

                let line_count = content.lines().count();

                Ok(EditFileOutput {
                    path: args.path,
                    mode: OutputEditMode::Create,
                    old_content: None,
                    new_content: content,
                    additions: line_count,
                    deletions: 0,
                })
            }

            EditMode::Overwrite => {
                // Overwrite mode: file must exist
                if !path.exists() {
                    return Err(EditFileError::FileDoesNotExist(args.path.clone()));
                }

                if !path.is_file() {
                    return Err(EditFileError::NotAFile(args.path.clone()));
                }

                let old_content = std::fs::read_to_string(&path).map_err(|e| match e.kind() {
                    std::io::ErrorKind::PermissionDenied => {
                        EditFileError::PermissionDenied(args.path.clone())
                    }
                    _ => EditFileError::Io(e.to_string()),
                })?;

                // For overwrite mode, content can come from 'content' param or 'new_string' as fallback
                let new_content = args.content.or(args.new_string.clone()).ok_or_else(|| {
                    EditFileError::MissingField(
                        "content (required for mode='overwrite')".to_string(),
                    )
                })?;

                Self::atomic_write(&path, &new_content)?;

                // Calculate additions and deletions
                let text_diff = TextDiff::from_lines(&old_content, &new_content);
                let (mut additions, mut deletions) = (0, 0);
                for change in text_diff.iter_all_changes() {
                    match change.tag() {
                        ChangeTag::Insert => additions += 1,
                        ChangeTag::Delete => deletions += 1,
                        ChangeTag::Equal => {}
                    }
                }

                Ok(EditFileOutput {
                    path: args.path,
                    mode: OutputEditMode::Overwrite,
                    old_content: Some(old_content),
                    new_content,
                    additions,
                    deletions,
                })
            }

            EditMode::Edit => {
                // Edit mode: search and replace with fuzzy matching
                if !path.exists() {
                    return Err(EditFileError::NotFound(args.path.clone()));
                }

                if !path.is_file() {
                    return Err(EditFileError::NotAFile(args.path.clone()));
                }

                let old_string = args.old_string.ok_or_else(|| {
                    EditFileError::MissingField("old_string (required for mode='edit')".to_string())
                })?;

                let new_string = args.new_string.ok_or_else(|| {
                    EditFileError::MissingField("new_string (required for mode='edit')".to_string())
                })?;

                let old_content = std::fs::read_to_string(&path).map_err(|e| match e.kind() {
                    std::io::ErrorKind::PermissionDenied => {
                        EditFileError::PermissionDenied(args.path.clone())
                    }
                    _ => EditFileError::Io(e.to_string()),
                })?;

                // Use cascading replacers
                let new_content = replace(&old_content, &old_string, &new_string, args.replace_all)?;

                Self::atomic_write(&path, &new_content)?;

                // Calculate additions and deletions
                let text_diff = TextDiff::from_lines(&old_content, &new_content);
                let (mut additions, mut deletions) = (0, 0);
                for change in text_diff.iter_all_changes() {
                    match change.tag() {
                        ChangeTag::Insert => additions += 1,
                        ChangeTag::Delete => deletions += 1,
                        ChangeTag::Equal => {}
                    }
                }

                Ok(EditFileOutput {
                    path: args.path,
                    mode: OutputEditMode::Edit,
                    old_content: Some(old_content),
                    new_content,
                    additions,
                    deletions,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup_test_dir() -> TempDir {
        tempfile::tempdir().unwrap()
    }

    // ==================== Tool integration tests ====================

    #[tokio::test]
    async fn test_create_new_file() {
        let dir = setup_test_dir();
        let tool = EditFile::new(dir.path().to_path_buf());

        let result = tool
            .call(EditFileArgs {
                path: "new_file.txt".to_string(),
                mode: EditMode::Create,
                description: Some("Create test file".to_string()),
                old_string: None,
                new_string: None,
                content: Some("Hello, World!\nLine 2\n".to_string()),
                replace_all: false,
            })
            .await
            .unwrap();

        assert_eq!(result.mode, OutputEditMode::Create);
        assert!(result.new_content.contains("Hello, World!"));
        assert!(result.old_content.is_none());

        let content = std::fs::read_to_string(dir.path().join("new_file.txt")).unwrap();
        assert_eq!(content, "Hello, World!\nLine 2\n");
    }

    #[tokio::test]
    async fn test_create_fails_if_exists() {
        let dir = setup_test_dir();
        std::fs::write(dir.path().join("existing.txt"), "content").unwrap();

        let tool = EditFile::new(dir.path().to_path_buf());

        let result = tool
            .call(EditFileArgs {
                path: "existing.txt".to_string(),
                mode: EditMode::Create,
                description: None,
                old_string: None,
                new_string: None,
                content: Some("new content".to_string()),
                replace_all: false,
            })
            .await;

        assert!(matches!(result, Err(EditFileError::FileAlreadyExists(_))));
    }

    #[tokio::test]
    async fn test_overwrite_existing_file() {
        let dir = setup_test_dir();
        std::fs::write(dir.path().join("file.txt"), "old content").unwrap();

        let tool = EditFile::new(dir.path().to_path_buf());

        let result = tool
            .call(EditFileArgs {
                path: "file.txt".to_string(),
                mode: EditMode::Overwrite,
                description: None,
                old_string: None,
                new_string: None,
                content: Some("new content".to_string()),
                replace_all: false,
            })
            .await
            .unwrap();

        assert_eq!(result.mode, OutputEditMode::Overwrite);
        assert_eq!(result.old_content.as_deref(), Some("old content"));
        assert_eq!(result.new_content, "new content");

        let content = std::fs::read_to_string(dir.path().join("file.txt")).unwrap();
        assert_eq!(content, "new content");
    }

    #[tokio::test]
    async fn test_edit_replace_string() {
        let dir = setup_test_dir();
        std::fs::write(
            dir.path().join("code.rs"),
            "fn old_name() {\n    println!(\"hello\");\n}\n",
        )
        .unwrap();

        let tool = EditFile::new(dir.path().to_path_buf());

        let result = tool
            .call(EditFileArgs {
                path: "code.rs".to_string(),
                mode: EditMode::Edit,
                description: Some("Rename function".to_string()),
                old_string: Some("fn old_name()".to_string()),
                new_string: Some("fn new_name()".to_string()),
                content: None,
                replace_all: false,
            })
            .await
            .unwrap();

        assert_eq!(result.mode, OutputEditMode::Edit);
        assert!(result.old_content.as_ref().unwrap().contains("fn old_name()"));
        assert!(result.new_content.contains("fn new_name()"));

        let content = std::fs::read_to_string(dir.path().join("code.rs")).unwrap();
        assert!(content.contains("fn new_name()"));
    }

    #[tokio::test]
    async fn test_edit_replace_all() {
        let dir = setup_test_dir();
        std::fs::write(dir.path().join("test.txt"), "foo bar foo baz foo").unwrap();

        let tool = EditFile::new(dir.path().to_path_buf());

        tool.call(EditFileArgs {
                path: "test.txt".to_string(),
                mode: EditMode::Edit,
                description: None,
                old_string: Some("foo".to_string()),
                new_string: Some("qux".to_string()),
                content: None,
                replace_all: true,
            })
            .await
            .unwrap();

        let content = std::fs::read_to_string(dir.path().join("test.txt")).unwrap();
        assert_eq!(content, "qux bar qux baz qux");
    }

    #[tokio::test]
    async fn test_create_with_nested_dirs() {
        let dir = setup_test_dir();
        let tool = EditFile::new(dir.path().to_path_buf());

        let result = tool
            .call(EditFileArgs {
                path: "a/b/c/deep.txt".to_string(),
                mode: EditMode::Create,
                description: None,
                old_string: None,
                new_string: None,
                content: Some("deep file".to_string()),
                replace_all: false,
            })
            .await
            .unwrap();

        assert_eq!(result.mode, OutputEditMode::Create);

        let content = std::fs::read_to_string(dir.path().join("a/b/c/deep.txt")).unwrap();
        assert_eq!(content, "deep file");
    }

    // ==================== replace() function tests ====================

    #[test]
    fn test_replace_simple() {
        let content = "Hello World\nHello Rust";
        let result = replace(content, "Hello World", "Hello Crow", false).unwrap();
        assert_eq!(result, "Hello Crow\nHello Rust");
    }

    #[test]
    fn test_replace_all_occurrences() {
        let content = "foo bar\nfoo baz\nfoo qux";
        let result = replace(content, "foo", "bar", true).unwrap();
        assert_eq!(result, "bar bar\nbar baz\nbar qux");
    }

    #[test]
    fn test_replace_single_when_multiple_exists() {
        let content = "foo bar\nfoo baz";
        let result = replace(content, "foo", "bar", false);
        assert!(result.is_err());
    }

    #[test]
    fn test_replace_not_found() {
        let content = "Hello World";
        let result = replace(content, "Goodbye", "Hi", false);
        assert!(matches!(result, Err(EditFileError::OldStringNotFound)));
    }

    #[test]
    fn test_replace_same_string_error() {
        let content = "Hello World";
        let result = replace(content, "Hello", "Hello", false);
        assert!(matches!(result, Err(EditFileError::SameStrings)));
    }

    #[test]
    fn test_replace_multiline() {
        let content = "fn main() {\n    println!(\"Hello\");\n}";
        let result = replace(
            content,
            "fn main() {\n    println!(\"Hello\");\n}",
            "fn main() {\n    println!(\"World\");\n}",
            false,
        )
        .unwrap();
        assert_eq!(result, "fn main() {\n    println!(\"World\");\n}");
    }

    // ==================== Fuzzy matching tests ====================

    #[test]
    fn test_replace_fuzzy_whitespace() {
        let content = "    let x = 5;";
        let result = replace(content, "let x = 5;", "let x = 42;", false).unwrap();
        assert_eq!(result, "    let x = 42;");
    }

    #[test]
    fn test_replace_fuzzy_indentation() {
        let content = "        deeply indented";
        let result = replace(content, "deeply indented", "not so deep", false).unwrap();
        assert_eq!(result, "        not so deep");
    }

    #[test]
    fn test_replace_fuzzy_multiline_indentation() {
        let content = "    fn test() {\n        let x = 1;\n    }";
        let result = replace(
            content,
            "fn test() {\n    let x = 1;\n}",
            "fn test() {\n    let x = 2;\n}",
            false,
        )
        .unwrap();
        assert!(result.contains("let x = 2"));
    }

    #[test]
    fn test_replace_trimmed_boundary() {
        let content = "   hello world   ";
        let result = replace(content, "hello world", "goodbye world", false).unwrap();
        assert!(result.contains("goodbye world"));
    }

    // ==================== Block anchor replacer tests ====================

    #[test]
    fn test_replace_block_anchor() {
        let content = "fn foo() {\n    let x = 1;\n    let y = 2;\n}";
        let result = replace(
            content,
            "fn foo() {\n    // different middle\n}",
            "fn bar() {\n    let z = 3;\n}",
            false,
        );
        assert!(result.is_ok());
    }

    // ==================== Levenshtein distance tests ====================

    #[test]
    fn test_levenshtein_empty_strings() {
        assert_eq!(levenshtein("", ""), 0);
    }

    #[test]
    fn test_levenshtein_one_empty() {
        assert_eq!(levenshtein("", "abc"), 3);
        assert_eq!(levenshtein("abc", ""), 3);
    }

    #[test]
    fn test_levenshtein_identical() {
        assert_eq!(levenshtein("hello", "hello"), 0);
    }

    #[test]
    fn test_levenshtein_classic_examples() {
        assert_eq!(levenshtein("kitten", "sitting"), 3);
        assert_eq!(levenshtein("flaw", "lawn"), 2);
        assert_eq!(levenshtein("saturday", "sunday"), 3);
    }

    // ==================== Individual replacer tests ====================

    #[test]
    fn test_simple_replacer() {
        let content = "hello world";
        let find = "hello";
        let matches = simple_replacer(content, find);
        assert_eq!(matches, vec!["hello"]);
    }

    #[test]
    fn test_line_trimmed_replacer_basic() {
        let content = "    hello world\n    foo bar";
        let find = "hello world\nfoo bar";
        let matches = line_trimmed_replacer(content, find);
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn test_indentation_flexible_replacer() {
        let content = "        deeply indented";
        let find = "deeply indented";
        let matches = indentation_flexible_replacer(content, find);
        assert!(!matches.is_empty());
    }

    #[test]
    fn test_multi_occurrence_replacer() {
        let content = "foo bar foo baz foo";
        let find = "foo";
        let matches = multi_occurrence_replacer(content, find);
        assert_eq!(matches.len(), 3);
    }

    // ==================== Real-world scenario tests ====================

    #[test]
    fn test_replace_rust_struct_field() {
        let content = r#"struct User {
    name: String,
    age: u32,
}"#;
        let result = replace(content, "    age: u32,", "    age: u64,", false).unwrap();
        assert!(result.contains("age: u64"));
    }

    #[test]
    fn test_replace_all_three_occurrences() {
        let content = r#"fn main() {
    println!("Hello, World!");
}

pub fn hello_world() -> String {
    "Hello, World!".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hello_world() {
        assert_eq!(hello_world(), "Hello, World!");
    }
}"#;
        let result = replace(content, "Hello, World!", "Hello, Universe!", true).unwrap();
        assert_eq!(result.matches("Hello, Universe!").count(), 3);
        assert_eq!(result.matches("Hello, World!").count(), 0);
    }

    // ==================== Edge case tests ====================

    #[test]
    fn test_replace_unicode() {
        let content = "Hello 世界!";
        let result = replace(content, "世界", "World", false).unwrap();
        assert_eq!(result, "Hello World!");
    }

    #[test]
    fn test_replace_emoji() {
        let content = "Hello 👋 World";
        let result = replace(content, "👋", "🌍", false).unwrap();
        assert_eq!(result, "Hello 🌍 World");
    }
}
