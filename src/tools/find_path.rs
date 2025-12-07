//! Find path tool - Search for files by name pattern

use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use walkdir::WalkDir;
use regex::Regex;

const MAX_RESULTS: usize = 200;

#[derive(Debug, thiserror::Error)]
pub enum FindPathError {
    #[error("Invalid pattern: {0}")]
    InvalidPattern(String),
    #[error("Directory not found: {0}")]
    NotFound(String),
    #[error("IO error: {0}")]
    Io(String),
    #[error("Path is outside working directory: {0}")]
    OutsideWorkDir(String),
}

#[derive(Debug, Deserialize)]
pub struct FindPathArgs {
    /// The pattern to search for in file/directory names.
    /// Supports glob-style patterns: * (any chars), ? (single char)
    pub pattern: String,

    /// Directory to search in (default: project root)
    #[serde(default)]
    pub path: Option<String>,

    /// Search type: "file", "directory", or "both" (default)
    #[serde(default)]
    pub file_type: Option<String>,

    /// Maximum depth to search (default: unlimited)
    #[serde(default)]
    pub max_depth: Option<usize>,

    /// Include hidden files/directories (default: false)
    #[serde(default)]
    pub include_hidden: bool,
}

/// Tool for finding files and directories by name pattern
#[derive(Debug, Clone)]
pub struct FindPath {
    working_dir: Arc<PathBuf>,
}

impl FindPath {
    pub fn new(working_dir: PathBuf) -> Self {
        Self {
            working_dir: Arc::new(working_dir),
        }
    }

    fn resolve_path(&self, path: Option<&str>) -> Result<PathBuf, FindPathError> {
        let requested = match path {
            Some(p) if !p.is_empty() && p != "." => {
                let pb = PathBuf::from(p);
                if pb.is_absolute() {
                    pb
                } else {
                    self.working_dir.join(pb)
                }
            }
            _ => self.working_dir.as_ref().clone(),
        };

        let canonical = requested.canonicalize().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                FindPathError::NotFound(path.unwrap_or(".").to_string())
            } else {
                FindPathError::Io(e.to_string())
            }
        })?;

        let working_canonical = self.working_dir.canonicalize().map_err(|e| {
            FindPathError::Io(format!("Cannot resolve working directory: {}", e))
        })?;

        if !canonical.starts_with(&working_canonical) {
            return Err(FindPathError::OutsideWorkDir(path.unwrap_or(".").to_string()));
        }

        Ok(canonical)
    }

    fn glob_to_regex(pattern: &str) -> Result<Regex, FindPathError> {
        // Convert glob pattern to regex
        let mut regex_pattern = String::from("(?i)^"); // Case insensitive by default

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

        Regex::new(&regex_pattern).map_err(|e| {
            FindPathError::InvalidPattern(e.to_string())
        })
    }

    fn should_skip_dir(&self, name: &str) -> bool {
        let skip_dirs = [
            "node_modules", "target", ".git", "__pycache__",
            "venv", ".venv", "dist", "build", ".cargo",
            ".idea", ".vscode", "vendor"
        ];
        skip_dirs.contains(&name)
    }
}

impl Serialize for FindPath {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_unit()
    }
}

impl<'de> Deserialize<'de> for FindPath {
    fn deserialize<D>(_deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Ok(Self::new(std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))))
    }
}

impl Tool for FindPath {
    const NAME: &'static str = "find_path";

    type Error = FindPathError;
    type Args = FindPathArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "find_path".to_string(),
            description: r#"Find files and directories by name pattern.

Uses glob-style patterns:
- * matches any characters
- ? matches a single character

Automatically skips common build/dependency directories.

Examples:
- Find all Rust files: pattern="*.rs"
- Find test files: pattern="*test*"
- Find specific file: pattern="Cargo.toml"
- Find directories named src: pattern="src", file_type="directory""#.to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Glob pattern for file/directory names"
                    },
                    "path": {
                        "type": "string",
                        "description": "Directory to search in (default: project root)"
                    },
                    "file_type": {
                        "type": "string",
                        "enum": ["file", "directory", "both"],
                        "description": "Type to search for (default: both)"
                    },
                    "max_depth": {
                        "type": "integer",
                        "description": "Maximum directory depth to search"
                    },
                    "include_hidden": {
                        "type": "boolean",
                        "description": "Include hidden files/directories (default: false)"
                    }
                },
                "required": ["pattern"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let search_path = self.resolve_path(args.path.as_deref())?;
        let regex = Self::glob_to_regex(&args.pattern)?;

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

            // Skip hidden unless requested
            if !args.include_hidden && name.starts_with('.') {
                return false;
            }

            // Skip common build directories
            if e.file_type().is_dir() && self.should_skip_dir(&name) {
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

            // Check type filter
            if (is_file && !search_files) || (is_dir && !search_dirs) {
                continue;
            }

            // Check pattern against filename
            let filename = entry.file_name().to_string_lossy();
            if !regex.is_match(&filename) {
                continue;
            }

            let relative = entry.path()
                .strip_prefix(&self.working_dir.as_ref())
                .unwrap_or(entry.path());

            let suffix = if is_dir { "/" } else { "" };
            results.push(format!("{}{}", relative.display(), suffix));

            if results.len() >= MAX_RESULTS {
                break;
            }
        }

        if results.is_empty() {
            Ok(format!(
                "No matches found for pattern '{}' (searched {} directories)",
                args.pattern, dirs_searched
            ))
        } else {
            let truncated = if results.len() >= MAX_RESULTS {
                format!("\n\n(results limited to {})", MAX_RESULTS)
            } else {
                String::new()
            };

            Ok(format!(
                "Found {} matches for '{}':\n\n{}{}",
                results.len(),
                args.pattern,
                results.join("\n"),
                truncated
            ))
        }
    }
}
