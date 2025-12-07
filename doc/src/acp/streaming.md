# ACP Streaming

The ACP server converts rig's stream items into ACP session notifications for real-time updates to the client.

## Stream Item Conversion

### Text Content

```rust
StreamedAssistantContent::Text(text) => {
    // Accumulate for history
    accumulated_response.push_str(&text.text);
    
    // Send to client immediately
    self.send_update(SessionNotification::new(
        session_id.clone(),
        SessionUpdate::AgentMessageChunk(ContentChunk::new(
            ContentBlock::Text(TextContent::new(text.text)),
        )),
    )).await?;
}
```

### Tool Calls

```rust
StreamedAssistantContent::ToolCall(tool_call) => {
    let tool_call_id = ToolCallId::from(tool_call.id.clone());
    let kind = tool_name_to_kind(&tool_call.function.name);
    let title = format!("Calling {}", tool_call.function.name);
    
    self.send_update(SessionNotification::new(
        session_id.clone(),
        SessionUpdate::ToolCall(
            ToolCall::new(tool_call_id, title)
                .kind(kind)
                .status(ToolCallStatus::InProgress)
                .raw_input(tool_call.function.arguments.clone()),
        ),
    )).await?;
}
```

### Tool Results

```rust
StreamedUserContent::ToolResult(result) => {
    let tool_call_id = ToolCallId::from(result.id.clone());
    let result_text = /* extract text from result */;
    
    self.send_update(SessionNotification::new(
        session_id.clone(),
        SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
            tool_call_id,
            ToolCallUpdateFields::new()
                .status(ToolCallStatus::Completed)
                .content(vec![result_text.into()]),
        )),
    )).await?;
}
```

### Reasoning/Thinking

```rust
StreamedAssistantContent::Reasoning(reasoning) => {
    let text = reasoning.reasoning.join("");
    self.send_update(SessionNotification::new(
        session_id.clone(),
        SessionUpdate::AgentThoughtChunk(ContentChunk::new(
            ContentBlock::Text(TextContent::new(text)),
        )),
    )).await?;
}
```

## Tool Kind Mapping

Tools are categorized for UI treatment:

```rust
fn tool_name_to_kind(name: &str) -> ToolKind {
    match name {
        "read_file" => ToolKind::Read,
        "edit_file" => ToolKind::Edit,
        "list_directory" => ToolKind::Read,
        "grep" | "find_path" => ToolKind::Search,
        "terminal" => ToolKind::Execute,
        "thinking" => ToolKind::Think,
        "fetch" | "web_search" => ToolKind::Fetch,
        _ => ToolKind::Other,
    }
}
```

## Todo/Plan Updates

The `todo_write` tool is special-cased to send Plan updates:

```rust
if tool_call.function.name == "todo_write" {
    if let Some(plan) = parse_todo_write_to_plan(&tool_call.function.arguments) {
        self.send_update(SessionNotification::new(
            session_id.clone(),
            SessionUpdate::Plan(plan),
        )).await?;
    }
}
```

## Notification Delivery

Notifications are sent through a channel and delivered asynchronously:

```rust
async fn send_update(&self, notification: SessionNotification) -> acp::Result<()> {
    let (tx, rx) = oneshot::channel();
    self.session_update_tx
        .send((notification, tx))
        .map_err(|_| acp::Error::internal_error())?;
    rx.await.map_err(|_| acp::Error::internal_error())?;
    Ok(())
}
```

The background task sends notifications to the client:

```rust
tokio::task::spawn_local(async move {
    while let Some((notification, tx)) = rx.recv().await {
        if let Err(e) = conn.session_notification(notification).await {
            error!("Failed to send session notification: {}", e);
            break;
        }
        tx.send(()).ok();
    }
});
```

## Response Accumulation

Text is accumulated during streaming for history:

```rust
let mut accumulated_response = String::new();

// During streaming:
accumulated_response.push_str(&text.text);

// On FinalResponse:
let response_text = if !final_resp.response().is_empty() {
    final_resp.response().to_string()
} else {
    accumulated_response.clone()  // Fallback to accumulated
};
```

This ensures we have the complete response even if `FinalResponse` doesn't include it.
