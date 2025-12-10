//! System prompt construction for agents
//!
//! Based on OpenCode's approach - uses model-specific base prompts
//! with environment context appended.

use std::path::Path;

/// Get the base system prompt for a given model
///
/// Matches model ID patterns to select the appropriate prompt.
/// Falls back to qwen.txt for unknown models.
pub fn get_base_prompt(model_id: &str) -> &'static str {
    if model_id.contains("claude") {
        include_str!("../prompts/anthropic.txt")
    } else if model_id.contains("gpt-5") {
        include_str!("../prompts/codex.txt")
    } else if model_id.contains("gpt-") || model_id.contains("o1") || model_id.contains("o3") {
        include_str!("../prompts/beast.txt")
    } else if model_id.contains("gemini") {
        include_str!("../prompts/gemini.txt")
    } else if model_id.contains("polaris") {
        include_str!("../prompts/polaris.txt")
    } else {
        // Default: qwen.txt works well for most models
        include_str!("../prompts/qwen.txt")
    }
}

/// Get the anthropic spoof header (for Claude models via proxy)
pub fn get_anthropic_header() -> &'static str {
    include_str!("../prompts/anthropic_spoof.txt")
}

/// Build the complete system prompt for an agent
pub fn build_system_prompt(
    model_id: &str,
    working_dir: &Path,
    custom_prompt: Option<&str>,
) -> String {
    let mut parts = Vec::new();

    // Use custom prompt if provided, otherwise use model-based prompt
    let base = custom_prompt.unwrap_or_else(|| get_base_prompt(model_id));
    parts.push(base.to_string());

    // Add environment context
    parts.push(build_environment_context(working_dir));

    // Add custom instructions if found
    if let Some(instructions) = load_custom_instructions(working_dir) {
        parts.push(instructions);
    }

    parts.join("\n\n")
}

/// Build environment context section (includes file tree like opencode)
fn build_environment_context(working_dir: &Path) -> String {
    let mut lines = vec![
        "Here is useful information about the environment you are running in:".to_string(),
        "<env>".to_string(),
    ];

    lines.push(format!("  Working directory: {}", working_dir.display()));

    // Git repo check
    let is_git = std::process::Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .current_dir(working_dir)
        .output()
        .ok()
        .map(|o| o.status.success())
        .unwrap_or(false);

    lines.push(format!(
        "  Is directory a git repo: {}",
        if is_git { "yes" } else { "no" }
    ));

    lines.push(format!("  Platform: {}", std::env::consts::OS));

    // Today's date
    let date = chrono::Local::now().format("%Y-%m-%d").to_string();
    lines.push(format!("  Today's date: {}", date));

    lines.push("</env>".to_string());

    // Add file tree like opencode does
    if is_git {
        lines.push("<files>".to_string());
        if let Some(tree) = build_file_tree(working_dir, 200) {
            lines.push(format!("  {}", tree));
        }
        lines.push("</files>".to_string());
    }

    lines.join("\n")
}

/// Build file tree using ripgrep (like opencode)
fn build_file_tree(working_dir: &Path, limit: usize) -> Option<String> {
    // Use rg --files to get file list, respecting .gitignore
    let output = std::process::Command::new("rg")
        .args(["--files", "--sort", "path"])
        .current_dir(working_dir)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let files: Vec<&str> = std::str::from_utf8(&output.stdout)
        .ok()?
        .lines()
        .take(limit)
        .collect();

    if files.is_empty() {
        return None;
    }

    Some(files.join("\n  "))
}

/// Load custom instructions from AGENTS.md, CLAUDE.md, etc.
fn load_custom_instructions(working_dir: &Path) -> Option<String> {
    let files_to_check = ["AGENTS.md", "CLAUDE.md", "CONTEXT.md", ".crow/AGENTS.md"];

    for filename in &files_to_check {
        let path = working_dir.join(filename);
        if path.exists() {
            if let Ok(content) = std::fs::read_to_string(&path) {
                return Some(format!(
                    "# Custom Instructions (from {})\n\n{}",
                    filename, content
                ));
            }
        }
    }

    // Check global config
    if let Some(config_dir) = dirs::config_dir() {
        let global_path = config_dir.join("crow").join("AGENTS.md");
        if global_path.exists() {
            if let Ok(content) = std::fs::read_to_string(&global_path) {
                return Some(format!(
                    "# Custom Instructions (from ~/.config/crow/AGENTS.md)\n\n{}",
                    content
                ));
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_base_prompt_claude() {
        let prompt = get_base_prompt("claude-3-5-sonnet");
        assert!(!prompt.is_empty());
    }

    #[test]
    fn test_get_base_prompt_qwen() {
        let prompt = get_base_prompt("qwen-2.5-72b");
        assert!(!prompt.is_empty());
    }

    #[test]
    fn test_get_base_prompt_gpt() {
        let prompt = get_base_prompt("gpt-4o");
        assert!(!prompt.is_empty());
    }

    #[test]
    fn test_build_environment_context() {
        let ctx = build_environment_context(Path::new("/tmp"));
        assert!(ctx.contains("<env>"));
        assert!(ctx.contains("</env>"));
        assert!(ctx.contains("Working directory:"));
        assert!(ctx.contains("Platform:"));
    }

    #[test]
    fn test_build_system_prompt() {
        let prompt = build_system_prompt("test-model", Path::new("/tmp"), None);
        assert!(!prompt.is_empty());
        assert!(prompt.contains("<env>")); // Has environment context
    }

    #[test]
    fn test_build_system_prompt_with_custom() {
        let prompt = build_system_prompt(
            "test-model",
            Path::new("/tmp"),
            Some("Custom system prompt here"),
        );
        assert!(prompt.contains("Custom system prompt here"));
        assert!(prompt.contains("<env>")); // Still has environment context
    }
}
