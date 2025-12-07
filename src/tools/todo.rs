//! Todo tools for agent planning and progress tracking.

use anyhow::Result;
use parking_lot::RwLock;
use rig::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TodoItem {
    /// Brief description of the task
    pub content: String,
    /// Current status of the task: pending, in_progress, completed, cancelled
    pub status: TodoStatus,
    /// Present continuous form shown during execution (e.g., "Implementing feature X")
    #[serde(rename = "activeForm")]
    pub active_form: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    /// Task not yet started
    Pending,
    /// Currently working on this task
    InProgress,
    /// Task finished successfully
    Completed,
    /// Task no longer needed
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

/// Shared storage for todo lists across sessions.
#[derive(Clone, Default)]
pub struct TodoStore {
    todos: Arc<RwLock<HashMap<String, Vec<TodoItem>>>>,
}

impl TodoStore {
    pub fn new() -> Self {
        Self {
            todos: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn get_todos(&self, session_id: &str) -> Vec<TodoItem> {
        self.todos
            .read()
            .get(session_id)
            .cloned()
            .unwrap_or_default()
    }

    pub fn set_todos(&self, session_id: &str, todos: Vec<TodoItem>) {
        self.todos.write().insert(session_id.to_string(), todos);
    }
}

// ============================================================================
// TodoWrite Tool
// ============================================================================

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct TodoWriteInput {
    /// The complete updated todo list. This replaces the entire existing list.
    pub todos: Vec<TodoItem>,
}

#[derive(Clone)]
pub struct TodoWrite {
    store: TodoStore,
    session_id: String,
}

impl TodoWrite {
    pub fn new(store: TodoStore, session_id: String) -> Self {
        Self { store, session_id }
    }
}

impl Tool for TodoWrite {
    const NAME: &'static str = "todo_write";

    type Error = std::convert::Infallible;
    type Args = TodoWriteInput;
    type Output = String;

    async fn definition(&self, _prompt: String) -> rig::completion::ToolDefinition {
        rig::completion::ToolDefinition {
            name: Self::NAME.to_string(),
            description: r#"Use this tool to create and manage a structured task list for your current coding session. This helps you track progress, organize complex tasks, and demonstrate thoroughness to the user.
It also helps the user understand the progress of the task and overall progress of their requests.

## When to Use This Tool
Use this tool proactively in these scenarios:

1. Complex multi-step tasks - When a task requires 3 or more distinct steps or actions
2. Non-trivial and complex tasks - Tasks that require careful planning or multiple operations
3. User explicitly requests todo list - When the user directly asks you to use the todo list
4. User provides multiple tasks - When users provide a list of things to be done (numbered or comma-separated)
5. After receiving new instructions - Immediately capture user requirements as todos. Feel free to edit the todo list based on new information.
6. After completing a task - Mark it complete and add any new follow-up tasks
7. When you start working on a new task, mark the todo as in_progress. Ideally you should only have one todo as in_progress at a time. Complete existing tasks before starting new ones.

## When NOT to Use This Tool

Skip using this tool when:
1. There is only a single, straightforward task
2. The task is trivial and tracking it provides no organizational benefit
3. The task can be completed in less than 3 trivial steps
4. The task is purely conversational or informational

NOTE that you should not use this tool if there is only one trivial task to do. In this case you are better off just doing the task directly.

## Examples of When to Use the Todo List

<example>
User: I want to add a dark mode toggle to the application settings. Make sure you run the tests and build when you're done!
Assistant: I'll help add a dark mode toggle to your application settings. Let me create a todo list to track this implementation.
*Creates todo list with the following items:*
1. Create dark mode toggle component in Settings page
2. Add dark mode state management (context/store)
3. Implement CSS-in-JS styles for dark theme
4. Update existing components to support theme switching
5. Run tests and build process, addressing any failures or errors that occur
*Begins working on the first task*

<reasoning>
The assistant used the todo list because:
1. Adding dark mode is a multi-step feature requiring UI, state management, and styling changes
2. The user explicitly requested tests and build be run afterward
3. The assistant inferred that tests and build need to pass by adding "Ensure tests and build succeed" as the final task
</reasoning>
</example>

<example>
User: Help me rename the function getCwd to getCurrentWorkingDirectory across my project
Assistant: Let me first search through your codebase to find all occurrences of 'getCwd'.
*Uses grep or search tools to locate all instances of getCwd in the codebase*
Assistant: I've found 15 instances of 'getCwd' across 8 different files. Let me create a todo list to track these changes.
*Creates todo list with specific items for each file that needs updating*

<reasoning>
The assistant used the todo list because:
1. First, the assistant searched to understand the scope of the task
2. Upon finding multiple occurrences across different files, it determined this was a complex task with multiple steps
3. The todo list helps ensure every instance is tracked and updated systematically
4. This approach prevents missing any occurrences and maintains code consistency
</reasoning>
</example>

<example>
User: I need to implement these features for my e-commerce site: user registration, product catalog, shopping cart, and checkout flow.
Assistant: I'll help implement these features. First, let's add all the features to the todo list.
*Creates a todo list breaking down each feature into specific tasks based on the project architecture*
Assistant: Let's start with user registration. This will involve creating database models, API endpoints, and frontend forms that integrate with your existing authentication system.

<reasoning>
The assistant used the todo list because:
1. The user provided multiple complex features to implement in a comma separated list
2. The todo list helps organize these large features into manageable tasks
3. This approach allows for tracking progress across the entire implementation
</reasoning>
</example>

<example>
User: Can you help optimize my React application? It's rendering slowly and has performance issues.
Assistant: I'll help optimize your React application. First, let me examine your codebase to identify potential performance bottlenecks.
*Reviews component structure, render patterns, state management, and data fetching*
Assistant: After analyzing your codebase, I've identified several performance issues. Let me create a todo list to track our optimization efforts.
*Creates todo list with items like: 1) Implement memoization for expensive calculations in ProductList, 2) Add virtualization for long lists in Dashboard, 3) Optimize image loading in Gallery component, 4) Fix state update loops in ShoppingCart, 5) Review bundle size and implement code splitting*
Let's start by implementing memoization for the expensive calculations in your ProductList component.

<reasoning>
The assistant used the todo list because:
1. First, the assistant examined the codebase to identify specific performance issues
2. Based on this analysis, it identified multiple optimization opportunities
3. Performance optimization is a non-trivial task requiring multiple steps
4. The todo list helps methodically track improvements across different components
5. This systematic approach ensures all performance bottlenecks are addressed
</reasoning>
</example>

## Examples of When NOT to Use the Todo List

<example>
User: How do I print 'Hello World' in Python?
Assistant: In Python, you can print "Hello World" with this simple code:

python
print("Hello World")

This will output the text "Hello World" to the console when executed.

<reasoning>
The assistant did not use the todo list because this is a single, trivial task that can be completed in one step. There's no need to track multiple tasks or steps for such a straightforward request.
</reasoning>
</example>

<example>
User: What does the git status command do?
Assistant: The git status command shows the current state of your working directory and staging area. It displays which changes have been staged, which haven't, and which files aren't being tracked by Git. This is a fundamental command for understanding the current state of your repository before making commits.

<reasoning>
The assistant did not use the todo list because this is an informational request with no actual coding task to complete. The user is simply asking for an explanation, not for the assistant to perform multiple steps or tasks.
</reasoning>
</example>

<example>
User: Can you add a comment to the calculateTotal function to explain what it does?
Assistant: Sure, let me add a comment to the calculateTotal function to explain what it does.
* Uses the Edit tool to add a comment to the calculateTotal function *

<reasoning>
The assistant did not use the todo list because this is a single, straightforward task confined to one location in the code. Adding a comment doesn't require tracking multiple steps or systematic organization.
</reasoning>
</example>

<example>
User: Run npm install for me and tell me what happens.
Assistant: I'll run the npm install command for you.

*Executes: npm install*

The command completed successfully. Here's the output:
[Output of npm install command]

All dependencies have been installed according to your package.json file.

<reasoning>
The assistant did not use the todo list because this is a single command execution with immediate results. There are no multiple steps to track or organize, making the todo list unnecessary for this straightforward task.
</reasoning>
</example>

## Task States and Management

1. **Task States**: Use these states to track progress:
   - pending: Task not yet started
   - in_progress: Currently working on (limit to ONE task at a time)
   - completed: Task finished successfully
   - cancelled: Task no longer needed

2. **Task Management**:
   - Update task status in real-time as you work
   - Mark tasks complete IMMEDIATELY after finishing (don't batch completions)
   - Only have ONE task in_progress at any time
   - Complete current tasks before starting new ones
   - Cancel tasks that become irrelevant

3. **Task Breakdown**:
   - Create specific, actionable items
   - Break complex tasks into smaller, manageable steps
   - Use clear, descriptive task names

When in doubt, use this tool. Being proactive with task management demonstrates attentiveness and ensures you complete all requirements successfully."#.to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "required": ["todos"],
                "properties": {
                    "todos": {
                        "type": "array",
                        "description": "The complete updated todo list. This replaces the entire existing list.",
                        "items": {
                            "type": "object",
                            "required": ["content", "status", "activeForm"],
                            "properties": {
                                "content": {
                                    "type": "string",
                                    "description": "Brief description of the task"
                                },
                                "status": {
                                    "description": "Current status of the task: pending, in_progress, completed, cancelled",
                                    "anyOf": [
                                        {"type": "string", "enum": ["pending"], "description": "Task not yet started"},
                                        {"type": "string", "enum": ["in_progress"], "description": "Currently working on this task"},
                                        {"type": "string", "enum": ["completed"], "description": "Task finished successfully"},
                                        {"type": "string", "enum": ["cancelled"], "description": "Task no longer needed"}
                                    ]
                                },
                                "activeForm": {
                                    "type": "string",
                                    "description": "Present continuous form shown during execution (e.g., \"Implementing feature X\")"
                                }
                            }
                        }
                    }
                }
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let count = args.todos.len();
        self.store.set_todos(&self.session_id, args.todos.clone());

        let markdown = format_todos_markdown(&args.todos);
        Ok(format!("{}\n\nUpdated {} todos.", markdown, count))
    }
}

// ============================================================================
// TodoRead Tool
// ============================================================================

/// Input for the todo_read tool.
///
/// Retrieves the current task list state to track pending or completed items.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct TodoReadInput {
    /// Action to perform (currently only "show" is supported)
    #[serde(default)]
    pub action: TodoReadAction,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum TodoReadAction {
    /// Show the current todo list
    #[default]
    Show,
}

#[derive(Clone)]
pub struct TodoRead {
    store: TodoStore,
    session_id: String,
}

impl TodoRead {
    pub fn new(store: TodoStore, session_id: String) -> Self {
        Self { store, session_id }
    }
}

impl Tool for TodoRead {
    const NAME: &'static str = "todo_read";

    type Error = std::convert::Infallible;
    type Args = TodoReadInput;
    type Output = String;

    async fn definition(&self, _prompt: String) -> rig::completion::ToolDefinition {
        rig::completion::ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Input for the todo_read tool.\n\nRetrieves the current task list state to track pending or completed items.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {
                        "description": "Action to perform (currently only \"show\" is supported)",
                        "default": "show",
                        "anyOf": [
                            {"type": "string", "enum": ["show"], "description": "Show the current todo list"}
                        ]
                    }
                }
            }),
        }
    }

    async fn call(&self, _args: Self::Args) -> Result<Self::Output, Self::Error> {
        let todos = self.store.get_todos(&self.session_id);

        let output = serde_json::json!({
            "count": todos.len(),
            "todos": todos,
        });

        Ok(serde_json::to_string(&output).unwrap_or_default())
    }
}

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
        store.set_todos(session, todos.clone());

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
    fn test_format_todos_markdown() {
        let todos = vec![
            TodoItem {
                content: "First task".to_string(),
                status: TodoStatus::Completed,
                active_form: "Completing first task".to_string(),
            },
            TodoItem {
                content: "Second task".to_string(),
                status: TodoStatus::InProgress,
                active_form: "Working on second task".to_string(),
            },
            TodoItem {
                content: "Third task".to_string(),
                status: TodoStatus::Pending,
                active_form: "Starting third task".to_string(),
            },
            TodoItem {
                content: "Fourth task".to_string(),
                status: TodoStatus::Cancelled,
                active_form: "Cancelling fourth task".to_string(),
            },
        ];

        let markdown = format_todos_markdown(&todos);
        assert!(markdown.contains("1. [x] First task"));
        assert!(markdown.contains("2. [~] Second task"));
        assert!(markdown.contains("3. [ ] Third task"));
        assert!(markdown.contains("4. [-] Fourth task"));
    }
}
