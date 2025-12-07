//! Terminal tool - Execute shell commands

use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use tokio::process::Command;
use tokio::io::AsyncReadExt;

const OUTPUT_LIMIT: usize = 16 * 1024; // 16KB limit on output
const TIMEOUT_SECS: u64 = 120; // 2 minute timeout

#[derive(Debug, thiserror::Error)]
pub enum TerminalError {
    #[error("Command failed with exit code {0}: {1}")]
    NonZeroExit(i32, String),
    #[error("Command timed out after {0} seconds")]
    Timeout(u64),
    #[error("Failed to spawn process: {0}")]
    SpawnFailed(String),
    #[error("IO error: {0}")]
    Io(String),
    #[error("Working directory not found: {0}")]
    WorkDirNotFound(String),
}

#[derive(Debug, Deserialize)]
pub struct TerminalArgs {
    /// The shell command to execute
    pub command: String,

    /// Working directory for the command (relative to project root)
    #[serde(default)]
    pub cd: Option<String>,

    /// Timeout in seconds (default: 120)
    #[serde(default)]
    pub timeout: Option<u64>,
}

/// Tool for executing shell commands
#[derive(Debug, Clone)]
pub struct Terminal {
    working_dir: Arc<PathBuf>,
}

impl Terminal {
    pub fn new(working_dir: PathBuf) -> Self {
        Self {
            working_dir: Arc::new(working_dir),
        }
    }

    fn resolve_working_dir(&self, cd: Option<&str>) -> Result<PathBuf, TerminalError> {
        match cd {
            Some(dir) if !dir.is_empty() && dir != "." => {
                let path = self.working_dir.join(dir);
                if path.is_dir() {
                    Ok(path)
                } else {
                    Err(TerminalError::WorkDirNotFound(dir.to_string()))
                }
            }
            _ => Ok(self.working_dir.as_ref().clone()),
        }
    }
}

impl Serialize for Terminal {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_unit()
    }
}

impl<'de> Deserialize<'de> for Terminal {
    fn deserialize<D>(_deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Ok(Self::new(std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))))
    }
}

impl Tool for Terminal {
    const NAME: &'static str = "terminal";

    type Error = TerminalError;
    type Args = TerminalArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "terminal".to_string(),
            description: r#"Execute a shell command and return the output.

Use this for:
- Running build commands (cargo build, npm install, etc.)
- Running tests (cargo test, pytest, etc.)
- File operations that are easier via shell
- Inspecting system state (ps, df, etc.)

IMPORTANT:
- Do NOT run commands that don't terminate (servers, watchers)
- Each invocation starts a fresh shell (no state persists)
- Use the 'cd' parameter to change directory, not 'cd' in the command
- Output is limited to 16KB"#.to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The shell command to execute"
                    },
                    "cd": {
                        "type": "string",
                        "description": "Working directory (relative to project root). Optional."
                    },
                    "timeout": {
                        "type": "integer",
                        "description": "Timeout in seconds (default: 120)"
                    }
                },
                "required": ["command"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let work_dir = self.resolve_working_dir(args.cd.as_deref())?;
        let timeout_secs = args.timeout.unwrap_or(TIMEOUT_SECS);

        // Spawn the command
        let mut child = Command::new("sh")
            .arg("-c")
            .arg(&args.command)
            .current_dir(&work_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| TerminalError::SpawnFailed(e.to_string()))?;

        // Set up timeout
        let timeout = tokio::time::Duration::from_secs(timeout_secs);

        let result = tokio::time::timeout(timeout, async {
            let mut stdout = child.stdout.take().unwrap();
            let mut stderr = child.stderr.take().unwrap();

            let mut stdout_buf = Vec::new();
            let mut stderr_buf = Vec::new();

            // Read stdout and stderr
            let (stdout_result, stderr_result) = tokio::join!(
                stdout.read_to_end(&mut stdout_buf),
                stderr.read_to_end(&mut stderr_buf)
            );

            stdout_result.map_err(|e| TerminalError::Io(e.to_string()))?;
            stderr_result.map_err(|e| TerminalError::Io(e.to_string()))?;

            let status = child.wait().await.map_err(|e| TerminalError::Io(e.to_string()))?;

            Ok::<_, TerminalError>((status, stdout_buf, stderr_buf))
        }).await;

        match result {
            Ok(Ok((status, stdout_buf, stderr_buf))) => {
                let stdout = String::from_utf8_lossy(&stdout_buf);
                let stderr = String::from_utf8_lossy(&stderr_buf);

                // Combine output
                let mut output = String::new();

                if !stdout.is_empty() {
                    output.push_str(&stdout);
                }

                if !stderr.is_empty() {
                    if !output.is_empty() {
                        output.push_str("\n--- stderr ---\n");
                    }
                    output.push_str(&stderr);
                }

                // Truncate if too long
                let truncated = if output.len() > OUTPUT_LIMIT {
                    format!(
                        "{}...\n\n[Output truncated at {} bytes]",
                        &output[..OUTPUT_LIMIT],
                        OUTPUT_LIMIT
                    )
                } else {
                    output
                };

                if status.success() {
                    if truncated.is_empty() {
                        Ok("Command completed successfully (no output)".to_string())
                    } else {
                        Ok(truncated)
                    }
                } else {
                    let code = status.code().unwrap_or(-1);
                    Err(TerminalError::NonZeroExit(code, truncated))
                }
            }
            Ok(Err(e)) => Err(e),
            Err(_) => {
                // Timeout - try to kill the process
                let _ = child.kill().await;
                Err(TerminalError::Timeout(timeout_secs))
            }
        }
    }
}
