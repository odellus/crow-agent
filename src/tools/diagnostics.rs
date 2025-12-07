//! Diagnostics tool - get errors/warnings from language servers via LSP

use crate::lsp::{LspClient, LspError};
use lsp_types::DiagnosticSeverity;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use parking_lot::Mutex;
use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum DiagnosticsError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("LSP error: {0}")]
    Lsp(#[from] LspError),
    #[error("Unknown project type - no supported language server found")]
    UnknownProject,
    #[error("Language server not available: {0}")]
    ServerNotAvailable(String),
}

/// Get diagnostics (errors/warnings) for a file or project
#[derive(Debug, Serialize, Deserialize)]
pub struct DiagnosticsInput {
    /// Path to check (file or directory). If empty, checks current directory.
    #[serde(default)]
    pub path: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ProjectType {
    Rust,
    TypeScript,
    JavaScript,
    Python,
    Go,
}

impl ProjectType {
    fn language_id(&self) -> &'static str {
        match self {
            ProjectType::Rust => "rust",
            ProjectType::TypeScript => "typescript",
            ProjectType::JavaScript => "javascript",
            ProjectType::Python => "python",
            ProjectType::Go => "go",
        }
    }

    fn file_extensions(&self) -> &[&'static str] {
        match self {
            ProjectType::Rust => &["rs"],
            ProjectType::TypeScript => &["ts", "tsx"],
            ProjectType::JavaScript => &["js", "jsx"],
            ProjectType::Python => &["py"],
            ProjectType::Go => &["go"],
        }
    }
}

/// Server configuration for different languages
struct ServerConfig {
    command: &'static str,
    args: &'static [&'static str],
}

impl ProjectType {
    fn server_config(&self) -> ServerConfig {
        match self {
            ProjectType::Rust => ServerConfig {
                command: "rust-analyzer",
                args: &[],
            },
            ProjectType::TypeScript | ProjectType::JavaScript => ServerConfig {
                command: "typescript-language-server",
                args: &["--stdio"],
            },
            ProjectType::Python => ServerConfig {
                command: "pyright-langserver",
                args: &["--stdio"],
            },
            ProjectType::Go => ServerConfig {
                command: "gopls",
                args: &[],
            },
        }
    }
}

#[derive(Clone)]
pub struct Diagnostics {
    working_dir: PathBuf,
    /// Cache of LSP clients by project type
    clients: Arc<Mutex<HashMap<ProjectType, LspClient>>>,
}

impl Diagnostics {
    pub fn new(working_dir: PathBuf) -> Self {
        Self {
            working_dir,
            clients: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn detect_project_type(&self, path: Option<&Path>) -> Option<ProjectType> {
        let check_dir = path.unwrap_or(&self.working_dir);

        // Check for project files
        if check_dir.join("Cargo.toml").exists() {
            return Some(ProjectType::Rust);
        }
        if check_dir.join("package.json").exists() {
            if check_dir.join("tsconfig.json").exists() {
                return Some(ProjectType::TypeScript);
            }
            return Some(ProjectType::JavaScript);
        }
        if check_dir.join("pyproject.toml").exists()
            || check_dir.join("setup.py").exists()
            || check_dir.join("requirements.txt").exists()
        {
            return Some(ProjectType::Python);
        }
        if check_dir.join("go.mod").exists() {
            return Some(ProjectType::Go);
        }

        // Check file extension if path is a file
        if let Some(p) = path {
            if p.is_file() {
                if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
                    return match ext {
                        "rs" => Some(ProjectType::Rust),
                        "ts" | "tsx" => Some(ProjectType::TypeScript),
                        "js" | "jsx" => Some(ProjectType::JavaScript),
                        "py" => Some(ProjectType::Python),
                        "go" => Some(ProjectType::Go),
                        _ => None,
                    };
                }
            }
        }

        None
    }

    fn get_or_create_client(&self, project_type: ProjectType) -> Result<(), DiagnosticsError> {
        let mut clients = self.clients.lock();

        if clients.contains_key(&project_type) {
            return Ok(());
        }

        let config = project_type.server_config();

        // Check if server is available
        if which::which(config.command).is_err() {
            return Err(DiagnosticsError::ServerNotAvailable(format!(
                "{} not found in PATH. Install it to get {} diagnostics.",
                config.command,
                project_type.language_id()
            )));
        }

        let mut client = LspClient::new(config.command, config.args, &self.working_dir)?;
        client.initialize()?;

        clients.insert(project_type, client);
        Ok(())
    }

    fn collect_files(&self, path: Option<&Path>, project_type: ProjectType) -> Vec<PathBuf> {
        let extensions = project_type.file_extensions();
        let start_path = path.unwrap_or(&self.working_dir);

        if start_path.is_file() {
            return vec![start_path.to_path_buf()];
        }

        let mut files = Vec::new();
        self.collect_files_recursive(start_path, extensions, &mut files);
        files
    }

    fn collect_files_recursive(&self, dir: &Path, extensions: &[&str], files: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };

        for entry in entries.flatten() {
            let path = entry.path();

            // Skip common non-source directories
            if path.is_dir() {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if matches!(name, "target" | "node_modules" | ".git" | "__pycache__" | "vendor" | ".venv" | "venv") {
                    continue;
                }
                self.collect_files_recursive(&path, extensions, files);
            } else if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                if extensions.contains(&ext) {
                    files.push(path);
                }
            }
        }
    }

    fn format_diagnostics(&self, all_diagnostics: &HashMap<String, Vec<lsp_types::Diagnostic>>) -> String {
        if all_diagnostics.is_empty() {
            return "No errors or warnings found.".to_string();
        }

        let mut output = String::new();
        let mut error_count = 0;
        let mut warning_count = 0;

        for (path, diagnostics) in all_diagnostics {
            if diagnostics.is_empty() {
                continue;
            }

            // Make path relative if possible
            let display_path = if let Ok(relative) = Path::new(path).strip_prefix(&self.working_dir) {
                relative.display().to_string()
            } else {
                path.clone()
            };

            for diag in diagnostics {
                let severity = match diag.severity {
                    Some(DiagnosticSeverity::ERROR) => {
                        error_count += 1;
                        "error"
                    }
                    Some(DiagnosticSeverity::WARNING) => {
                        warning_count += 1;
                        "warning"
                    }
                    Some(DiagnosticSeverity::INFORMATION) => "info",
                    Some(DiagnosticSeverity::HINT) => "hint",
                    _ => "note",
                };

                let line = diag.range.start.line + 1;
                let col = diag.range.start.character + 1;

                output.push_str(&format!(
                    "{}:{}:{}: {}: {}\n",
                    display_path, line, col, severity, diag.message
                ));

                // Add related information if present
                if let Some(related) = &diag.related_information {
                    for info in related {
                        let rel_path = info.location.uri.as_str();
                        let rel_line = info.location.range.start.line + 1;
                        output.push_str(&format!(
                            "  --> {}:{}: {}\n",
                            rel_path, rel_line, info.message
                        ));
                    }
                }
            }
        }

        if output.is_empty() {
            "No errors or warnings found.".to_string()
        } else {
            format!(
                "{}\n---\nFound {} error(s), {} warning(s)",
                output.trim(),
                error_count,
                warning_count
            )
        }
    }

    async fn get_diagnostics_for_project(
        &self,
        path: Option<&Path>,
        project_type: ProjectType,
    ) -> Result<String, DiagnosticsError> {
        // Ensure client is started
        self.get_or_create_client(project_type)?;

        let files = self.collect_files(path, project_type);

        if files.is_empty() {
            return Ok("No source files found.".to_string());
        }

        // Open all files to trigger diagnostics
        {
            let mut clients = self.clients.lock();
            let client = clients.get_mut(&project_type).unwrap();

            for file_path in &files {
                let content = fs::read_to_string(file_path)?;
                client.open_document(file_path, &content, project_type.language_id())?;
            }
        }

        // Give the server time to compute diagnostics
        // In a real implementation, we'd wait for the server to signal completion
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Collect diagnostics
        let all_diagnostics = {
            let clients = self.clients.lock();
            let client = clients.get(&project_type).unwrap();
            client.get_all_diagnostics()
        };

        Ok(self.format_diagnostics(&all_diagnostics))
    }
}

impl Tool for Diagnostics {
    const NAME: &'static str = "diagnostics";

    type Error = DiagnosticsError;
    type Args = DiagnosticsInput;
    type Output = String;

    async fn definition(&self, _prompt: String) -> rig::completion::ToolDefinition {
        rig::completion::ToolDefinition {
            name: Self::NAME.to_string(),
            description: r#"Get errors and warnings for the project or a specific file using Language Server Protocol (LSP).

Automatically detects project type and starts the appropriate language server:
- Rust: rust-analyzer
- TypeScript/JavaScript: typescript-language-server
- Python: pyright-langserver
- Go: gopls

Use after making edits to check for errors. Returns diagnostics with file locations, severity, and messages."#.to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to check (file or directory). If empty, checks entire project."
                    }
                }
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let path = args.path.as_ref().map(|p| {
            if Path::new(p).is_absolute() {
                PathBuf::from(p)
            } else {
                self.working_dir.join(p)
            }
        });

        let project_type = self.detect_project_type(path.as_deref())
            .ok_or(DiagnosticsError::UnknownProject)?;

        self.get_diagnostics_for_project(path.as_deref(), project_type).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_rust_project() {
        let diag = Diagnostics::new(PathBuf::from(env!("CARGO_MANIFEST_DIR")));
        assert_eq!(diag.detect_project_type(None), Some(ProjectType::Rust));
    }

    #[test]
    fn test_file_extensions() {
        assert!(ProjectType::Rust.file_extensions().contains(&"rs"));
        assert!(ProjectType::Python.file_extensions().contains(&"py"));
        assert!(ProjectType::TypeScript.file_extensions().contains(&"ts"));
    }
}
