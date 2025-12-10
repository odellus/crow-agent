//! Grep tool - Search for patterns in files
//!
//! Ported from tools/grep.rs with pagination and context support.

use crate::tool::{Tool, ToolContext, ToolDefinition, ToolResult};
use async_trait::async_trait;
use regex::Regex;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::PathBuf;
use walkdir::WalkDir;

const RESULTS_PER_PAGE: usize = 20;
const MAX_LINE_LENGTH: usize = 500;
const BINARY_CHECK_SIZE: usize = 8192;

#[derive(Debug, Deserialize)]
struct Args {
    pattern: String,
    #[serde(default)]
    path: Option<String>,
    /// Include pattern - supports both "include" (OpenCode style) and "include_pattern"
    #[serde(default, alias = "include_pattern")]
    include: Option<String>,
}

pub struct GrepTool {
    working_dir: PathBuf,
}

impl GrepTool {
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

    fn is_binary_file(path: &PathBuf) -> bool {
        use std::io::Read;
        if let Ok(mut file) = std::fs::File::open(path) {
            let mut buffer = vec![0u8; BINARY_CHECK_SIZE];
            if let Ok(bytes_read) = file.read(&mut buffer) {
                return buffer[..bytes_read].contains(&0);
            }
        }
        false
    }

    fn should_search_file(&self, path: &PathBuf, include_pattern: Option<&str>) -> bool {
        let relative = path.strip_prefix(&self.working_dir).unwrap_or(path);

        // Skip hidden files
        for component in relative.components() {
            if let std::path::Component::Normal(name) = component {
                if name.to_string_lossy().starts_with('.') {
                    return false;
                }
            }
        }

        // Skip common non-text directories
        let skip_dirs = [
            "node_modules", "target", ".git", "__pycache__",
            "venv", ".venv", "dist", "build",
        ];
        let relative_str = relative.to_string_lossy();
        for dir in &skip_dirs {
            if relative_str.contains(&format!("{}/", dir)) || relative_str == *dir {
                return false;
            }
        }

        // Check include pattern
        if let Some(pattern) = include_pattern {
            if let Ok(glob) = glob::Pattern::new(pattern) {
                let filename = path.file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                if !glob.matches(&filename) && !glob.matches_path(path) {
                    return false;
                }
            }
        }

        // Skip binary extensions
        let binary_exts = [
            "exe", "dll", "so", "dylib", "bin", "o", "a",
            "png", "jpg", "jpeg", "gif", "ico", "webp",
            "pdf", "zip", "tar", "gz", "7z", "rar",
            "mp3", "mp4", "avi", "mov", "wav", "wasm", "pyc", "class",
        ];
        if let Some(ext) = path.extension() {
            let ext_lower = ext.to_string_lossy().to_lowercase();
            if binary_exts.contains(&ext_lower.as_str()) {
                return false;
            }
        }

        true
    }
}

#[async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &str {
        "grep"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "grep".to_string(),
            description: r#"- Fast content search tool that works with any codebase size
- Searches file contents using regular expressions
- Supports full regex syntax (eg. "log.*Error", "function\s+\w+", etc.)
- Filter files by pattern with the include parameter (eg. "*.js", "*.{ts,tsx}")
- Returns file paths with at least one match sorted by modification time
- Use this tool when you need to find files containing specific patterns
- If you need to identify/count the number of matches within files, use the Bash tool with `rg` (ripgrep) directly. Do NOT use `grep`.
- When you are doing an open ended search that may require multiple rounds of globbing and grepping, use the Task tool instead"#.to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "The regex pattern to search for in file contents"
                    },
                    "path": {
                        "type": "string",
                        "description": "The directory to search in. Defaults to the current working directory."
                    },
                    "include": {
                        "type": "string",
                        "description": "File pattern to include in the search (e.g. \"*.js\", \"*.{ts,tsx}\")"
                    }
                },
                "required": ["pattern"]
            }),
        }
    }

    /// Humanize: pattern + first 30 lines of matches
    fn humanize(&self, args: &Value, result: &ToolResult) -> Option<String> {
        let pattern = args.get("pattern").and_then(|v| v.as_str())?;

        if result.is_error {
            return Some(format!("grep \"{}\" → err: {}", pattern, result.output));
        }

        let lines: Vec<&str> = result.output.lines().collect();
        let total = lines.len();

        let preview: String = if total <= 30 {
            result.output.clone()
        } else {
            let first_30 = lines[..30].join("\n");
            format!("{}\n... ({} more lines)", first_30, total - 30)
        };

        Some(format!("grep \"{}\"\n{}", pattern, preview))
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

        // Build regex (case insensitive by default like ripgrep)
        let pattern = format!("(?i){}", args.pattern);

        let regex = match Regex::new(&pattern) {
            Ok(r) => r,
            Err(e) => return ToolResult::error(format!("Invalid regex: {}", e)),
        };

        // Collect matches: (path, line_num, line_text)
        let mut all_matches: Vec<(PathBuf, usize, String)> = Vec::new();

        let walker = if search_path.is_file() {
            WalkDir::new(&search_path).max_depth(0)
        } else {
            WalkDir::new(&search_path)
        };

        for entry in walker.into_iter().filter_map(|e| e.ok()) {
            let path = entry.path();

            if !path.is_file() {
                continue;
            }

            if !self.should_search_file(&path.to_path_buf(), args.include.as_deref()) {
                continue;
            }

            if Self::is_binary_file(&path.to_path_buf()) {
                continue;
            }

            let content = match std::fs::read_to_string(path) {
                Ok(c) => c,
                Err(_) => continue,
            };

            let lines: Vec<&str> = content.lines().collect();

            for (line_num, line) in lines.iter().enumerate() {
                if regex.is_match(line) {
                    all_matches.push((
                        path.to_path_buf(),
                        line_num + 1,
                        line.to_string(),
                    ));
                }
            }
        }

        let total_matches = all_matches.len();

        if total_matches == 0 {
            return ToolResult::success("No files found".to_string());
        }

        // Limit results (like crow-old's 100 limit)
        let truncated = total_matches > RESULTS_PER_PAGE * 5;
        let limit = if truncated { RESULTS_PER_PAGE * 5 } else { total_matches };

        // Format output like crow-old
        let mut output_lines = vec![format!("Found {} matches", total_matches.min(limit))];
        let mut current_file = String::new();

        for (path, line_num, line_text) in all_matches.into_iter().take(limit) {
            let relative = path.strip_prefix(&self.working_dir).unwrap_or(&path);
            let relative_str = relative.display().to_string();

            if current_file != relative_str {
                if !current_file.is_empty() {
                    output_lines.push(String::new());
                }
                current_file = relative_str.clone();
                output_lines.push(format!("{}:", relative_str));
            }

            let display_line = if line_text.len() > MAX_LINE_LENGTH {
                format!("{}...", &line_text[..MAX_LINE_LENGTH])
            } else {
                line_text
            };

            output_lines.push(format!("  Line {}: {}", line_num, display_line));
        }

        if truncated {
            output_lines.push(String::new());
            output_lines.push(
                "(Results are truncated. Consider using a more specific path or pattern.)".to_string()
            );
        }

        ToolResult::success(output_lines.join("\n"))
    }
}
