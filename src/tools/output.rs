//! Structured tool outputs for ACP rendering
//!
//! Tools return structured data, and the presentation layer decides how to render:
//! - CLI: markdown/terminal formatted text
//! - ACP: native UI components (diff view, file viewer, terminal, etc.)
//! - Telemetry: JSON for storage/replay
//!
//! The LLM sees JSON-serialized output (via rig's Tool trait), but ACP clients
//! get rich rendering through ToolCallContent mapping.

use serde::{Deserialize, Serialize};

/// ACP tool kinds - determines how the client renders the tool call
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    /// File edit with diff view, keep/reject buttons
    Edit,
    /// File content display with syntax highlighting
    Read,
    /// Terminal output with ANSI handling
    Execute,
    /// File/match list with clickable locations
    Search,
    /// Web content fetch
    Fetch,
    /// Agent reasoning/thinking
    Think,
    /// Mode change UI
    SwitchMode,
    /// Generic fallback
    Other,
}

/// Location in a file (for clickable links)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileLocation {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<usize>,
}

// =============================================================================
// Edit Tool Output
// =============================================================================

/// Structured output for file edit operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditOutput {
    /// Path to the edited file
    pub path: String,
    /// Operation mode
    pub mode: EditMode,
    /// Original file content (None for new files)
    pub old_content: Option<String>,
    /// New file content after edit
    pub new_content: String,
    /// Number of lines added
    pub additions: usize,
    /// Number of lines removed
    pub deletions: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EditMode {
    Edit,
    Create,
    Overwrite,
}

impl EditOutput {
    pub fn kind() -> ToolKind {
        ToolKind::Edit
    }

    /// Generate unified diff for CLI display
    pub fn to_unified_diff(&self) -> String {
        use similar::{ChangeTag, TextDiff};

        let old = self.old_content.as_deref().unwrap_or("");
        let diff = TextDiff::from_lines(old, &self.new_content);

        let mut output = String::new();
        output.push_str(&format!("--- {}\n", self.path));
        output.push_str(&format!("+++ {}\n", self.path));

        for group in diff.grouped_ops(3) {
            for op in group {
                for change in diff.iter_changes(&op) {
                    match change.tag() {
                        ChangeTag::Delete => output.push_str(&format!("-{}", change)),
                        ChangeTag::Insert => output.push_str(&format!("+{}", change)),
                        ChangeTag::Equal => output.push_str(&format!(" {}", change)),
                    }
                }
            }
        }
        output
    }

    /// Render for CLI display
    pub fn to_cli_string(&self) -> String {
        let mode_str = match self.mode {
            EditMode::Edit => format!("Edited (+{} -{} lines)", self.additions, self.deletions),
            EditMode::Create => format!("Created ({} lines)", self.additions),
            EditMode::Overwrite => format!("Overwrote (+{} -{} lines)", self.additions, self.deletions),
        };
        format!("{}: {}\n\n```diff\n{}\n```", self.path, mode_str, self.to_unified_diff())
    }
}

// =============================================================================
// Read Tool Output
// =============================================================================

/// Structured output for file read operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadOutput {
    /// Path to the file
    pub path: String,
    /// File content
    pub content: String,
    /// Starting line (1-indexed, for partial reads)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_line: Option<usize>,
    /// Ending line (1-indexed, for partial reads)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_line: Option<usize>,
    /// Total lines in file
    pub total_lines: usize,
    /// Whether content was truncated
    pub truncated: bool,
}

impl ReadOutput {
    pub fn kind() -> ToolKind {
        ToolKind::Read
    }

    /// Render for CLI display
    pub fn to_cli_string(&self) -> String {
        let range = match (self.start_line, self.end_line) {
            (Some(start), Some(end)) => format!(" (lines {}-{})", start, end),
            (Some(start), None) => format!(" (from line {})", start),
            _ => String::new(),
        };

        let truncated = if self.truncated { " [truncated]" } else { "" };

        format!(
            "{}{}{}\n\n```\n{}\n```",
            self.path, range, truncated, self.content
        )
    }
}

// =============================================================================
// Terminal Tool Output
// =============================================================================

/// Structured output for terminal/bash operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalOutput {
    /// Command that was executed
    pub command: String,
    /// Standard output
    pub stdout: String,
    /// Standard error
    pub stderr: String,
    /// Exit code (None if still running or killed)
    pub exit_code: Option<i32>,
    /// Execution duration in milliseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// Whether output was truncated
    pub truncated: bool,
}

impl TerminalOutput {
    pub fn kind() -> ToolKind {
        ToolKind::Execute
    }

    /// Render for CLI display
    pub fn to_cli_string(&self) -> String {
        let mut output = format!("$ {}\n", self.command);

        if !self.stdout.is_empty() {
            output.push_str(&self.stdout);
            if !self.stdout.ends_with('\n') {
                output.push('\n');
            }
        }

        if !self.stderr.is_empty() {
            output.push_str(&format!("\nstderr:\n{}", self.stderr));
        }

        if let Some(code) = self.exit_code {
            if code != 0 {
                output.push_str(&format!("\n[exit code: {}]", code));
            }
        }

        if self.truncated {
            output.push_str("\n[output truncated]");
        }

        output
    }
}

// =============================================================================
// Search Tool Outputs (grep, find, ls)
// =============================================================================

/// A single match from grep
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrepMatch {
    pub path: String,
    pub line_number: usize,
    pub line_content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_before: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_after: Option<Vec<String>>,
}

/// Structured output for grep operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrepOutput {
    /// Pattern searched for
    pub pattern: String,
    /// Matches found
    pub matches: Vec<GrepMatch>,
    /// Total match count (may be > matches.len() if truncated)
    pub total_matches: usize,
    /// Whether results were truncated
    pub truncated: bool,
}

impl GrepOutput {
    pub fn kind() -> ToolKind {
        ToolKind::Search
    }

    /// Get file locations for ACP
    pub fn locations(&self) -> Vec<FileLocation> {
        self.matches
            .iter()
            .map(|m| FileLocation {
                path: m.path.clone(),
                line: Some(m.line_number),
                column: None,
            })
            .collect()
    }

    /// Render for CLI display
    pub fn to_cli_string(&self) -> String {
        let mut output = String::new();

        for m in &self.matches {
            output.push_str(&format!("{}:{}:{}\n", m.path, m.line_number, m.line_content));
        }

        if self.truncated {
            output.push_str(&format!(
                "\n... {} more matches (truncated)\n",
                self.total_matches - self.matches.len()
            ));
        }

        output
    }
}

/// Structured output for find/glob operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FindOutput {
    /// Pattern used
    pub pattern: String,
    /// Root path searched
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
    /// Files found
    pub files: Vec<String>,
    /// Total count (may be > files.len() if truncated)
    pub total_count: usize,
    /// Whether results were truncated
    pub truncated: bool,
}

impl FindOutput {
    pub fn kind() -> ToolKind {
        ToolKind::Search
    }

    /// Get file locations for ACP
    pub fn locations(&self) -> Vec<FileLocation> {
        self.files
            .iter()
            .map(|f| FileLocation {
                path: f.clone(),
                line: None,
                column: None,
            })
            .collect()
    }

    /// Render for CLI display
    pub fn to_cli_string(&self) -> String {
        let mut output = self.files.join("\n");

        if self.truncated {
            output.push_str(&format!(
                "\n... {} more files (truncated)\n",
                self.total_count - self.files.len()
            ));
        }

        output
    }
}

/// Directory entry for ls output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
}

/// Structured output for list directory operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListDirOutput {
    /// Directory listed
    pub path: String,
    /// Entries in the directory
    pub entries: Vec<DirEntry>,
    /// Whether results were truncated
    pub truncated: bool,
}

impl ListDirOutput {
    pub fn kind() -> ToolKind {
        ToolKind::Search
    }

    /// Render for CLI display
    pub fn to_cli_string(&self) -> String {
        let mut output = format!("{}:\n", self.path);

        for entry in &self.entries {
            let suffix = if entry.is_dir { "/" } else { "" };
            output.push_str(&format!("  {}{}\n", entry.name, suffix));
        }

        if self.truncated {
            output.push_str("  ... (truncated)\n");
        }

        output
    }
}

// =============================================================================
// Fetch Tool Output
// =============================================================================

/// Structured output for web fetch operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchOutput {
    /// URL fetched
    pub url: String,
    /// Response content
    pub content: String,
    /// HTTP status code
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_code: Option<u16>,
    /// Content type
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    /// Whether content was truncated
    pub truncated: bool,
}

impl FetchOutput {
    pub fn kind() -> ToolKind {
        ToolKind::Fetch
    }

    /// Render for CLI display
    pub fn to_cli_string(&self) -> String {
        let status = self
            .status_code
            .map(|s| format!(" [{}]", s))
            .unwrap_or_default();
        let truncated = if self.truncated { " [truncated]" } else { "" };

        format!("{}{}{}\n\n{}", self.url, status, truncated, self.content)
    }
}

// =============================================================================
// Web Search Tool Output
// =============================================================================

/// A single search result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
}

/// Structured output for web search operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebSearchOutput {
    /// Query searched
    pub query: String,
    /// Search results
    pub results: Vec<SearchResult>,
}

impl WebSearchOutput {
    pub fn kind() -> ToolKind {
        ToolKind::Fetch
    }

    /// Render for CLI display
    pub fn to_cli_string(&self) -> String {
        let mut output = format!("Search: {}\n\n", self.query);

        for (i, result) in self.results.iter().enumerate() {
            output.push_str(&format!("{}. {}\n", i + 1, result.title));
            output.push_str(&format!("   {}\n", result.url));
            if let Some(snippet) = &result.snippet {
                output.push_str(&format!("   {}\n", snippet));
            }
            output.push('\n');
        }

        output
    }
}

// =============================================================================
// Think Tool Output
// =============================================================================

/// Structured output for thinking/reasoning operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThinkOutput {
    /// The thinking content
    pub thought: String,
}

impl ThinkOutput {
    pub fn kind() -> ToolKind {
        ToolKind::Think
    }

    /// Render for CLI display
    pub fn to_cli_string(&self) -> String {
        self.thought.clone()
    }
}

// =============================================================================
// Todo Tool Output
// =============================================================================

/// A single todo item
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItem {
    pub content: String,
    pub status: TodoStatus,
    /// Present continuous form for display during execution
    pub active_form: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}

/// Structured output for todo operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoOutput {
    /// Current todo list
    pub todos: Vec<TodoItem>,
    /// What changed
    pub message: String,
}

impl TodoOutput {
    pub fn kind() -> ToolKind {
        ToolKind::Other // Special handling for plan updates
    }

    /// Render for CLI display
    pub fn to_cli_string(&self) -> String {
        let mut output = format!("{}\n\n", self.message);

        for todo in &self.todos {
            let status = match todo.status {
                TodoStatus::Pending => "[ ]",
                TodoStatus::InProgress => "[*]",
                TodoStatus::Completed => "[x]",
            };
            output.push_str(&format!("{} {}\n", status, todo.content));
        }

        output
    }
}

// =============================================================================
// Trait for unified handling
// =============================================================================

/// Trait for tool outputs that can render to different formats
pub trait ToolOutput: Serialize {
    /// ACP tool kind for this output
    fn kind(&self) -> ToolKind;

    /// Render for CLI display
    fn to_cli_string(&self) -> String;

    /// Get file locations for ACP (if applicable)
    fn locations(&self) -> Vec<FileLocation> {
        Vec::new()
    }
}

// Implement for all output types
impl ToolOutput for EditOutput {
    fn kind(&self) -> ToolKind { ToolKind::Edit }
    fn to_cli_string(&self) -> String { self.to_cli_string() }
    fn locations(&self) -> Vec<FileLocation> {
        vec![FileLocation { path: self.path.clone(), line: None, column: None }]
    }
}

impl ToolOutput for ReadOutput {
    fn kind(&self) -> ToolKind { ToolKind::Read }
    fn to_cli_string(&self) -> String { self.to_cli_string() }
    fn locations(&self) -> Vec<FileLocation> {
        vec![FileLocation { path: self.path.clone(), line: self.start_line, column: None }]
    }
}

impl ToolOutput for TerminalOutput {
    fn kind(&self) -> ToolKind { ToolKind::Execute }
    fn to_cli_string(&self) -> String { self.to_cli_string() }
}

impl ToolOutput for GrepOutput {
    fn kind(&self) -> ToolKind { ToolKind::Search }
    fn to_cli_string(&self) -> String { self.to_cli_string() }
    fn locations(&self) -> Vec<FileLocation> { self.locations() }
}

impl ToolOutput for FindOutput {
    fn kind(&self) -> ToolKind { ToolKind::Search }
    fn to_cli_string(&self) -> String { self.to_cli_string() }
    fn locations(&self) -> Vec<FileLocation> { self.locations() }
}

impl ToolOutput for ListDirOutput {
    fn kind(&self) -> ToolKind { ToolKind::Search }
    fn to_cli_string(&self) -> String { self.to_cli_string() }
    fn locations(&self) -> Vec<FileLocation> {
        vec![FileLocation { path: self.path.clone(), line: None, column: None }]
    }
}

impl ToolOutput for FetchOutput {
    fn kind(&self) -> ToolKind { ToolKind::Fetch }
    fn to_cli_string(&self) -> String { self.to_cli_string() }
}

impl ToolOutput for WebSearchOutput {
    fn kind(&self) -> ToolKind { ToolKind::Fetch }
    fn to_cli_string(&self) -> String { self.to_cli_string() }
}

impl ToolOutput for ThinkOutput {
    fn kind(&self) -> ToolKind { ToolKind::Think }
    fn to_cli_string(&self) -> String { self.to_cli_string() }
}

impl ToolOutput for TodoOutput {
    fn kind(&self) -> ToolKind { ToolKind::Other }
    fn to_cli_string(&self) -> String { self.to_cli_string() }
}
