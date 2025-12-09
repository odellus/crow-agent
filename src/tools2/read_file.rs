//! Read file tool

use crate::tool::{Tool, ToolContext, ToolDefinition, ToolResult};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use std::path::PathBuf;

const DEFAULT_MAX_LINES: usize = 2000;
const BINARY_CHECK_SIZE: usize = 8192;

#[derive(Debug, Deserialize)]
struct Args {
    path: String,
    #[serde(default)]
    start_line: Option<u32>,
    #[serde(default)]
    end_line: Option<u32>,
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
            description: "Read the contents of a file. Use start_line/end_line for large files."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the file (relative to project root)"
                    },
                    "start_line": {
                        "type": "integer",
                        "description": "Line number to start reading from (1-indexed)"
                    },
                    "end_line": {
                        "type": "integer",
                        "description": "Line number to stop reading at (1-indexed, inclusive)"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of lines to return (default: 2000)"
                    }
                },
                "required": ["path"]
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

        let lines: Vec<&str> = content.lines().collect();
        let total_lines = lines.len();
        let max_lines = args.limit.unwrap_or(DEFAULT_MAX_LINES);

        let output = match (args.start_line, args.end_line) {
            (Some(start), Some(end)) => {
                let start = start.max(1) as usize;
                let end = end.max(start as u32) as usize;
                let selected: Vec<&str> = lines
                    .iter()
                    .skip(start - 1)
                    .take(end - start + 1)
                    .copied()
                    .collect();
                format!(
                    "# {} (lines {}-{} of {})\n\n{}",
                    args.path,
                    start,
                    end.min(total_lines),
                    total_lines,
                    selected.join("\n")
                )
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
                let mut out = format!(
                    "# {} (lines {}-{} of {})\n\n{}",
                    args.path,
                    start,
                    actual_end,
                    total_lines,
                    selected.join("\n")
                );
                if truncated {
                    out.push_str(&format!(
                        "\n\n... truncated ({} more lines)",
                        total_lines - actual_end
                    ));
                }
                out
            }
            (None, Some(end)) => {
                let end = end.max(1) as usize;
                let selected: Vec<&str> = lines.iter().take(end).copied().collect();
                format!(
                    "# {} (lines 1-{} of {})\n\n{}",
                    args.path,
                    end.min(total_lines),
                    total_lines,
                    selected.join("\n")
                )
            }
            (None, None) => {
                let truncated = total_lines > max_lines;
                let selected: Vec<&str> = lines.iter().take(max_lines).copied().collect();
                let mut out = format!(
                    "# {} ({} lines)\n\n{}",
                    args.path,
                    total_lines,
                    selected.join("\n")
                );
                if truncated {
                    out.push_str(&format!(
                        "\n\n... truncated ({} more lines)",
                        total_lines - max_lines
                    ));
                }
                out
            }
        };

        ToolResult::success(output)
    }
}
