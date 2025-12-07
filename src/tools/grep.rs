//! Grep tool - Search for patterns in files

use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use walkdir::WalkDir;
use regex::Regex;

/// Results per page for pagination
const RESULTS_PER_PAGE: usize = 20;

/// Maximum line length before truncation
const MAX_LINE_LENGTH: usize = 500;

/// Number of bytes to check for binary content
const BINARY_CHECK_SIZE: usize = 8192;

#[derive(Debug, thiserror::Error)]
pub enum GrepError {
    #[error("Invalid regex pattern: {0}")]
    InvalidPattern(String),
    #[error("Directory not found: {0}")]
    NotFound(String),
    #[error("IO error: {0}")]
    Io(String),
    #[error("Path is outside working directory: {0}")]
    OutsideWorkDir(String),
}

#[derive(Debug, Deserialize)]
pub struct GrepArgs {
    /// The regex pattern to search for
    pub pattern: String,

    /// Directory or file to search in (relative to project root). Default: "."
    #[serde(default)]
    pub path: Option<String>,

    /// Glob pattern for files to include (e.g., "*.rs", "**/*.ts")
    #[serde(default)]
    pub include_pattern: Option<String>,

    /// Case sensitive search (default: false)
    #[serde(default)]
    pub case_sensitive: bool,

    /// Include line numbers in output (default: true)
    #[serde(default = "default_true")]
    pub line_numbers: bool,

    /// Number of context lines before match
    #[serde(default)]
    pub context_before: Option<usize>,

    /// Number of context lines after match
    #[serde(default)]
    pub context_after: Option<usize>,

    /// Pagination offset (0-based). Use to get subsequent pages of results.
    #[serde(default)]
    pub offset: usize,
}

fn default_true() -> bool {
    true
}

/// Tool for searching file contents with regex
#[derive(Debug, Clone)]
pub struct Grep {
    working_dir: Arc<PathBuf>,
}

impl Grep {
    pub fn new(working_dir: PathBuf) -> Self {
        Self {
            working_dir: Arc::new(working_dir),
        }
    }

    fn resolve_path(&self, path: Option<&str>) -> Result<PathBuf, GrepError> {
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
                GrepError::NotFound(path.unwrap_or(".").to_string())
            } else {
                GrepError::Io(e.to_string())
            }
        })?;

        let working_canonical = self.working_dir.canonicalize().map_err(|e| {
            GrepError::Io(format!("Cannot resolve working directory: {}", e))
        })?;

        if !canonical.starts_with(&working_canonical) {
            return Err(GrepError::OutsideWorkDir(path.unwrap_or(".").to_string()));
        }

        Ok(canonical)
    }

    /// Check if a file appears to be binary
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
        // Get relative path from working dir for checking hidden files
        let relative_path = path.strip_prefix(&*self.working_dir).unwrap_or(path);

        // Skip hidden files and directories (only in relative path, not temp dir path)
        for component in relative_path.components() {
            if let std::path::Component::Normal(name) = component {
                if name.to_string_lossy().starts_with('.') {
                    return false;
                }
            }
        }

        // Skip common non-text files and directories
        let skip_dirs = ["node_modules", "target", ".git", "__pycache__", "venv", ".venv", "dist", "build"];
        let relative_str = relative_path.to_string_lossy();
        for dir in &skip_dirs {
            if relative_str.contains(&format!("{}/", dir))
               || relative_str.contains(&format!("{}\\", dir))
               || relative_str == *dir {
                return false;
            }
        }

        // Check include pattern (glob)
        if let Some(pattern) = include_pattern {
            if let Ok(glob_pattern) = glob::Pattern::new(pattern) {
                let filename = path.file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();

                // Try matching against filename first
                if !glob_pattern.matches(&filename) {
                    // Also try matching against the full path
                    if !glob_pattern.matches_path(path) {
                        return false;
                    }
                }
            }
        }

        // Skip binary files by extension
        let binary_extensions = ["exe", "dll", "so", "dylib", "bin", "o", "a",
                                  "png", "jpg", "jpeg", "gif", "ico", "webp",
                                  "pdf", "zip", "tar", "gz", "7z", "rar",
                                  "mp3", "mp4", "avi", "mov", "wav",
                                  "wasm", "pyc", "class"];
        if let Some(ext) = path.extension() {
            let ext_lower = ext.to_string_lossy().to_lowercase();
            if binary_extensions.contains(&ext_lower.as_str()) {
                return false;
            }
        }

        true
    }
}

impl Serialize for Grep {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_unit()
    }
}

impl<'de> Deserialize<'de> for Grep {
    fn deserialize<D>(_deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Ok(Self::new(std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))))
    }
}

impl Tool for Grep {
    const NAME: &'static str = "grep";

    type Error = GrepError;
    type Args = GrepArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "grep".to_string(),
            description: r#"Searches the contents of files in the project with a regular expression

- Prefer this tool to path search when searching for symbols in the project, because you won't need to guess what path it's in.
- Supports full regex syntax (eg. "log.*Error", "function\\s+\\w+", etc.)
- Pass an `include_pattern` if you know how to narrow your search on the files system
- Never use this tool to search for paths. Only search file contents with this tool.
- Use this tool when you need to find files containing specific patterns
- Results are paginated with 20 matches per page. Use the optional 'offset' parameter to request subsequent pages.
- DO NOT use HTML entities solely to escape characters in the tool parameters."#.to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Regex pattern to search for"
                    },
                    "path": {
                        "type": "string",
                        "description": "Directory or file to search (default: project root)"
                    },
                    "include_pattern": {
                        "type": "string",
                        "description": "Glob pattern to filter files (e.g., '*.rs', '**/*.ts')"
                    },
                    "case_sensitive": {
                        "type": "boolean",
                        "description": "Case sensitive search (default: false)"
                    },
                    "line_numbers": {
                        "type": "boolean",
                        "description": "Include line numbers (default: true)"
                    },
                    "context_before": {
                        "type": "integer",
                        "description": "Lines of context before match"
                    },
                    "context_after": {
                        "type": "integer",
                        "description": "Lines of context after match"
                    },
                    "offset": {
                        "type": "integer",
                        "description": "Pagination offset (0-based). Results return 20 at a time."
                    }
                },
                "required": ["pattern"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let search_path = self.resolve_path(args.path.as_deref())?;

        // Build regex
        let pattern = if args.case_sensitive {
            args.pattern.clone()
        } else {
            format!("(?i){}", args.pattern)
        };

        let regex = Regex::new(&pattern).map_err(|e| {
            GrepError::InvalidPattern(e.to_string())
        })?;

        let mut all_matches: Vec<(PathBuf, usize, String, Vec<String>, Vec<String>)> = Vec::new();
        let mut files_searched = 0;
        let mut files_with_matches = 0;

        // Walk the directory tree
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

            if !self.should_search_file(&path.to_path_buf(), args.include_pattern.as_deref()) {
                continue;
            }

            // Check for binary content
            if Self::is_binary_file(&path.to_path_buf()) {
                continue;
            }

            // Try to read the file
            let content = match std::fs::read_to_string(path) {
                Ok(c) => c,
                Err(_) => continue, // Skip files we can't read
            };

            files_searched += 1;
            let lines: Vec<&str> = content.lines().collect();
            let mut file_has_match = false;

            for (line_num, line) in lines.iter().enumerate() {
                if regex.is_match(line) {
                    file_has_match = true;

                    // Collect context lines
                    let context_before: Vec<String> = if let Some(before) = args.context_before {
                        let start = line_num.saturating_sub(before);
                        lines[start..line_num].iter().map(|s| s.to_string()).collect()
                    } else {
                        Vec::new()
                    };

                    let context_after: Vec<String> = if let Some(after) = args.context_after {
                        let end = (line_num + after + 1).min(lines.len());
                        lines[line_num + 1..end].iter().map(|s| s.to_string()).collect()
                    } else {
                        Vec::new()
                    };

                    all_matches.push((
                        path.to_path_buf(),
                        line_num + 1,
                        line.to_string(),
                        context_before,
                        context_after,
                    ));
                }
            }

            if file_has_match {
                files_with_matches += 1;
            }
        }

        let total_matches = all_matches.len();

        if total_matches == 0 {
            return Ok(format!(
                "No matches found for pattern '{}' in {} files",
                args.pattern, files_searched
            ));
        }

        // Apply pagination
        let page_start = args.offset;
        let page_end = (args.offset + RESULTS_PER_PAGE).min(total_matches);
        let page_matches: Vec<_> = all_matches.into_iter()
            .skip(page_start)
            .take(RESULTS_PER_PAGE)
            .collect();

        let current_page = args.offset / RESULTS_PER_PAGE + 1;
        let total_pages = (total_matches + RESULTS_PER_PAGE - 1) / RESULTS_PER_PAGE;

        // Format results
        let mut results = Vec::new();
        let mut current_file: Option<PathBuf> = None;

        for (path, line_num, line, before, after) in page_matches {
            // Add file header when file changes
            let relative_path = path.strip_prefix(&self.working_dir.as_ref())
                .unwrap_or(&path);

            if current_file.as_ref() != Some(&path) {
                results.push(format!("\n## {}", relative_path.display()));
                current_file = Some(path);
            }

            // Truncate long lines
            let display_line = if line.len() > MAX_LINE_LENGTH {
                format!("{}...", &line[..MAX_LINE_LENGTH])
            } else {
                line
            };

            // Format the match with optional context
            let mut match_output = Vec::new();

            for (i, ctx_line) in before.iter().enumerate() {
                let ctx_line_num = line_num - before.len() + i;
                if args.line_numbers {
                    match_output.push(format!("  {}:  {}", ctx_line_num, ctx_line));
                } else {
                    match_output.push(format!("  {}", ctx_line));
                }
            }

            if args.line_numbers {
                match_output.push(format!("  {}> {}", line_num, display_line));
            } else {
                match_output.push(format!("> {}", display_line));
            }

            for (i, ctx_line) in after.iter().enumerate() {
                if args.line_numbers {
                    match_output.push(format!("  {}:  {}", line_num + 1 + i, ctx_line));
                } else {
                    match_output.push(format!("  {}", ctx_line));
                }
            }

            results.push(match_output.join("\n"));
        }

        // Build final output
        let mut output = format!(
            "Found {} matches in {} files ({} files searched)\n",
            total_matches, files_with_matches, files_searched
        );

        if total_pages > 1 {
            output.push_str(&format!(
                "Showing results {}-{} (page {}/{})\n",
                page_start + 1, page_end, current_page, total_pages
            ));
        }

        output.push_str(&results.join("\n"));

        if page_end < total_matches {
            output.push_str(&format!(
                "\n\n... {} more matches. Use offset={} to see next page.",
                total_matches - page_end,
                page_end
            ));
        }

        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup_test_dir() -> TempDir {
        let dir = tempfile::tempdir().unwrap();

        // Create test files
        std::fs::write(dir.path().join("main.rs"), "fn main() {\n    println!(\"Hello\");\n}\n").unwrap();
        std::fs::write(dir.path().join("lib.rs"), "pub fn helper() {\n    // TODO: implement\n}\n").unwrap();
        std::fs::write(dir.path().join("test.txt"), "This is a test file\nWith multiple lines\n").unwrap();

        // Create subdirectory
        std::fs::create_dir(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/utils.rs"), "fn utility() {\n    todo!()\n}\n").unwrap();

        dir
    }

    #[tokio::test]
    async fn test_grep_simple_pattern() {
        let dir = setup_test_dir();
        let tool = Grep::new(dir.path().to_path_buf());

        let result = tool.call(GrepArgs {
            pattern: "fn".to_string(),
            path: None,
            include_pattern: None,
            case_sensitive: false,
            line_numbers: true,
            context_before: None,
            context_after: None,
            offset: 0,
        }).await.unwrap();

        assert!(result.contains("fn main"));
        assert!(result.contains("fn helper"));
        assert!(result.contains("fn utility"));
    }

    #[tokio::test]
    async fn test_grep_case_sensitive() {
        let dir = setup_test_dir();
        let tool = Grep::new(dir.path().to_path_buf());

        // Case insensitive (default)
        let result = tool.call(GrepArgs {
            pattern: "HELLO".to_string(),
            path: None,
            include_pattern: None,
            case_sensitive: false,
            line_numbers: true,
            context_before: None,
            context_after: None,
            offset: 0,
        }).await.unwrap();
        assert!(result.contains("Hello"));

        // Case sensitive
        let result = tool.call(GrepArgs {
            pattern: "HELLO".to_string(),
            path: None,
            include_pattern: None,
            case_sensitive: true,
            line_numbers: true,
            context_before: None,
            context_after: None,
            offset: 0,
        }).await.unwrap();
        assert!(result.contains("No matches"));
    }

    #[tokio::test]
    async fn test_grep_include_pattern() {
        let dir = setup_test_dir();
        let tool = Grep::new(dir.path().to_path_buf());

        let result = tool.call(GrepArgs {
            pattern: "fn".to_string(),
            path: None,
            include_pattern: Some("*.rs".to_string()),
            case_sensitive: false,
            line_numbers: true,
            context_before: None,
            context_after: None,
            offset: 0,
        }).await.unwrap();

        assert!(result.contains("main.rs"));
        assert!(result.contains("lib.rs"));
        assert!(!result.contains("test.txt"));
    }

    #[tokio::test]
    async fn test_grep_pagination() {
        let dir = tempfile::tempdir().unwrap();

        // Create file with many matches
        let content: String = (1..=50).map(|i| format!("match line {}\n", i)).collect();
        std::fs::write(dir.path().join("many.txt"), &content).unwrap();

        let tool = Grep::new(dir.path().to_path_buf());

        // First page
        let result = tool.call(GrepArgs {
            pattern: "match".to_string(),
            path: None,
            include_pattern: None,
            case_sensitive: false,
            line_numbers: true,
            context_before: None,
            context_after: None,
            offset: 0,
        }).await.unwrap();

        assert!(result.contains("Found 50 matches"));
        assert!(result.contains("page 1/3"));
        assert!(result.contains("offset=20"));

        // Second page
        let result = tool.call(GrepArgs {
            pattern: "match".to_string(),
            path: None,
            include_pattern: None,
            case_sensitive: false,
            line_numbers: true,
            context_before: None,
            context_after: None,
            offset: 20,
        }).await.unwrap();

        assert!(result.contains("page 2/3"));
    }

    #[tokio::test]
    async fn test_grep_context_lines() {
        let dir = setup_test_dir();
        let tool = Grep::new(dir.path().to_path_buf());

        let result = tool.call(GrepArgs {
            pattern: "println".to_string(),
            path: None,
            include_pattern: None,
            case_sensitive: false,
            line_numbers: true,
            context_before: Some(1),
            context_after: Some(1),
            offset: 0,
        }).await.unwrap();

        // Should include context lines around the match
        assert!(result.contains("fn main"));  // context before
        assert!(result.contains("println"));  // match
        assert!(result.contains("}"));        // context after
    }

    #[tokio::test]
    async fn test_grep_no_matches() {
        let dir = setup_test_dir();
        let tool = Grep::new(dir.path().to_path_buf());

        let result = tool.call(GrepArgs {
            pattern: "nonexistent_pattern_xyz".to_string(),
            path: None,
            include_pattern: None,
            case_sensitive: false,
            line_numbers: true,
            context_before: None,
            context_after: None,
            offset: 0,
        }).await.unwrap();

        assert!(result.contains("No matches found"));
    }

    #[tokio::test]
    async fn test_grep_invalid_regex() {
        let dir = setup_test_dir();
        let tool = Grep::new(dir.path().to_path_buf());

        let result = tool.call(GrepArgs {
            pattern: "[invalid(regex".to_string(),
            path: None,
            include_pattern: None,
            case_sensitive: false,
            line_numbers: true,
            context_before: None,
            context_after: None,
            offset: 0,
        }).await;

        assert!(matches!(result, Err(GrepError::InvalidPattern(_))));
    }
}
