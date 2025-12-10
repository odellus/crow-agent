//! Read file tool - reads file contents with line numbering
//! Mirrors OpenCode's read tool

use crate::tool::{Tool, ToolContext, ToolDefinition, ToolResult};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use std::path::PathBuf;

const DEFAULT_LINE_LIMIT: usize = 2000;
const MAX_LINE_LENGTH: usize = 2000;
const BINARY_CHECK_SIZE: usize = 8192;

#[derive(Debug, Deserialize)]
struct Args {
    /// File path - supports both filePath (OpenCode style) and path
    #[serde(alias = "filePath")]
    path: String,
    /// Line offset (1-indexed) - supports both offset and start_line
    #[serde(default, alias = "start_line")]
    offset: Option<usize>,
    /// Maximum lines to read
    #[serde(default)]
    limit: Option<usize>,
}

pub struct ReadFileTool {
    working_dir: PathBuf,
}

impl ReadFileTool {
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

        let canonical = full_path.canonicalize().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                format!("File not found: {}", path)
            } else {
                format!("IO error: {}", e)
            }
        })?;

        let working_canonical = self
            .working_dir
            .canonicalize()
            .map_err(|e| format!("Cannot resolve working directory: {}", e))?;

        if !canonical.starts_with(&working_canonical) {
            return Err(format!("Path is outside working directory: {}", path));
        }

        Ok(canonical)
    }

    fn is_binary_file(path: &PathBuf) -> Result<bool, String> {
        use std::io::Read;
        let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
        let mut buffer = vec![0u8; BINARY_CHECK_SIZE];
        let bytes_read = file.read(&mut buffer).map_err(|e| e.to_string())?;
        Ok(buffer[..bytes_read].contains(&0))
    }
}

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "read_file".to_string(),
            description: r#"Reads a file from the local filesystem. You can access any file directly by using this tool.
Assume this tool is able to read all files on the machine. If the User provides a path to a file assume that path is valid. It is okay to read a file that does not exist; an error will be returned.

Usage:
- The filePath parameter must be an absolute path, not a relative path
- By default, it reads up to 2000 lines starting from the beginning of the file
- You can optionally specify a line offset and limit (especially handy for long files), but it's recommended to read the whole file by not providing these parameters
- Any lines longer than 2000 characters will be truncated
- Results are returned using cat -n format, with line numbers starting at 1
- You have the capability to call multiple tools in a single response. It is always better to speculatively read multiple files as a batch that are potentially useful.
- If you read a file that exists but has empty contents you will receive a system reminder warning in place of file contents.
- You can read image files using this tool."#.to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "filePath": {
                        "type": "string",
                        "description": "The path to the file to read"
                    },
                    "offset": {
                        "type": "integer",
                        "description": "Line number to start reading from (1-indexed, optional)"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of lines to read (optional, default 2000)"
                    }
                },
                "required": ["filePath"]
            }),
        }
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let args: Args = match serde_json::from_value(args) {
            Ok(a) => a,
            Err(e) => return ToolResult::error(format!("Invalid arguments: {}", e)),
        };

        if ctx.is_cancelled() {
            return ToolResult::error("Cancelled");
        }

        let path = match self.resolve_path(&args.path) {
            Ok(p) => p,
            Err(e) => return ToolResult::error(e),
        };

        if !path.is_file() {
            return ToolResult::error(format!("Not a file: {}", args.path));
        }

        if let Ok(true) = Self::is_binary_file(&path) {
            return ToolResult::error(format!("Binary file cannot be read as text: {}", args.path));
        }

        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => return ToolResult::error(format!("Failed to read file: {}", e)),
        };

        // Check for empty file
        if content.is_empty() {
            return ToolResult::success("Warning: File exists but has empty contents".to_string());
        }

        let all_lines: Vec<&str> = content.lines().collect();
        let total_lines = all_lines.len();

        // Apply offset and limit (offset is 1-indexed)
        let offset = args.offset.unwrap_or(1).saturating_sub(1); // Convert to 0-indexed
        let limit = args.limit.unwrap_or(DEFAULT_LINE_LIMIT);

        // Format output like cat -n (line numbers with content)
        let lines_to_show: Vec<String> = all_lines
            .iter()
            .skip(offset)
            .take(limit)
            .enumerate()
            .map(|(idx, line)| {
                let line_number = offset + idx + 1; // Convert back to 1-indexed
                // Truncate long lines
                let truncated_line = if line.len() > MAX_LINE_LENGTH {
                    &line[..MAX_LINE_LENGTH]
                } else {
                    line
                };
                format!("{:6}\t{}", line_number, truncated_line)
            })
            .collect();

        let output = lines_to_show.join("\n");

        ToolResult::success(output)
    }
}
