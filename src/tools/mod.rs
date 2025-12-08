//! Tool implementations for the Crow Agent
//!
//! Each tool implements `rig::tool::Tool` and can be used with the rig agent builder.
//!
//! ## Output Architecture
//!
//! Tools return structured data via the types in [`output`]. The presentation layer
//! decides how to render:
//! - CLI: markdown/terminal formatted text via `to_cli_string()`
//! - ACP: native UI components (diff view, file viewer, etc.) via `ToolOutput` trait
//! - Telemetry: JSON for storage/replay via `Serialize`

mod diagnostics;
mod edit_file;
mod fetch;
mod find_path;
mod grep;
mod list_directory;
mod now;
pub mod output;
mod read_file;
mod task_complete;
mod terminal;
mod thinking;
mod todo;
mod web_search;

pub use diagnostics::Diagnostics;
pub use edit_file::EditFile;
pub use fetch::Fetch;
pub use find_path::FindPath;
pub use grep::Grep;
pub use list_directory::ListDirectory;
pub use now::Now;
pub use output::{
    DirEntry, EditMode, EditOutput, FetchOutput, FileLocation, FindOutput, GrepMatch, GrepOutput,
    ListDirOutput, ReadOutput, SearchResult, TerminalOutput, ThinkOutput, TodoItem, TodoOutput,
    TodoStatus, ToolKind, ToolOutput, WebSearchOutput,
};
pub use read_file::ReadFile;
pub use task_complete::TaskComplete;
pub use terminal::Terminal;
pub use thinking::Thinking;
pub use todo::{TodoRead, TodoStore, TodoWrite};
pub use web_search::WebSearch;
