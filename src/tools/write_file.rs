//! Write file tool - writes file contents to the filesystem
//! Mirrors OpenCode's write tool with verbatim description

use crate::tool::{Tool, ToolContext, ToolDefinition, ToolResult};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::PathBuf;

/// Raw args before content normalization
#[derive(Debug, Deserialize)]
struct RawArgs {
    /// File path - supports both filePath (OpenCode style) and file_path
    #[serde(alias = "filePath", alias = "file_path")]
    path: String,
    /// The content to write to the file - can be string or object (will be stringified)
    content: Value,
}

struct Args {
    path: String,
    content: String,
}

pub struct WriteFileTool {
    working_dir: PathBuf,
}

impl WriteFileTool {
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

        // For write, the file might not exist yet, so we check the parent directory
        let parent = full_path
            .parent()
            .ok_or_else(|| "Invalid path: no parent directory".to_string())?;

        // Canonicalize parent to check it's within working directory
        let parent_canonical = parent.canonicalize().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                format!("Parent directory not found: {}", parent.display())
            } else {
                format!("IO error: {}", e)
            }
        })?;

        let working_canonical = self
            .working_dir
            .canonicalize()
            .map_err(|e| format!("Cannot resolve working directory: {}", e))?;

        if !parent_canonical.starts_with(&working_canonical) {
            return Err(format!("Path is outside working directory: {}", path));
        }

        // Return the full path (not canonicalized since file may not exist yet)
        Ok(full_path)
    }
}

#[async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &str {
        "write_file"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "write_file".to_string(),
            // Verbatim copy from opencode's write.txt
            description: r#"Writes a file to the local filesystem.

Usage:
- This tool will overwrite the existing file if there is one at the provided path.
- If this is an existing file, you MUST use the Read tool first to read the file's contents. This tool will fail if you did not read the file first.
- ALWAYS prefer editing existing files in the codebase. NEVER write new files unless explicitly required.
- NEVER proactively create documentation files (*.md) or README files. Only create documentation files if explicitly requested by the User.
- Only use emojis if the user explicitly requests it. Avoid writing emojis to files unless asked."#.to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "filePath": {
                        "type": "string",
                        "description": "The absolute path to the file to write (must be absolute, not relative)"
                    },
                    "content": {
                        "type": "string",
                        "description": "The content to write to the file"
                    }
                },
                "required": ["filePath", "content"]
            }),
        }
    }

    /// Humanize: path + whether it was created or overwritten
    fn humanize(&self, args: &Value, result: &ToolResult) -> Option<String> {
        let path = args
            .get("filePath")
            .or_else(|| args.get("file_path"))
            .or_else(|| args.get("path"))
            .and_then(|v| v.as_str())?;

        if result.is_error {
            return Some(format!("write {} → err: {}", path, result.output));
        }

        Some(format!("write {} → ok", path))
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let raw_args: RawArgs = match serde_json::from_value(args) {
            Ok(a) => a,
            Err(e) => return ToolResult::error(format!("Invalid arguments: {}", e)),
        };

        // Normalize content: if LLM passed an object/array, stringify it
        let content = match raw_args.content {
            Value::String(s) => s,
            Value::Null => String::new(),
            other => serde_json::to_string_pretty(&other).unwrap_or_else(|_| other.to_string()),
        };

        let args = Args {
            path: raw_args.path,
            content,
        };

        if ctx.is_cancelled() {
            return ToolResult::error("Cancelled");
        }

        let path = match self.resolve_path(&args.path) {
            Ok(p) => p,
            Err(e) => return ToolResult::error(e),
        };

        // Check if file exists (for reporting purposes)
        let existed = path.exists();

        // Write the file
        match std::fs::write(&path, &args.content) {
            Ok(()) => {
                let action = if existed { "Updated" } else { "Created" };
                ToolResult::success(format!("{} {}", action, path.display()))
            }
            Err(e) => ToolResult::error(format!("Failed to write file: {}", e)),
        }
    }
}
