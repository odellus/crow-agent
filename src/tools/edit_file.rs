//! Edit file tool - Modify file contents with search/replace
//!
//! Supports three modes:
//! - `edit`: Modify existing file by replacing text (old_string must exist and be unique)
//! - `create`: Create a new file (must not already exist)
//! - `overwrite`: Replace entire file contents

use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;
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
    #[error("Old string found multiple times ({0} occurrences). Use replace_all=true or provide more context.")]
    MultipleMatches(usize),
    #[error("File already exists: {0}. Use mode='edit' or mode='overwrite' instead.")]
    FileAlreadyExists(String),
    #[error("File does not exist: {0}. Use mode='create' instead.")]
    FileDoesNotExist(String),
    #[error("Invalid mode: {0}. Must be 'edit', 'create', or 'overwrite'.")]
    InvalidMode(String),
    #[error("Missing required field: {0}")]
    MissingField(String),
}

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

/// Output from edit_file including a unified diff
#[derive(Debug, Serialize)]
pub struct EditFileOutput {
    pub path: String,
    pub mode: String,
    pub message: String,
    pub diff: String,
}

impl std::fmt::Display for EditFileOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}\n\n```diff\n{}\n```", self.message, self.diff)
    }
}

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

        let canonical = full_path.canonicalize().map_err(|e| {
            EditFileError::Io(e.to_string())
        })?;

        let working_canonical = self.working_dir.canonicalize().map_err(|e| {
            EditFileError::Io(format!("Cannot resolve working directory: {}", e))
        })?;

        if !canonical.starts_with(&working_canonical) {
            return Err(EditFileError::OutsideWorkDir(path.to_string()));
        }

        Ok(canonical)
    }

    /// Generate a unified diff between old and new content
    fn generate_diff(old: &str, new: &str, path: &str) -> String {
        use std::fmt::Write;

        let old_lines: Vec<&str> = old.lines().collect();
        let new_lines: Vec<&str> = new.lines().collect();

        let mut diff = String::new();
        writeln!(diff, "--- a/{}", path).unwrap();
        writeln!(diff, "+++ b/{}", path).unwrap();

        // Simple line-by-line diff (not optimal but readable)
        let mut old_idx = 0;
        let mut new_idx = 0;
        let mut hunk_start_old = 0;
        let mut hunk_start_new = 0;
        let mut hunk_lines: Vec<String> = Vec::new();
        let mut in_hunk = false;

        while old_idx < old_lines.len() || new_idx < new_lines.len() {
            let old_line = old_lines.get(old_idx);
            let new_line = new_lines.get(new_idx);

            match (old_line, new_line) {
                (Some(o), Some(n)) if o == n => {
                    if in_hunk {
                        hunk_lines.push(format!(" {}", o));
                    }
                    old_idx += 1;
                    new_idx += 1;
                }
                (Some(o), Some(n)) => {
                    if !in_hunk {
                        in_hunk = true;
                        hunk_start_old = old_idx + 1;
                        hunk_start_new = new_idx + 1;
                        // Add context before
                        let context_start = old_idx.saturating_sub(3);
                        for i in context_start..old_idx {
                            hunk_lines.push(format!(" {}", old_lines[i]));
                        }
                        if context_start < old_idx {
                            hunk_start_old = context_start + 1;
                            hunk_start_new = new_idx.saturating_sub(old_idx - context_start) + 1;
                        }
                    }
                    hunk_lines.push(format!("-{}", o));
                    hunk_lines.push(format!("+{}", n));
                    old_idx += 1;
                    new_idx += 1;
                }
                (Some(o), None) => {
                    if !in_hunk {
                        in_hunk = true;
                        hunk_start_old = old_idx + 1;
                        hunk_start_new = new_idx + 1;
                    }
                    hunk_lines.push(format!("-{}", o));
                    old_idx += 1;
                }
                (None, Some(n)) => {
                    if !in_hunk {
                        in_hunk = true;
                        hunk_start_old = old_idx + 1;
                        hunk_start_new = new_idx + 1;
                    }
                    hunk_lines.push(format!("+{}", n));
                    new_idx += 1;
                }
                (None, None) => break,
            }
        }

        if !hunk_lines.is_empty() {
            let old_count = hunk_lines.iter().filter(|l| l.starts_with('-') || l.starts_with(' ')).count();
            let new_count = hunk_lines.iter().filter(|l| l.starts_with('+') || l.starts_with(' ')).count();
            writeln!(diff, "@@ -{},{} +{},{} @@", hunk_start_old, old_count, hunk_start_new, new_count).unwrap();
            for line in hunk_lines {
                writeln!(diff, "{}", line).unwrap();
            }
        }

        diff
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

        std::fs::write(&temp_path, content).map_err(|e| {
            match e.kind() {
                std::io::ErrorKind::PermissionDenied => {
                    EditFileError::PermissionDenied(path.display().to_string())
                }
                _ => EditFileError::Io(e.to_string()),
            }
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
        Ok(Self::new(std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))))
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
            description: r#"Edit a file by finding and replacing text.

To edit a file, you MUST provide:
- path: the file path
- old_string: the EXACT text to find (copy from the file)
- new_string: the text to replace it with

The old_string must match exactly including whitespace and indentation.
Returns a unified diff showing the changes made.

IMPORTANT: Always read the file first before editing!"#.to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the file (relative to project root)"
                    },
                    "old_string": {
                        "type": "string",
                        "description": "The EXACT text to find and replace (copy from file, include whitespace)"
                    },
                    "new_string": {
                        "type": "string",
                        "description": "The new text to replace old_string with"
                    },
                    "replace_all": {
                        "type": "boolean",
                        "description": "Replace all occurrences if old_string appears multiple times (default: false)"
                    },
                    "mode": {
                        "type": "string",
                        "enum": ["edit", "create", "overwrite"],
                        "description": "Operation mode (default: edit). Use 'create' for new files with 'content' param."
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
                let content = args.content
                    .or(args.new_string.clone())
                    .ok_or_else(|| {
                        EditFileError::MissingField("content (required for mode='create')".to_string())
                    })?;

                Self::atomic_write(&path, &content)?;

                let diff = Self::generate_diff("", &content, &args.path);
                let line_count = content.lines().count();

                Ok(EditFileOutput {
                    path: args.path,
                    mode: "create".to_string(),
                    message: format!("Created file ({} lines)", line_count),
                    diff,
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

                let old_content = std::fs::read_to_string(&path).map_err(|e| {
                    match e.kind() {
                        std::io::ErrorKind::PermissionDenied => {
                            EditFileError::PermissionDenied(args.path.clone())
                        }
                        _ => EditFileError::Io(e.to_string()),
                    }
                })?;

                // For overwrite mode, content can come from 'content' param or 'new_string' as fallback
                let new_content = args.content
                    .or(args.new_string.clone())
                    .ok_or_else(|| {
                        EditFileError::MissingField("content (required for mode='overwrite')".to_string())
                    })?;

                Self::atomic_write(&path, &new_content)?;

                let diff = Self::generate_diff(&old_content, &new_content, &args.path);

                Ok(EditFileOutput {
                    path: args.path,
                    mode: "overwrite".to_string(),
                    message: "Replaced file contents".to_string(),
                    diff,
                })
            }

            EditMode::Edit => {
                // Edit mode: search and replace
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

                let old_content = std::fs::read_to_string(&path).map_err(|e| {
                    match e.kind() {
                        std::io::ErrorKind::PermissionDenied => {
                            EditFileError::PermissionDenied(args.path.clone())
                        }
                        _ => EditFileError::Io(e.to_string()),
                    }
                })?;

                // Count occurrences
                let count = old_content.matches(&old_string).count();

                if count == 0 {
                    return Err(EditFileError::OldStringNotFound);
                }

                if count > 1 && !args.replace_all {
                    return Err(EditFileError::MultipleMatches(count));
                }

                // Perform replacement
                let new_content = if args.replace_all {
                    old_content.replace(&old_string, &new_string)
                } else {
                    old_content.replacen(&old_string, &new_string, 1)
                };

                Self::atomic_write(&path, &new_content)?;

                let diff = Self::generate_diff(&old_content, &new_content, &args.path);
                let replacements = if args.replace_all { count } else { 1 };

                Ok(EditFileOutput {
                    path: args.path,
                    mode: "edit".to_string(),
                    message: format!(
                        "Edited file ({} replacement{})",
                        replacements,
                        if replacements == 1 { "" } else { "s" }
                    ),
                    diff,
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

    #[tokio::test]
    async fn test_create_new_file() {
        let dir = setup_test_dir();
        let tool = EditFile::new(dir.path().to_path_buf());

        let result = tool.call(EditFileArgs {
            path: "new_file.txt".to_string(),
            mode: EditMode::Create,
            description: Some("Create test file".to_string()),
            old_string: None,
            new_string: None,
            content: Some("Hello, World!\nLine 2\n".to_string()),
            replace_all: false,
        }).await.unwrap();

        assert_eq!(result.mode, "create");
        assert!(result.message.contains("Created"));
        assert!(result.diff.contains("+Hello, World!"));

        // Verify file was created
        let content = std::fs::read_to_string(dir.path().join("new_file.txt")).unwrap();
        assert_eq!(content, "Hello, World!\nLine 2\n");
    }

    #[tokio::test]
    async fn test_create_fails_if_exists() {
        let dir = setup_test_dir();
        std::fs::write(dir.path().join("existing.txt"), "content").unwrap();

        let tool = EditFile::new(dir.path().to_path_buf());

        let result = tool.call(EditFileArgs {
            path: "existing.txt".to_string(),
            mode: EditMode::Create,
            description: None,
            old_string: None,
            new_string: None,
            content: Some("new content".to_string()),
            replace_all: false,
        }).await;

        assert!(matches!(result, Err(EditFileError::FileAlreadyExists(_))));
    }

    #[tokio::test]
    async fn test_overwrite_existing_file() {
        let dir = setup_test_dir();
        std::fs::write(dir.path().join("file.txt"), "old content").unwrap();

        let tool = EditFile::new(dir.path().to_path_buf());

        let result = tool.call(EditFileArgs {
            path: "file.txt".to_string(),
            mode: EditMode::Overwrite,
            description: None,
            old_string: None,
            new_string: None,
            content: Some("new content".to_string()),
            replace_all: false,
        }).await.unwrap();

        assert_eq!(result.mode, "overwrite");
        assert!(result.diff.contains("-old content"));
        assert!(result.diff.contains("+new content"));

        let content = std::fs::read_to_string(dir.path().join("file.txt")).unwrap();
        assert_eq!(content, "new content");
    }

    #[tokio::test]
    async fn test_overwrite_fails_if_not_exists() {
        let dir = setup_test_dir();
        let tool = EditFile::new(dir.path().to_path_buf());

        let result = tool.call(EditFileArgs {
            path: "nonexistent.txt".to_string(),
            mode: EditMode::Overwrite,
            description: None,
            old_string: None,
            new_string: None,
            content: Some("content".to_string()),
            replace_all: false,
        }).await;

        assert!(matches!(result, Err(EditFileError::FileDoesNotExist(_))));
    }

    #[tokio::test]
    async fn test_edit_replace_string() {
        let dir = setup_test_dir();
        std::fs::write(dir.path().join("code.rs"), "fn old_name() {\n    println!(\"hello\");\n}\n").unwrap();

        let tool = EditFile::new(dir.path().to_path_buf());

        let result = tool.call(EditFileArgs {
            path: "code.rs".to_string(),
            mode: EditMode::Edit,
            description: Some("Rename function".to_string()),
            old_string: Some("fn old_name()".to_string()),
            new_string: Some("fn new_name()".to_string()),
            content: None,
            replace_all: false,
        }).await.unwrap();

        assert_eq!(result.mode, "edit");
        assert!(result.message.contains("1 replacement"));
        assert!(result.diff.contains("-fn old_name()"));
        assert!(result.diff.contains("+fn new_name()"));

        let content = std::fs::read_to_string(dir.path().join("code.rs")).unwrap();
        assert!(content.contains("fn new_name()"));
    }

    #[tokio::test]
    async fn test_edit_replace_all() {
        let dir = setup_test_dir();
        std::fs::write(dir.path().join("test.txt"), "foo bar foo baz foo").unwrap();

        let tool = EditFile::new(dir.path().to_path_buf());

        let result = tool.call(EditFileArgs {
            path: "test.txt".to_string(),
            mode: EditMode::Edit,
            description: None,
            old_string: Some("foo".to_string()),
            new_string: Some("qux".to_string()),
            content: None,
            replace_all: true,
        }).await.unwrap();

        assert!(result.message.contains("3 replacements"));

        let content = std::fs::read_to_string(dir.path().join("test.txt")).unwrap();
        assert_eq!(content, "qux bar qux baz qux");
    }

    #[tokio::test]
    async fn test_edit_multiple_matches_without_replace_all() {
        let dir = setup_test_dir();
        std::fs::write(dir.path().join("test.txt"), "foo bar foo").unwrap();

        let tool = EditFile::new(dir.path().to_path_buf());

        let result = tool.call(EditFileArgs {
            path: "test.txt".to_string(),
            mode: EditMode::Edit,
            description: None,
            old_string: Some("foo".to_string()),
            new_string: Some("qux".to_string()),
            content: None,
            replace_all: false,
        }).await;

        assert!(matches!(result, Err(EditFileError::MultipleMatches(2))));
    }

    #[tokio::test]
    async fn test_edit_string_not_found() {
        let dir = setup_test_dir();
        std::fs::write(dir.path().join("test.txt"), "some content").unwrap();

        let tool = EditFile::new(dir.path().to_path_buf());

        let result = tool.call(EditFileArgs {
            path: "test.txt".to_string(),
            mode: EditMode::Edit,
            description: None,
            old_string: Some("nonexistent".to_string()),
            new_string: Some("replacement".to_string()),
            content: None,
            replace_all: false,
        }).await;

        assert!(matches!(result, Err(EditFileError::OldStringNotFound)));
    }

    #[tokio::test]
    async fn test_create_with_nested_dirs() {
        let dir = setup_test_dir();
        let tool = EditFile::new(dir.path().to_path_buf());

        let result = tool.call(EditFileArgs {
            path: "a/b/c/deep.txt".to_string(),
            mode: EditMode::Create,
            description: None,
            old_string: None,
            new_string: None,
            content: Some("deep file".to_string()),
            replace_all: false,
        }).await.unwrap();

        assert_eq!(result.mode, "create");

        let content = std::fs::read_to_string(dir.path().join("a/b/c/deep.txt")).unwrap();
        assert_eq!(content, "deep file");
    }

    #[tokio::test]
    async fn test_path_traversal_blocked() {
        let dir = setup_test_dir();
        let tool = EditFile::new(dir.path().to_path_buf());

        let result = tool.call(EditFileArgs {
            path: "../../../etc/passwd".to_string(),
            mode: EditMode::Create,
            description: None,
            old_string: None,
            new_string: None,
            content: Some("hacked".to_string()),
            replace_all: false,
        }).await;

        // Should fail with either OutsideWorkDir or by not finding the path
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_diff_output_format() {
        let dir = setup_test_dir();
        std::fs::write(dir.path().join("test.txt"), "line1\nline2\nline3\n").unwrap();

        let tool = EditFile::new(dir.path().to_path_buf());

        let result = tool.call(EditFileArgs {
            path: "test.txt".to_string(),
            mode: EditMode::Edit,
            description: None,
            old_string: Some("line2".to_string()),
            new_string: Some("modified".to_string()),
            content: None,
            replace_all: false,
        }).await.unwrap();

        // Check diff format
        assert!(result.diff.contains("--- a/test.txt"));
        assert!(result.diff.contains("+++ b/test.txt"));
        assert!(result.diff.contains("@@"));
        assert!(result.diff.contains("-line2"));
        assert!(result.diff.contains("+modified"));
    }
}
