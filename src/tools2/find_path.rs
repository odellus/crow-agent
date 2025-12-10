//! Find path tool - Search for files by name pattern
//!
//! Ported from tools/find_path.rs with glob pattern support.

use crate::tool::{Tool, ToolContext, ToolDefinition, ToolResult};
use async_trait::async_trait;
use regex::Regex;
use serde::Deserialize;
use serde_json::json;
use std::path::PathBuf;
use walkdir::WalkDir;

const MAX_RESULTS: usize = 200;

#[derive(Debug, Deserialize)]
struct Args {
    pattern: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    file_type: Option<String>,
    #[serde(default)]
    max_depth: Option<usize>,
    #[serde(default)]
    include_hidden: bool,
}

pub struct FindPathTool {
    working_dir: PathBuf,
}

impl FindPathTool {
    pub fn new(working_dir: PathBuf) -> Self {
        Self { working_dir }
    }

    fn resolve_path(&self, path: Option<&str>) -> Result<PathBuf, String> {
        let requested = match path {
            Some(p) if !p.is_empty() && p != "." => {
                let pb = PathBuf::from(p);
                if pb.is_absolute() {
                    pb
                } else {
                    self.working_dir.join(pb)
                }
            }
            _ => self.working_dir.clone(),
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

    fn glob_to_regex(pattern: &str) -> Result<Regex, String> {
        let mut regex_pattern = String::from("(?i)^");

        for c in pattern.chars() {
            match c {
                '*' => regex_pattern.push_str(".*"),
                '?' => regex_pattern.push('.'),
                '.' => regex_pattern.push_str("\\."),
                '[' => regex_pattern.push('['),
                ']' => regex_pattern.push(']'),
                '\\' => regex_pattern.push_str("\\\\"),
                '^' => regex_pattern.push_str("\\^"),
                '$' => regex_pattern.push_str("\\$"),
                '+' => regex_pattern.push_str("\\+"),
                '(' => regex_pattern.push_str("\\("),
                ')' => regex_pattern.push_str("\\)"),
                '|' => regex_pattern.push_str("\\|"),
                '{' => regex_pattern.push_str("\\{"),
                '}' => regex_pattern.push_str("\\}"),
                _ => regex_pattern.push(c),
            }
        }

        regex_pattern.push('$');

        Regex::new(&regex_pattern).map_err(|e| format!("Invalid pattern: {}", e))
    }

    fn should_skip_dir(name: &str) -> bool {
        let skip_dirs = [
            "node_modules", "target", ".git", "__pycache__",
            "venv", ".venv", "dist", "build", ".cargo",
            ".idea", ".vscode", "vendor",
        ];
        skip_dirs.contains(&name)
    }
}

#[async_trait]
impl Tool for FindPathTool {
    fn name(&self) -> &str {
        "find_path"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "find_path".to_string(),
            description: r#"Fast file path pattern matching.

- Supports glob patterns like "*.rs", "**/*.ts"
- Returns matching paths sorted alphabetically
- Use `grep` when searching for content, this when searching by filename
- Results limited to 200 matches"#.to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Glob pattern for file/directory names"
                    },
                    "path": {
                        "type": "string",
                        "description": "Directory to search (default: project root)"
                    },
                    "file_type": {
                        "type": "string",
                        "enum": ["file", "directory", "both"],
                        "description": "Type to search for (default: both)"
                    },
                    "max_depth": {
                        "type": "integer",
                        "description": "Maximum directory depth"
                    },
                    "include_hidden": {
                        "type": "boolean",
                        "description": "Include hidden files (default: false)"
                    }
                },
                "required": ["pattern"]
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

        let search_path = match self.resolve_path(args.path.as_deref()) {
            Ok(p) => p,
            Err(e) => return ToolResult::error(e),
        };

        let regex = match Self::glob_to_regex(&args.pattern) {
            Ok(r) => r,
            Err(e) => return ToolResult::error(e),
        };

        let file_type = args.file_type.as_deref().unwrap_or("both");
        let search_files = file_type == "file" || file_type == "both";
        let search_dirs = file_type == "directory" || file_type == "both";

        let mut results = Vec::new();
        let mut dirs_searched = 0;

        let mut walker = WalkDir::new(&search_path);
        if let Some(depth) = args.max_depth {
            walker = walker.max_depth(depth);
        }

        for entry in walker.into_iter().filter_entry(|e| {
            let name = e.file_name().to_string_lossy();

            if !args.include_hidden && name.starts_with('.') {
                return false;
            }

            if e.file_type().is_dir() && Self::should_skip_dir(&name) {
                return false;
            }

            true
        }) {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };

            let is_dir = entry.file_type().is_dir();
            let is_file = entry.file_type().is_file();

            if is_dir {
                dirs_searched += 1;
            }

            if (is_file && !search_files) || (is_dir && !search_dirs) {
                continue;
            }

            let filename = entry.file_name().to_string_lossy();
            if !regex.is_match(&filename) {
                continue;
            }

            let relative = entry
                .path()
                .strip_prefix(&self.working_dir)
                .unwrap_or(entry.path());

            let suffix = if is_dir { "/" } else { "" };
            results.push(format!("{}{}", relative.display(), suffix));

            if results.len() >= MAX_RESULTS {
                break;
            }
        }

        if results.is_empty() {
            ToolResult::success(format!(
                "No matches for '{}' ({} directories searched)",
                args.pattern, dirs_searched
            ))
        } else {
            let truncated = if results.len() >= MAX_RESULTS {
                format!("\n\n(limited to {} results)", MAX_RESULTS)
            } else {
                String::new()
            };

            ToolResult::success(format!(
                "Found {} matches for '{}':\n\n{}{}",
                results.len(),
                args.pattern,
                results.join("\n"),
                truncated
            ))
        }
    }
}
