//! Tool implementations (new system - no rig dependency)
//!
//! Each tool implements our `Tool` trait from `crate::tool`.

mod read_file;
mod task_complete;

pub use read_file::ReadFileTool;
pub use task_complete::TaskCompleteTool;

use crate::tool::ToolRegistry;
use std::path::PathBuf;

/// Create a registry with all standard tools
pub fn create_registry(working_dir: PathBuf) -> ToolRegistry {
    let mut registry = ToolRegistry::new();

    registry.register(ReadFileTool::new(working_dir.clone()));
    registry.register(TaskCompleteTool::new());

    // TODO: Port remaining tools:
    // - edit_file
    // - terminal
    // - grep
    // - find_path
    // - list_directory
    // - thinking
    // - now
    // - todo_read / todo_write
    // - fetch
    // - web_search
    // - diagnostics

    registry
}
