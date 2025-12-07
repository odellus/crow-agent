//! Read file tool - Read contents of files

use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;

/// Maximum file size in bytes before we truncate/summarize
const MAX_FILE_SIZE: u64 = 50_000;

/// Default maximum lines to return when no range specified
const DEFAULT_MAX_LINES: usize = 2000;

/// Number of bytes to check for binary content detection
const BINARY_CHECK_SIZE: usize = 8192;

#[derive(Debug, thiserror::Error)]
pub enum ReadFileError {
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
    #[error("Binary file cannot be read as text: {0}")]
    BinaryFile(String),
}

#[derive(Debug, Deserialize)]
pub struct ReadFileArgs {
    /// The path to the file to read (relative to working directory)
    pub path: String,

    /// Optional line number to start reading from (1-indexed)
    #[serde(default)]
    pub start_line: Option<u32>,

    /// Optional line number to stop reading at (1-indexed, inclusive)
    #[serde(default)]
    pub end_line: Option<u32>,

    /// Maximum number of lines to return (default: 2000)
    #[serde(default)]
    pub limit: Option<usize>,
}

/// Tool for reading file contents
#[derive(Debug, Clone)]
pub struct ReadFile {
    working_dir: Arc<PathBuf>,
}

impl ReadFile {
    pub fn new(working_dir: PathBuf) -> Self {
        Self {
            working_dir: Arc::new(working_dir),
        }
    }

    fn resolve_path(&self, path: &str) -> Result<PathBuf, ReadFileError> {
        let requested = PathBuf::from(path);

        // If absolute, check it's within working dir
        let full_path = if requested.is_absolute() {
            requested
        } else {
            self.working_dir.join(&requested)
        };

        // Canonicalize to resolve .. and symlinks
        let canonical = full_path.canonicalize().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                ReadFileError::NotFound(path.to_string())
            } else {
                ReadFileError::Io(e.to_string())
            }
        })?;

        // Security check: ensure path is within working directory
        let working_canonical = self.working_dir.canonicalize().map_err(|e| {
            ReadFileError::Io(format!("Cannot resolve working directory: {}", e))
        })?;

        if !canonical.starts_with(&working_canonical) {
            return Err(ReadFileError::OutsideWorkDir(path.to_string()));
        }

        Ok(canonical)
    }

    /// Check if a file appears to be binary by looking for null bytes
    fn is_binary_file(path: &PathBuf) -> Result<bool, ReadFileError> {
        use std::io::Read;

        let mut file = std::fs::File::open(path).map_err(|e| {
            ReadFileError::Io(e.to_string())
        })?;

        let mut buffer = vec![0u8; BINARY_CHECK_SIZE];
        let bytes_read = file.read(&mut buffer).map_err(|e| {
            ReadFileError::Io(e.to_string())
        })?;

        // Check for null bytes which indicate binary content
        Ok(buffer[..bytes_read].contains(&0))
    }

    /// Get file metadata
    fn get_file_size(path: &PathBuf) -> Result<u64, ReadFileError> {
        std::fs::metadata(path)
            .map(|m| m.len())
            .map_err(|e| ReadFileError::Io(e.to_string()))
    }
}

// Need to implement Serialize/Deserialize for Tool trait
impl Serialize for ReadFile {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        // Serialize as unit - the working_dir is runtime state
        serializer.serialize_unit()
    }
}

impl<'de> Deserialize<'de> for ReadFile {
    fn deserialize<D>(_deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // Default to current dir when deserializing
        Ok(Self::new(std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))))
    }
}

impl Tool for ReadFile {
    const NAME: &'static str = "read_file";

    type Error = ReadFileError;
    type Args = ReadFileArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "read_file".to_string(),
            description: r#"Reads the content of the given file in the project.

- Never attempt to read a path that hasn't been previously mentioned."#.to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the file (relative to project root)"
                    },
                    "start_line": {
                        "type": "integer",
                        "description": "Line number to start reading from (1-indexed). Optional."
                    },
                    "end_line": {
                        "type": "integer",
                        "description": "Line number to stop reading at (1-indexed, inclusive). Optional."
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of lines to return (default: 2000). Optional."
                    }
                },
                "required": ["path"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let path = self.resolve_path(&args.path)?;

        if !path.is_file() {
            return Err(ReadFileError::NotAFile(args.path));
        }

        // Check for binary file
        if Self::is_binary_file(&path)? {
            return Err(ReadFileError::BinaryFile(args.path));
        }

        let file_size = Self::get_file_size(&path)?;
        let max_lines = args.limit.unwrap_or(DEFAULT_MAX_LINES);

        // Read the file content
        let content = std::fs::read_to_string(&path).map_err(|e| {
            match e.kind() {
                std::io::ErrorKind::PermissionDenied => {
                    ReadFileError::PermissionDenied(args.path.clone())
                }
                std::io::ErrorKind::InvalidData => {
                    // UTF-8 decode error - likely binary
                    ReadFileError::BinaryFile(args.path.clone())
                }
                _ => ReadFileError::Io(e.to_string()),
            }
        })?;

        let lines: Vec<&str> = content.lines().collect();
        let total_lines = lines.len();

        // Handle line range if specified
        match (args.start_line, args.end_line) {
            (Some(start), Some(end)) => {
                let start = start.max(1) as usize;
                let end = end.max(start as u32) as usize;

                let selected: Vec<&str> = lines
                    .iter()
                    .skip(start - 1)
                    .take(end - start + 1)
                    .copied()
                    .collect();

                Ok(format!(
                    "# {} (lines {}-{} of {})\n\n{}",
                    args.path,
                    start,
                    end.min(total_lines),
                    total_lines,
                    selected.join("\n")
                ))
            }
            (Some(start), None) => {
                let start = start.max(1) as usize;
                let selected: Vec<&str> = lines
                    .iter()
                    .skip(start - 1)
                    .take(max_lines)
                    .copied()
                    .collect();

                let actual_end = (start - 1 + selected.len()).min(total_lines);
                let truncated = total_lines > start - 1 + max_lines;

                let mut output = format!(
                    "# {} (lines {}-{} of {})\n\n{}",
                    args.path,
                    start,
                    actual_end,
                    total_lines,
                    selected.join("\n")
                );

                if truncated {
                    output.push_str(&format!(
                        "\n\n... truncated ({} more lines). Use start_line/end_line to read specific sections.",
                        total_lines - actual_end
                    ));
                }

                Ok(output)
            }
            (None, Some(end)) => {
                let end = end.max(1) as usize;
                let selected: Vec<&str> = lines.iter().take(end).copied().collect();

                Ok(format!(
                    "# {} (lines 1-{} of {})\n\n{}",
                    args.path,
                    end.min(total_lines),
                    total_lines,
                    selected.join("\n")
                ))
            }
            (None, None) => {
                // No range specified - apply limits
                let size_warning = if file_size > MAX_FILE_SIZE {
                    Some(format!(
                        "Note: File is {} bytes (>{} limit).",
                        file_size, MAX_FILE_SIZE
                    ))
                } else {
                    None
                };

                let truncated = total_lines > max_lines;
                let selected: Vec<&str> = lines.iter().take(max_lines).copied().collect();
                let lines_shown = selected.len();

                let mut output = format!(
                    "# {} ({} lines",
                    args.path,
                    total_lines,
                );

                if let Some(warning) = size_warning {
                    output.push_str(&format!(", {} bytes", file_size));
                    output.push_str(")\n\n");
                    output.push_str(&warning);
                    output.push_str("\n\n");
                } else {
                    output.push_str(")\n\n");
                }

                output.push_str(&selected.join("\n"));

                if truncated {
                    output.push_str(&format!(
                        "\n\n... truncated after {} lines ({} more lines). Use start_line/end_line to read specific sections.",
                        lines_shown,
                        total_lines - lines_shown
                    ));
                }

                Ok(output)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    fn setup_test_dir() -> TempDir {
        tempfile::tempdir().unwrap()
    }

    #[tokio::test]
    async fn test_read_simple_file() {
        let dir = setup_test_dir();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, "line 1\nline 2\nline 3").unwrap();

        let tool = ReadFile::new(dir.path().to_path_buf());
        let result = tool.call(ReadFileArgs {
            path: "test.txt".to_string(),
            start_line: None,
            end_line: None,
            limit: None,
        }).await.unwrap();

        assert!(result.contains("line 1"));
        assert!(result.contains("line 2"));
        assert!(result.contains("line 3"));
        assert!(result.contains("3 lines"));
    }

    #[tokio::test]
    async fn test_read_line_range() {
        let dir = setup_test_dir();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, "line 1\nline 2\nline 3\nline 4\nline 5").unwrap();

        let tool = ReadFile::new(dir.path().to_path_buf());
        let result = tool.call(ReadFileArgs {
            path: "test.txt".to_string(),
            start_line: Some(2),
            end_line: Some(4),
            limit: None,
        }).await.unwrap();

        assert!(result.contains("line 2"));
        assert!(result.contains("line 3"));
        assert!(result.contains("line 4"));
        assert!(!result.contains("line 1\n"));
        assert!(!result.contains("\nline 5"));
        assert!(result.contains("lines 2-4 of 5"));
    }

    #[tokio::test]
    async fn test_read_with_limit() {
        let dir = setup_test_dir();
        let file_path = dir.path().join("test.txt");
        let content: String = (1..=100).map(|i| format!("line {}\n", i)).collect();
        std::fs::write(&file_path, &content).unwrap();

        let tool = ReadFile::new(dir.path().to_path_buf());
        let result = tool.call(ReadFileArgs {
            path: "test.txt".to_string(),
            start_line: None,
            end_line: None,
            limit: Some(10),
        }).await.unwrap();

        assert!(result.contains("line 1"));
        assert!(result.contains("line 10"));
        assert!(result.contains("truncated"));
        assert!(result.contains("90 more lines"));
    }

    #[tokio::test]
    async fn test_binary_file_detection() {
        let dir = setup_test_dir();
        let file_path = dir.path().join("binary.bin");
        let mut file = std::fs::File::create(&file_path).unwrap();
        file.write_all(&[0x00, 0x01, 0x02, 0xFF, 0x00]).unwrap();

        let tool = ReadFile::new(dir.path().to_path_buf());
        let result = tool.call(ReadFileArgs {
            path: "binary.bin".to_string(),
            start_line: None,
            end_line: None,
            limit: None,
        }).await;

        assert!(matches!(result, Err(ReadFileError::BinaryFile(_))));
    }

    #[tokio::test]
    async fn test_file_not_found() {
        let dir = setup_test_dir();
        let tool = ReadFile::new(dir.path().to_path_buf());

        let result = tool.call(ReadFileArgs {
            path: "nonexistent.txt".to_string(),
            start_line: None,
            end_line: None,
            limit: None,
        }).await;

        assert!(matches!(result, Err(ReadFileError::NotFound(_))));
    }

    #[tokio::test]
    async fn test_path_traversal_blocked() {
        let dir = setup_test_dir();
        let tool = ReadFile::new(dir.path().to_path_buf());

        let result = tool.call(ReadFileArgs {
            path: "../../../etc/passwd".to_string(),
            start_line: None,
            end_line: None,
            limit: None,
        }).await;

        // Should either be NotFound or OutsideWorkDir
        assert!(matches!(result, Err(ReadFileError::NotFound(_)) | Err(ReadFileError::OutsideWorkDir(_))));
    }

    #[tokio::test]
    async fn test_large_file_warning() {
        let dir = setup_test_dir();
        let file_path = dir.path().join("large.txt");
        // Create a file larger than MAX_FILE_SIZE
        let content = "x".repeat(60_000);
        std::fs::write(&file_path, &content).unwrap();

        let tool = ReadFile::new(dir.path().to_path_buf());
        let result = tool.call(ReadFileArgs {
            path: "large.txt".to_string(),
            start_line: None,
            end_line: None,
            limit: None,
        }).await.unwrap();

        assert!(result.contains("bytes"));
        assert!(result.contains("60000"));
    }
}
