//! Bash tool - executes shell commands
//!
//! Based on crow-old's bash tool with full cancellation support.

use crate::tool::{Tool, ToolContext, ToolDefinition, ToolResult};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use std::process::Stdio;
use tokio::process::Command;

const DEFAULT_TIMEOUT_MS: u64 = 120_000; // 2 minutes
const MAX_TIMEOUT_MS: u64 = 600_000; // 10 minutes
const MAX_OUTPUT_LENGTH: usize = 30_000;

#[derive(Debug, Deserialize)]
struct Args {
    command: String,
    #[serde(default)]
    timeout: Option<u64>,
    #[serde(default)]
    #[allow(dead_code)]
    description: Option<String>,
}

pub struct BashTool;

impl BashTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for BashTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &str {
        "bash"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "bash".to_string(),
            description: r#"Executes a given bash command in a persistent shell session with optional timeout, ensuring proper handling and security measures.

Before executing the command, please follow these steps:

1. Directory Verification:
   - If the command will create new directories or files, first use the List tool to verify the parent directory exists and is the correct location
   - For example, before running "mkdir foo/bar", first use List to check that "foo" exists and is the intended parent directory

2. Command Execution:
   - Always quote file paths that contain spaces with double quotes (e.g., cd "path with spaces/file.txt")
   - Examples of proper quoting:
     - cd "/Users/name/My Documents" (correct)
     - cd /Users/name/My Documents (incorrect - will fail)
     - python "/path/with spaces/script.py" (correct)
     - python /path/with spaces/script.py (incorrect - will fail)
   - After ensuring proper quoting, execute the command.
   - Capture the output of the command.

Usage notes:
  - The command argument is required.
  - You can specify an optional timeout in milliseconds (up to 600000ms / 10 minutes). If not specified, commands will timeout after 120000ms (2 minutes).
  - It is very helpful if you write a clear, concise description of what this command does in 5-10 words.
  - If the output exceeds 30000 characters, output will be truncated before being returned to you.
  - VERY IMPORTANT: You MUST avoid using search commands like `find` and `grep`. Instead use Grep, Glob, or Task to search. You MUST avoid read tools like `cat`, `head`, `tail`, and `ls`, and use Read and List to read files.
  - If you _still_ need to run `grep`, STOP. ALWAYS USE ripgrep at `rg` (or /usr/bin/rg) first, which all opencode users have pre-installed.
  - When issuing multiple commands, use the ';' or '&&' operator to separate them. DO NOT use newlines (newlines are ok in quoted strings).
  - Try to maintain your current working directory throughout the session by using absolute paths and avoiding usage of `cd`. You may use `cd` if the User explicitly requests it.

# Committing changes with git

When the user asks you to create a new git commit, follow these steps carefully:

1. You have the capability to call multiple tools in a single response. When multiple independent pieces of information are requested, batch your tool calls together for optimal performance. ALWAYS run the following bash commands in parallel, each using the Bash tool:
   - Run a git status command to see all untracked files.
   - Run a git diff command to see both staged and unstaged changes that will be committed.
   - Run a git log command to see recent commit messages, so that you can follow this repository's commit message style.

2. Analyze all staged changes (both previously staged and newly added) and draft a commit message. Wrap your analysis process in <commit_analysis> tags.

3. You have the capability to call multiple tools in a single response. When multiple independent pieces of information are requested, batch your tool calls together for optimal performance. ALWAYS run the following commands in parallel:
   - Add relevant untracked files to the staging area.
   - Run git status to make sure the commit succeeded.

4. If the commit fails due to pre-commit hook changes, retry the commit ONCE to include these automated changes.

# Creating pull requests

Use the gh command via the Bash tool for ALL GitHub-related tasks including working with issues, pull requests, checks, and releases. If given a Github URL use the gh command to get the information needed.

IMPORTANT: When the user asks you to create a pull request, follow these steps carefully:

1. Run bash commands in parallel using the Bash tool to understand the current state of the branch since it diverged from the main branch.

2. Analyze all changes that will be included in the pull request, making sure to look at all relevant commits. Wrap your analysis process in <pr_analysis> tags.

3. Run the following commands in parallel:
   - Create new branch if needed
   - Push to remote with -u flag if needed
   - Create PR using gh pr create"#.to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The command to execute"
                    },
                    "timeout": {
                        "type": "number",
                        "description": "Optional timeout in milliseconds (max 600000ms / 10 minutes, default 120000ms / 2 minutes)"
                    },
                    "description": {
                        "type": "string",
                        "description": "Clear, concise description of what this command does in 5-10 words"
                    }
                },
                "required": ["command", "description"]
            }),
        }
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        // Check cancellation before starting
        if ctx.is_cancelled() {
            return ToolResult::error("Cancelled");
        }

        let args: Args = match serde_json::from_value(args) {
            Ok(a) => a,
            Err(e) => return ToolResult::error(format!("Invalid arguments: {}", e)),
        };

        // Calculate timeout
        let timeout_ms = args
            .timeout
            .map(|t| t.min(MAX_TIMEOUT_MS))
            .unwrap_or(DEFAULT_TIMEOUT_MS);
        let timeout_duration = std::time::Duration::from_millis(timeout_ms);

        // Spawn the command
        let child = match Command::new("bash")
            .arg("-c")
            .arg(&args.command)
            .current_dir(&ctx.working_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true) // Ensure cleanup
            .spawn()
        {
            Ok(child) => child,
            Err(e) => return ToolResult::error(format!("Failed to spawn command: {}", e)),
        };

        // Wait with timeout and cancellation support
        let output = tokio::select! {
            biased;

            // Cancellation takes priority
            _ = ctx.cancellation.cancelled() => {
                // kill_on_drop handles cleanup
                return ToolResult::error("Command was cancelled");
            }

            // Timeout
            _ = tokio::time::sleep(timeout_duration) => {
                // kill_on_drop handles cleanup
                return ToolResult::error(format!("Command timed out after {}ms", timeout_ms));
            }

            // Command completion
            result = child.wait_with_output() => {
                match result {
                    Ok(output) => output,
                    Err(e) => return ToolResult::error(format!("Failed to execute command: {}", e)),
                }
            }
        };

        // Process output
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let exit_code = output.status.code().unwrap_or(-1);

        // Combine stdout and stderr
        let mut combined = stdout.to_string();
        if !stderr.is_empty() {
            if !combined.is_empty() {
                combined.push('\n');
            }
            combined.push_str(&stderr);
        }

        // Truncate if needed
        if combined.len() > MAX_OUTPUT_LENGTH {
            combined.truncate(MAX_OUTPUT_LENGTH);
            combined.push_str("\n\n(Output truncated due to length limit)");
        }

        if exit_code == 0 {
            ToolResult::success(combined)
        } else {
            ToolResult::error(format!(
                "Command exited with code {}\n{}",
                exit_code, combined
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tokio_util::sync::CancellationToken;

    fn test_ctx() -> ToolContext {
        ToolContext::new(PathBuf::from("/tmp"), CancellationToken::new())
    }

    #[tokio::test]
    async fn test_bash_echo() {
        let tool = BashTool::new();
        let result = tool
            .execute(
                json!({
                    "command": "echo 'hello world'"
                }),
                &test_ctx(),
            )
            .await;

        assert!(!result.is_error);
        assert!(result.output.contains("hello world"));
    }

    #[tokio::test]
    async fn test_bash_exit_code() {
        let tool = BashTool::new();
        let result = tool
            .execute(
                json!({
                    "command": "exit 42"
                }),
                &test_ctx(),
            )
            .await;

        assert!(result.is_error);
        assert!(result.output.contains("42"));
    }

    #[tokio::test]
    async fn test_bash_stderr() {
        let tool = BashTool::new();
        let result = tool
            .execute(
                json!({
                    "command": "echo 'error' >&2"
                }),
                &test_ctx(),
            )
            .await;

        assert!(!result.is_error);
        assert!(result.output.contains("error"));
    }

    #[tokio::test]
    async fn test_bash_working_dir() {
        let tool = BashTool::new();
        let result = tool
            .execute(
                json!({
                    "command": "pwd"
                }),
                &test_ctx(),
            )
            .await;

        assert!(!result.is_error);
        assert!(result.output.contains("/tmp"));
    }

    #[tokio::test]
    async fn test_bash_cancellation() {
        let tool = BashTool::new();
        let cancel = CancellationToken::new();
        let ctx = ToolContext::new(PathBuf::from("/tmp"), cancel.clone());

        // Cancel after 50ms
        let cancel_clone = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            cancel_clone.cancel();
        });

        let start = std::time::Instant::now();
        let result = tool
            .execute(
                json!({
                    "command": "sleep 10"
                }),
                &ctx,
            )
            .await;

        let elapsed = start.elapsed();
        assert!(result.is_error);
        assert!(result.output.contains("cancel"));
        assert!(elapsed.as_millis() < 1000); // Should be quick
    }

    #[tokio::test]
    async fn test_bash_timeout() {
        let tool = BashTool::new();
        let result = tool
            .execute(
                json!({
                    "command": "sleep 10",
                    "timeout": 100
                }),
                &test_ctx(),
            )
            .await;

        assert!(result.is_error);
        assert!(result.output.contains("timed out"));
    }
}
