# Tool Calls

ACP provides rich tool call notifications so clients can display tool execution status and results.

## Tool Call Lifecycle

```
1. ToolCall notification (status: InProgress)
2. Tool executes...
3. ToolCallUpdate notification (status: Completed/Failed)
```

## ACP Tool Call Structure

```rust
ToolCall {
    tool_call_id: String,
    title: String,
    kind: ToolKind,
    status: ToolCallStatus,
    raw_input: Option<serde_json::Value>,
}

ToolCallUpdate {
    tool_call_id: String,
    fields: ToolCallUpdateFields {
        status: Option<ToolCallStatus>,
        content: Option<Vec<ContentBlock>>,
    },
}
```

## Tool Kinds

| ToolKind | Tools | UI Treatment |
|----------|-------|--------------|
| `Read` | read_file, list_directory | File icon |
| `Edit` | edit_file | Edit icon |
| `Search` | grep, find_path | Search icon |
| `Execute` | terminal | Terminal icon |
| `Think` | thinking | Brain icon |
| `Fetch` | fetch, web_search | Globe icon |
| `Other` | (default) | Generic icon |

## Example Notification Sequence

### Reading a File

```json
// Tool call started
{
  "sessionId": "0",
  "update": {
    "sessionUpdate": "tool_call",
    "toolCallId": "call_abc123",
    "title": "Calling read_file",
    "kind": "read",
    "status": "in_progress",
    "rawInput": {"path": "/home/user/file.txt"}
  }
}

// Tool call completed
{
  "sessionId": "0",
  "update": {
    "sessionUpdate": "tool_call_update",
    "toolCallId": "call_abc123",
    "status": "completed",
    "content": [{"type": "text", "text": "file contents here..."}]
  }
}
```

### Running a Command

```json
// Tool call started
{
  "sessionId": "0",
  "update": {
    "sessionUpdate": "tool_call",
    "toolCallId": "call_def456",
    "title": "Calling terminal",
    "kind": "execute",
    "status": "in_progress",
    "rawInput": {"command": "ls -la"}
  }
}

// Tool call completed
{
  "sessionId": "0",
  "update": {
    "sessionUpdate": "tool_call_update",
    "toolCallId": "call_def456",
    "status": "completed",
    "content": [{"type": "text", "text": "total 42\ndrwxr-xr-x ..."}]
  }
}
```

## Plan Updates (TodoWrite)

The `todo_write` tool is special - it also sends Plan updates:

```rust
fn parse_todo_write_to_plan(args: &serde_json::Value) -> Option<Plan> {
    let todos = args.get("todos")?.as_array()?;
    
    let entries: Vec<PlanEntry> = todos
        .iter()
        .filter_map(|todo| {
            let content = todo.get("content")?.as_str()?.to_string();
            let status_str = todo.get("status")?.as_str()?;
            
            let status = match status_str {
                "pending" => PlanEntryStatus::Pending,
                "in_progress" => PlanEntryStatus::InProgress,
                "completed" => PlanEntryStatus::Completed,
                _ => PlanEntryStatus::Pending,
            };
            
            Some(PlanEntry::new(content, PlanEntryPriority::Medium, status))
        })
        .collect();
    
    Some(Plan::new(entries))
}
```

Example Plan notification:

```json
{
  "sessionId": "0",
  "update": {
    "sessionUpdate": "plan",
    "entries": [
      {"content": "Read the file", "priority": "medium", "status": "completed"},
      {"content": "Make changes", "priority": "medium", "status": "in_progress"},
      {"content": "Test changes", "priority": "medium", "status": "pending"}
    ]
  }
}
```

## Error Handling

If a tool fails, the update includes the error:

```json
{
  "sessionId": "0",
  "update": {
    "sessionUpdate": "tool_call_update",
    "toolCallId": "call_xyz789",
    "status": "failed",
    "content": [{"type": "text", "text": "Error: file not found"}]
  }
}
```
