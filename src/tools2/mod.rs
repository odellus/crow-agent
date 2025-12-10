//! Tool implementations (new system - no rig dependency)
//!
//! Each tool implements our `Tool` trait from `crate::tool`.

mod bash;
mod edit;
mod fetch;
mod find_path;
mod grep;
mod list_directory;
mod read_file;
mod task;
mod task_complete;
mod todo;
mod web_search;

pub use bash::BashTool;
pub use edit::EditTool;
pub use fetch::FetchTool;
pub use find_path::FindPathTool;
pub use grep::GrepTool;
pub use list_directory::ListDirectoryTool;
pub use read_file::ReadFileTool;
pub use task::TaskTool;
pub use task_complete::TaskCompleteTool;
pub use todo::{TodoItem, TodoReadTool, TodoStatus, TodoStore, TodoWriteTool};
pub use web_search::WebSearchTool;

use crate::agent::AgentRegistry;
use crate::provider::ProviderClient;
use crate::tool::ToolRegistry;
use std::path::PathBuf;
use std::sync::Arc;

/// Create a registry with all standard tools (without session-specific tools like todo)
pub fn create_registry(working_dir: PathBuf) -> ToolRegistry {
    let mut registry = ToolRegistry::new();

    // File reading
    registry.register(ReadFileTool::new(working_dir.clone()));

    // File editing
    registry.register(EditTool::new(working_dir.clone()));

    // Shell execution
    registry.register(BashTool::new());

    // Search tools
    registry.register(GrepTool::new(working_dir.clone()));
    registry.register(FindPathTool::new(working_dir.clone()));
    registry.register(ListDirectoryTool::new(working_dir.clone()));

    // Web tools
    registry.register(FetchTool::new());
    registry.register(WebSearchTool::new());

    // Task completion (for subagents)
    registry.register(TaskCompleteTool::new());

    // Note: Todo tools require session_id and are added via create_registry_with_session()
    // Note: Task tool for subagent spawning requires additional dependencies

    registry
}

/// Create a registry with all tools including session-specific ones (todo)
pub fn create_registry_with_session(
    working_dir: PathBuf,
    session_id: String,
    todo_store: TodoStore,
) -> ToolRegistry {
    let mut registry = create_registry(working_dir);

    // Add session-specific todo tools
    registry.register(TodoWriteTool::new(todo_store.clone(), session_id.clone()));
    registry.register(TodoReadTool::new(todo_store, session_id));

    registry
}

/// Create a full registry including Task tool for subagent spawning
///
/// This requires:
/// - AgentRegistry for looking up subagent configs
/// - ProviderClient for LLM calls to child agents
/// - Session info for todo tools
pub fn create_full_registry(
    working_dir: PathBuf,
    session_id: String,
    todo_store: TodoStore,
    agent_registry: AgentRegistry,
    provider: Arc<ProviderClient>,
) -> ToolRegistry {
    // Create base registry without task tool
    let base_registry = create_registry(working_dir.clone());

    // Create the task tool with access to agent registry and provider
    // Note: Task tool gets a copy of base_registry (without task tool to prevent recursion)
    let task_tool = TaskTool::new(agent_registry, provider, base_registry);

    // Now build the full registry
    let mut registry = create_registry(working_dir);

    // Add session-specific todo tools
    registry.register(TodoWriteTool::new(todo_store.clone(), session_id.clone()));
    registry.register(TodoReadTool::new(todo_store, session_id));

    // Add task tool
    registry.register(task_tool);

    registry
}

/// Create a full registry with dynamic agent descriptions (async version)
///
/// Same as create_full_registry but fetches agent descriptions from registry
/// for a better Task tool description.
pub async fn create_full_registry_async(
    working_dir: PathBuf,
    session_id: String,
    todo_store: TodoStore,
    agent_registry: AgentRegistry,
    provider: Arc<ProviderClient>,
) -> ToolRegistry {
    // Create base registry without task tool
    let base_registry = create_registry(working_dir.clone());

    // Create the task tool with dynamic agent descriptions
    let task_tool =
        TaskTool::new_with_registry(agent_registry, provider, base_registry).await;

    // Now build the full registry
    let mut registry = create_registry(working_dir);

    // Add session-specific todo tools
    registry.register(TodoWriteTool::new(todo_store.clone(), session_id.clone()));
    registry.register(TodoReadTool::new(todo_store, session_id));

    // Add task tool
    registry.register(task_tool);

    registry
}
