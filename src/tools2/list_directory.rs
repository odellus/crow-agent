//! List directory tool - List files and directories
//!
//! Ported from tools/list_directory.rs with recursive support.

use crate::tool::{Tool, ToolContext, ToolDefinition, ToolResult};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
struct Args {
    path: String,
    #[serde(default)]
    pattern: Option<String>,
    #[serde(default)]
    recursive: bool,
}

pub struct ListDirectoryTool {
    working_dir: PathBuf,
}

impl ListDirectoryTool {
    pub fn new(working_dir: PathBuf) -> Self {
        Self { working_dir }
    }

    fn resolve_path(&self, path: &str) -> Result<PathBuf, String> {
        let requested = if path.is_empty() || path == "." {
            self.working_dir.clone()
        } else {
            let p = PathBuf::from(path);
            if p.is_absolute() {
                p
            } else {
                self.working_dir.join(p)
            }
        };

        let canonical = requested
            .canonicalize()
            .map_err(|e| format!("Path not found: {}", e))?;

        let working_canonical = self
            .working_dir
            .canonicalize()
            .map_err(|e| format!("Cannot resolve working dir: {}", e))?;

        if !canonical.starts_with(&working_canonical) {
            return Err("Path outside working directory".to_string());
        }

        Ok(canonical)
    }

    fn list_recursive_with_pattern(
        &self,
        path: &PathBuf,
        base: &PathBuf,
        pattern: Option<&glob::Pattern>,
        max_depth: u32,
        current_depth: u32,
        output: &mut Vec<String>,
    ) -> Result<(), String> {
        if current_depth > max_depth {
            return Ok(());
        }

        let entries = std::fs::read_dir(path)
            .map_err(|e| format!("Cannot read directory: {}", e))?;

        let mut items: Vec<_> = entries.filter_map(|e| e.ok()).collect();
        items.sort_by(|a, b| a.file_name().cmp(&b.file_name()));

        for entry in items {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();

            // Skip hidden files
            if name_str.starts_with('.') {
                continue;
            }

            let entry_path = entry.path();
            let relative = entry_path.strip_prefix(base).unwrap_or(&entry_path);
            let is_dir = entry_path.is_dir();

            // Apply pattern filter (only to files, always show directories for navigation)
            if let Some(pat) = pattern {
                if !is_dir && !pat.matches(&name_str) {
                    continue;
                }
            }

            let prefix = "  ".repeat(current_depth as usize);
            let suffix = if is_dir { "/" } else { "" };

            output.push(format!("{}{}{}", prefix, relative.display(), suffix));

            if is_dir && current_depth < max_depth {
                self.list_recursive_with_pattern(
                    &entry_path,
                    base,
                    pattern,
                    max_depth,
                    current_depth + 1,
                    output,
                )?;
            }
        }

        Ok(())
    }
}

#[async_trait]
impl Tool for ListDirectoryTool {
    fn name(&self) -> &str {
        "list_directory"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "list_directory".to_string(),
            description: "Display files and directories in a given path. Accepts glob patterns to filter results (e.g., '*.rs', 'src/**/*.ts').".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Directory path to list (defaults to current directory)"
                    },
                    "pattern": {
                        "type": "string",
                        "description": "Optional glob pattern to filter results (e.g., '*.rs')"
                    },
                    "recursive": {
                        "type": "boolean",
                        "description": "List files recursively (default: false)"
                    }
                },
                "required": ["path"]
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

        if !path.is_dir() {
            return ToolResult::error(format!("Not a directory: {}", args.path));
        }

        // Max depth: 0 for non-recursive, high number for recursive
        let max_depth = if args.recursive { 10 } else { 0 };
        let mut output = Vec::new();

        output.push(format!("# {}/", args.path));
        output.push(String::new());

        // Build glob pattern matcher if provided
        let glob_pattern = args.pattern.as_ref().and_then(|p| glob::Pattern::new(p).ok());

        if let Err(e) = self.list_recursive_with_pattern(
            &path,
            &path,
            glob_pattern.as_ref(),
            max_depth,
            0,
            &mut output,
        ) {
            return ToolResult::error(e);
        }

        if output.len() == 2 {
            output.push("(empty directory)".to_string());
        }

        ToolResult::success(output.join("\n"))
    }
}
