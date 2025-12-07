# Rig Integration

Crow Agent uses [rig](https://github.com/0xPlaygrounds/rig) as its AI framework. Rig provides:

- Agent abstraction with tools and system prompts
- Streaming completion support
- Multi-turn conversation handling
- OpenTelemetry instrumentation

## Agent Configuration

The rig agent is configured in `agent.rs`:

```rust
let agent = openrouter_client
    .agent(&self.config.model)
    .preamble(SYSTEM_PROMPT)
    .max_tokens(8096)
    .tool(ReadFileTool)
    .tool(EditFileTool)
    .tool(ListDirectoryTool)
    .tool(GrepTool)
    .tool(FindPathTool)
    .tool(TerminalTool)
    .tool(ThinkingTool)
    .tool(FetchTool)
    .tool(WebSearchTool)
    .tool(TodoWriteTool::new(todo_tx))
    .build();
```

## Streaming API

Rig's streaming API uses `StreamingPromptRequest`:

```rust
// Create streaming request
let request = agent
    .stream_prompt(message)
    .with_history(history)
    .with_hook(hook)
    .multi_turn(MAX_TOOL_TURNS);

// Await to get the stream
let stream = request.await;

// Process stream items
while let Some(item) = stream.next().await {
    match item {
        Ok(MultiTurnStreamItem::StreamAssistantItem(content)) => {
            // Handle text, tool calls, reasoning
        }
        Ok(MultiTurnStreamItem::StreamUserItem(content)) => {
            // Handle tool results
        }
        Ok(MultiTurnStreamItem::FinalResponse(resp)) => {
            // Conversation complete
        }
        Err(e) => {
            // Handle errors including cancellation
        }
    }
}
```

## Stream Item Types

### StreamAssistantItem

Contains `StreamedAssistantContent` variants:

- `Text` - Streamed text chunks
- `ToolCall` - Complete tool call (name, args)
- `ToolCallDelta` - Partial tool call updates
- `Reasoning` - Extended thinking content
- `Final` - Final message marker

### StreamUserItem

Contains `StreamedUserContent`:

- `ToolResult` - Result from tool execution

### FinalResponse

Indicates the conversation turn is complete:

```rust
pub struct FinalResponse {
    response: String,  // Accumulated response text
}
```

## History Management

**Important**: Rig expects the caller to manage chat history. The streaming API does not automatically update history. After a `FinalResponse`:

```rust
// Add user message to history
history.push(Message::User {
    content: OneOrMany::one(UserContent::text(&prompt_text)),
});

// Add assistant response to history
history.push(Message::Assistant {
    id: None,
    content: OneOrMany::one(AssistantContent::Text(Text {
        text: response_text,
    })),
});
```

## Hooks

Rig supports hooks for observing agent execution:

- `PromptHook` - For non-streaming requests
- `StreamingPromptHook` - For streaming requests

Both provide callbacks for:
- `on_completion_call` - Before LLM request
- `on_completion_response` - After LLM response (non-streaming only)
- `on_text_delta` - Text chunks (streaming only)
- `on_tool_call` - Before tool execution
- `on_tool_result` - After tool execution

Hooks receive a `CancelSignal` that can be used to cancel the operation.
