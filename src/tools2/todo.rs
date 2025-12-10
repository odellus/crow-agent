//! Todo tools for agent planning and progress tracking.
//!
//! Ported from tools/todo.rs with shared session support for dual-agent mode.

use crate::tool::{Tool, ToolContext, ToolDefinition, ToolResult};
use async_trait::async_trait;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;

// ============================================================================
// Types
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItem {
    pub content: String,
    pub status: TodoStatus,
    #[serde(rename = "activeForm")]
    pub active_form: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
    Cancelled,
}

impl std::fmt::Display for TodoStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TodoStatus::Pending => write!(f, "pending"),
            TodoStatus::InProgress => write!(f, "in_progress"),
            TodoStatus::Completed => write!(f, "completed"),
            TodoStatus::Cancelled => write!(f, "cancelled"),
        }
    }
}

// ============================================================================
// TodoStore - Shared storage with sibling session support
// ============================================================================

/// Shared storage for todo lists across sessions.
/// Supports linking sibling sessions (executor/arbiter) to share the same todo state.
#[derive(Clone, Default)]
pub struct TodoStore {
    todos: Arc<RwLock<HashMap<String, Vec<TodoItem>>>>,
    /// Maps session_id -> shared_key for sibling sessions
    shared_keys: Arc<RwLock<HashMap<String, String>>>,
}

impl TodoStore {
    pub fn new() -> Self {
        Self {
            todos: Arc::new(RwLock::new(HashMap::new())),
            shared_keys: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Make two sessions share the same todo state (for dual-agent mode).
    /// Both session IDs will map to the same underlying storage key.
    pub fn share_sessions(&self, session_a: &str, session_b: &str) {
        let shared_key = session_a.to_string();
        let mut keys = self.shared_keys.write();
        keys.insert(session_a.to_string(), shared_key.clone());
        keys.insert(session_b.to_string(), shared_key);
    }

    /// Get the effective storage key for a session.
    fn get_todo_key(&self, session_id: &str) -> String {
        self.shared_keys
            .read()
            .get(session_id)
            .cloned()
            .unwrap_or_else(|| session_id.to_string())
    }

    pub fn get_todos(&self, session_id: &str) -> Vec<TodoItem> {
        let key = self.get_todo_key(session_id);
        self.todos.read().get(&key).cloned().unwrap_or_default()
    }

    pub fn set_todos(&self, session_id: &str, todos: Vec<TodoItem>) {
        let key = self.get_todo_key(session_id);
        self.todos.write().insert(key, todos);
    }
}

// ============================================================================
// TodoWrite Tool
// ============================================================================

#[derive(Debug, Deserialize)]
struct TodoWriteArgs {
    todos: Vec<TodoItem>,
}

pub struct TodoWriteTool {
    store: TodoStore,
    session_id: String,
}

impl TodoWriteTool {
    pub fn new(store: TodoStore, session_id: String) -> Self {
        Self { store, session_id }
    }
}

#[async_trait]
impl Tool for TodoWriteTool {
    fn name(&self) -> &str {
        "todo_write"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "todo_write".to_string(),
            description: r#"Manage a structured task list for tracking progress on complex tasks.

## When to Use
- Complex multi-step tasks (3+ steps)
- User provides multiple tasks
- After completing a task, mark it complete
- When starting a new task, mark it in_progress

## When NOT to Use
- Single straightforward task
- Trivial tasks (<3 steps)
- Purely informational requests

## Task States
- pending: Not yet started
- in_progress: Currently working (limit to ONE at a time)
- completed: Finished successfully
- cancelled: No longer needed

## Best Practices
- Update status in real-time
- Mark complete IMMEDIATELY after finishing
- Complete current tasks before starting new ones"#.to_string(),
            parameters: json!({
                "type": "object",
                "required": ["todos"],
                "properties": {
                    "todos": {
                        "type": "array",
                        "description": "The complete updated todo list (replaces existing)",
                        "items": {
                            "type": "object",
                            "required": ["content", "status", "activeForm"],
                            "properties": {
                                "content": {
                                    "type": "string",
                                    "description": "Brief description of the task"
                                },
                                "status": {
                                    "type": "string",
                                    "enum": ["pending", "in_progress", "completed", "cancelled"]
                                },
                                "activeForm": {
                                    "type": "string",
                                    "description": "Present continuous form (e.g., 'Implementing feature X')"
                                }
                            }
                        }
                    }
                }
            }),
        }
    }

    async fn execute(&self, args_value: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        if ctx.is_cancelled() {
            return ToolResult::error("Cancelled");
        }

        let args: TodoWriteArgs = match serde_json::from_value(args_value) {
            Ok(a) => a,
            Err(e) => return ToolResult::error(format!("Invalid arguments: {}", e)),
        };

        let count = args.todos.len();
        self.store.set_todos(&self.session_id, args.todos.clone());

        let markdown = format_todos_markdown(&args.todos);
        ToolResult::success(format!("{}\n\nUpdated {} todos.", markdown, count))
    }
}

// ============================================================================
// TodoRead Tool
// ============================================================================

#[derive(Debug, Deserialize, Default)]
#[allow(dead_code)]
struct TodoReadArgs {
    #[serde(default)]
    #[allow(dead_code)]
    action: Option<String>,
}

pub struct TodoReadTool {
    store: TodoStore,
    session_id: String,
}

impl TodoReadTool {
    pub fn new(store: TodoStore, session_id: String) -> Self {
        Self { store, session_id }
    }
}

#[async_trait]
impl Tool for TodoReadTool {
    fn name(&self) -> &str {
        "todo_read"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "todo_read".to_string(),
            description: "Retrieve the current task list state.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["show"],
                        "description": "Action to perform (default: show)"
                    }
                }
            }),
        }
    }

    async fn execute(&self, _args_value: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        if ctx.is_cancelled() {
            return ToolResult::error("Cancelled");
        }

        let todos = self.store.get_todos(&self.session_id);

        let output = json!({
            "count": todos.len(),
            "todos": todos,
        });

        ToolResult::success(serde_json::to_string_pretty(&output).unwrap_or_default())
    }
}

// ============================================================================
// Helpers
// ============================================================================

fn format_todos_markdown(todos: &[TodoItem]) -> String {
    if todos.is_empty() {
        return "No todos.".to_string();
    }

    let mut markdown = String::new();
    for (i, todo) in todos.iter().enumerate() {
        let checkbox = match todo.status {
            TodoStatus::Pending => "[ ]",
            TodoStatus::InProgress => "[~]",
            TodoStatus::Completed => "[x]",
            TodoStatus::Cancelled => "[-]",
        };
        markdown.push_str(&format!("{}. {} {}\n", i + 1, checkbox, todo.content));
    }
    markdown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_todo_store_basic() {
        let store = TodoStore::new();
        let session = "test-session";

        assert!(store.get_todos(session).is_empty());

        let todos = vec![TodoItem {
            content: "Task 1".to_string(),
            status: TodoStatus::Pending,
            active_form: "Working on task 1".to_string(),
        }];
        store.set_todos(session, todos);

        let retrieved = store.get_todos(session);
        assert_eq!(retrieved.len(), 1);
        assert_eq!(retrieved[0].content, "Task 1");
    }

    #[test]
    fn test_todo_store_session_isolation() {
        let store = TodoStore::new();

        store.set_todos(
            "session-a",
            vec![TodoItem {
                content: "Task A".to_string(),
                status: TodoStatus::Pending,
                active_form: "Working on A".to_string(),
            }],
        );

        store.set_todos(
            "session-b",
            vec![TodoItem {
                content: "Task B".to_string(),
                status: TodoStatus::Completed,
                active_form: "Working on B".to_string(),
            }],
        );

        let todos_a = store.get_todos("session-a");
        let todos_b = store.get_todos("session-b");

        assert_eq!(todos_a[0].content, "Task A");
        assert_eq!(todos_b[0].content, "Task B");
    }

    #[test]
    fn test_todo_store_shared_sessions() {
        let store = TodoStore::new();

        // Link executor and arbiter sessions
        store.share_sessions("executor-123", "arbiter-123");

        // Write from executor
        store.set_todos(
            "executor-123",
            vec![TodoItem {
                content: "Shared task".to_string(),
                status: TodoStatus::InProgress,
                active_form: "Working on shared task".to_string(),
            }],
        );

        // Read from arbiter should see the same todos
        let arbiter_todos = store.get_todos("arbiter-123");
        assert_eq!(arbiter_todos.len(), 1);
        assert_eq!(arbiter_todos[0].content, "Shared task");

        // Update from arbiter
        store.set_todos(
            "arbiter-123",
            vec![TodoItem {
                content: "Shared task".to_string(),
                status: TodoStatus::Completed,
                active_form: "Completed shared task".to_string(),
            }],
        );

        // Executor should see the update
        let executor_todos = store.get_todos("executor-123");
        assert_eq!(executor_todos[0].status, TodoStatus::Completed);
    }

    #[test]
    fn test_format_todos_markdown() {
        let todos = vec![
            TodoItem {
                content: "First task".to_string(),
                status: TodoStatus::Completed,
                active_form: "Completing".to_string(),
            },
            TodoItem {
                content: "Second task".to_string(),
                status: TodoStatus::InProgress,
                active_form: "Working".to_string(),
            },
            TodoItem {
                content: "Third task".to_string(),
                status: TodoStatus::Pending,
                active_form: "Starting".to_string(),
            },
            TodoItem {
                content: "Fourth task".to_string(),
                status: TodoStatus::Cancelled,
                active_form: "Cancelling".to_string(),
            },
        ];

        let markdown = format_todos_markdown(&todos);
        assert!(markdown.contains("1. [x] First task"));
        assert!(markdown.contains("2. [~] Second task"));
        assert!(markdown.contains("3. [ ] Third task"));
        assert!(markdown.contains("4. [-] Fourth task"));
    }
}
