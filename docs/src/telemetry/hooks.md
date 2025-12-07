# Telemetry Hooks

The `TelemetryHook` struct implements rig's hook traits to capture events during agent execution.

## Implementation

```rust
#[derive(Clone)]
pub struct TelemetryHook {
    telemetry: Arc<Telemetry>,
    tool_start: Arc<Mutex<Option<Instant>>>,
    current_tool: Arc<Mutex<Option<String>>>,
    current_args: Arc<Mutex<Option<String>>>,
    cancel_signal: Arc<Mutex<Option<CancelSignal>>>,
}
```

## Trait Implementations

### PromptHook (Non-Streaming)

```rust
impl<M> PromptHook<M> for TelemetryHook
where
    M: CompletionModel,
{
    async fn on_completion_call(&self, prompt: &Message, history: &[Message], cancel_sig: CancelSignal) {
        self.store_cancel_signal(&cancel_sig).await;
        tracing::debug!(prompt_len = ..., history_len = ..., "Completion call starting");
    }

    async fn on_completion_response(&self, prompt: &Message, response: &CompletionResponse<M::Response>, cancel_sig: CancelSignal) {
        tracing::debug!(has_tool_calls = ..., "Completion response received");
    }

    async fn on_tool_call(&self, tool_name: &str, args: &str, cancel_sig: CancelSignal) {
        *self.tool_start.lock().await = Some(Instant::now());
        *self.current_tool.lock().await = Some(tool_name.to_string());
        tracing::info!(tool = tool_name, "Tool call starting");
    }

    async fn on_tool_result(&self, tool_name: &str, args: &str, result: &str, cancel_sig: CancelSignal) {
        let duration_ms = self.tool_start.lock().await.take()
            .map(|start| start.elapsed().as_millis() as u64)
            .unwrap_or(0);
        
        self.telemetry.log_tool_call(tool_name, &args_json, Ok(result), duration_ms).await;
        tracing::info!(tool = tool_name, duration_ms, "Tool call completed");
    }
}
```

### StreamingPromptHook (Streaming)

```rust
impl<M> StreamingPromptHook<M> for TelemetryHook
where
    M: CompletionModel,
{
    async fn on_completion_call(&self, prompt: &Message, history: &[Message], cancel_sig: CancelSignal) {
        self.store_cancel_signal(&cancel_sig).await;
        tracing::debug!("Streaming completion call starting");
    }

    async fn on_text_delta(&self, text_delta: &str, aggregated_text: &str, cancel_sig: CancelSignal) {
        // Not logged - too verbose
    }

    async fn on_tool_call_delta(&self, tool_call_id: &str, tool_call_delta: &str, cancel_sig: CancelSignal) {
        // Tool deltas accumulated by rig
    }

    async fn on_tool_call(&self, tool_name: &str, args: &str, cancel_sig: CancelSignal) {
        *self.tool_start.lock().await = Some(Instant::now());
        tracing::info!(tool = tool_name, "Streaming tool call starting");
    }

    async fn on_tool_result(&self, tool_name: &str, args: &str, result: &str, cancel_sig: CancelSignal) {
        let duration_ms = /* calculate */;
        self.telemetry.log_tool_call(tool_name, &args_json, Ok(result), duration_ms).await;
        tracing::info!(tool = tool_name, duration_ms, "Streaming tool call completed");
    }
}
```

## Cancellation Support

The hook stores the `CancelSignal` for external cancellation:

```rust
impl TelemetryHook {
    pub async fn cancel(&self) {
        if let Some(ref signal) = *self.cancel_signal.lock().await {
            tracing::info!("Cancelling current operation via hook");
            signal.cancel();
        }
    }

    async fn store_cancel_signal(&self, signal: &CancelSignal) {
        *self.cancel_signal.lock().await = Some(signal.clone());
    }

    pub async fn clear_cancel_signal(&self) {
        *self.cancel_signal.lock().await = None;
    }
}
```

## Attaching Hooks

### Non-Streaming

```rust
let hook = TelemetryHook::new(self.telemetry.clone());
let request = agent
    .prompt(message)
    .with_history(&mut history)
    .with_hook(hook)
    .multi_turn(MAX_TOOL_TURNS);
```

### Streaming

```rust
let hook = TelemetryHook::new(self.telemetry.clone());
let hook_clone = hook.clone();  // Clone for return
let request = agent
    .stream_prompt(message)
    .with_history(history)
    .with_hook(hook)
    .multi_turn(MAX_TOOL_TURNS);

// Return both stream and hook (for cancellation)
Ok((request, hook_clone))
```

## Event Flow

```
1. on_completion_call
   - Store cancel signal
   - Log prompt info

2. (rig sends request to LLM)

3. on_completion_response (non-streaming only)
   - Log response info

4. For each tool call:
   a. on_tool_call
      - Start timer
      - Log tool name/args
   
   b. (tool executes)
   
   c. on_tool_result
      - Calculate duration
      - Log to telemetry
      - Log result

5. (loop back to step 1 if more tool calls needed)
```
