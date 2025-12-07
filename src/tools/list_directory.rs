//! List directory tool - List files and directories

use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum ListDirectoryError {
    #[error("Directory not found: {0}")]
    NotFound(String),
    #[error("Not a directory: {0}")]
    NotADirectory(String),
    #[error("Permission denied: {0}")]
    PermissionDenied(String),
    #[error("IO error: {0}")]
    Io(String),
    #[error("Path is outside working directory: {0}")]
    OutsideWorkDir(String),
}

#[derive(Debug, Deserialize)]
pub struct ListDirectoryArgs {
    /// The path to list (relative to working directory). Use "." for current directory.
    pub path: String,

    /// Include hidden files (starting with .)
    #[serde(default)]
    pub show_hidden: bool,

    /// Maximum depth for recursive listing (0 = current dir only, default)
    #[serde(default)]
    pub depth: Option<u32>,
}

/// Tool for listing directory contents
#[derive(Debug, Clone)]
pub struct ListDirectory {
    working_dir: Arc<PathBuf>,
}

impl ListDirectory {
    pub fn new(working_dir: PathBuf) -> Self {
        Self {
            working_dir: Arc::new(working_dir),
        }
    }

    fn resolve_path(&self, path: &str) -> Result<PathBuf, ListDirectoryError> {
        let requested = if path.is_empty() || path == "." {
            self.working_dir.as_ref().clone()
        } else {
            let p = PathBuf::from(path);
            if p.is_absolute() {
                p
            } else {
                self.working_dir.join(p)
            }
        };

        let canonical = requested.canonicalize().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                ListDirectoryError::NotFound(path.to_string())
            } else {
                ListDirectoryError::Io(e.to_string())
            }
        })?;

        let working_canonical = self.working_dir.canonicalize().map_err(|e| {
            ListDirectoryError::Io(format!("Cannot resolve working directory: {}", e))
        })?;

        if !canonical.starts_with(&working_canonical) {
            return Err(ListDirectoryError::OutsideWorkDir(path.to_string()));
        }

        Ok(canonical)
    }

    fn list_recursive(
        &self,
        path: &PathBuf,
        base: &PathBuf,
        show_hidden: bool,
        max_depth: u32,
        current_depth: u32,
        output: &mut Vec<String>,
    ) -> Result<(), ListDirectoryError> {
        if current_depth > max_depth {
            return Ok(());
        }

        let entries = std::fs::read_dir(path).map_err(|e| {
            match e.kind() {
                std::io::ErrorKind::PermissionDenied => {
                    ListDirectoryError::PermissionDenied(path.display().to_string())
                }
                _ => ListDirectoryError::Io(e.to_string()),
            }
        })?;

        let mut items: Vec<_> = entries
            .filter_map(|e| e.ok())
            .collect();

        // Sort by name
        items.sort_by(|a, b| a.file_name().cmp(&b.file_name()));

        for entry in items {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();

            // Skip hidden files unless requested
            if !show_hidden && name_str.starts_with('.') {
                continue;
            }

            let entry_path = entry.path();
            let relative = entry_path.strip_prefix(base).unwrap_or(&entry_path);
            let is_dir = entry_path.is_dir();

            let prefix = "  ".repeat(current_depth as usize);
            let suffix = if is_dir { "/" } else { "" };

            output.push(format!("{}{}{}", prefix, relative.display(), suffix));

            if is_dir && current_depth < max_depth {
                self.list_recursive(
                    &entry_path,
                    base,
                    show_hidden,
                    max_depth,
                    current_depth + 1,
                    output,
                )?;
            }
        }

        Ok(())
    }
}

impl Serialize for ListDirectory {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_unit()
    }
}

impl<'de> Deserialize<'de> for ListDirectory {
    fn deserialize<D>(_deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Ok(Self::new(std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))))
    }
}

impl Tool for ListDirectory {
    const NAME: &'static str = "list_directory";

    type Error = ListDirectoryError;
    type Args = ListDirectoryArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "list_directory".to_string(),
            description: r#"List the contents of a directory.

Use this to explore the project structure and find files.
Directories are shown with a trailing '/'.

Use depth parameter for recursive listing:
- depth=0 (default): current directory only
- depth=1: include immediate subdirectories
- depth=2+: deeper nesting"#.to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Directory path (relative to project root). Use '.' for current directory."
                    },
                    "show_hidden": {
                        "type": "boolean",
                        "description": "Include hidden files (starting with '.') Default: false"
                    },
                    "depth": {
                        "type": "integer",
                        "description": "Maximum recursion depth (0 = current dir only). Default: 0"
                    }
                },
                "required": ["path"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let path = self.resolve_path(&args.path)?;

        if !path.is_dir() {
            return Err(ListDirectoryError::NotADirectory(args.path));
        }

        let max_depth = args.depth.unwrap_or(0);
        let mut output = Vec::new();

        // Add header
        output.push(format!("# {}/", args.path));
        output.push(String::new());

        self.list_recursive(
            &path,
            &path,
            args.show_hidden,
            max_depth,
            0,
            &mut output,
        )?;

        if output.len() == 2 {
            output.push("(empty directory)".to_string());
        }

        Ok(output.join("\n"))
    }
}
