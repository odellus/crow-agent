# Rig Fork Modifications

Crow Agent uses a forked version of rig with several modifications to support our requirements. The fork is located at `crow_agent/rig/`.

## Why Fork?

We needed modifications that were either:
- Not yet upstream
- Specific to our OpenRouter + streaming + telemetry requirements
- Bug fixes for edge cases

## Modifications Made

### 1. OpenRouter Streaming Telemetry (`rig-core/src/providers/openrouter/streaming.rs`)

Added telemetry fields to the streaming span to match non-streaming telemetry:

```rust
// Serialize request for telemetry before converting to bytes
let tools_json = serde_json::to_string(&request.tools).ok();
let body_json = serde_json::to_string(&request).ok();

let span = info_span!(
    target: "rig::completions",
    "chat_streaming",
    gen_ai.operation.name = "chat_streaming",
    gen_ai.provider.name = "openrouter",
    gen_ai.request.model = self.model,
    gen_ai.system_instructions = preamble,
    gen_ai.request.tools = tracing::field::Empty,  // Added
    gen_ai.request.body = tracing::field::Empty,   // Added
    // ... other fields
);

// Record request tools and full body
if let Some(ref tools) = tools_json {
    span.record("gen_ai.request.tools", tools.as_str());
}
if let Some(ref body) = body_json {
    span.record("gen_ai.request.body", body.as_str());
}
```

### 2. Tools Field Visibility (`rig-core/src/providers/openrouter/completion.rs`)

Made the `tools` field accessible to the streaming module:

```rust
// Before: tools: Vec<ToolDefinition>,
// After:
pub(super) tools: Vec<ToolDefinition>,
```

### 3. Empty History Bug Fix (`rig-core/src/agent/prompt_request/streaming.rs`)

Fixed a panic when hooks are present and history is empty on the first iteration:

```rust
if let Some(ref hook) = self.hook {
    let reader = chat_history.read().await;
    // On first iteration, history may be empty - use current_prompt instead
    let (prompt_for_hook, history_for_hook) = if reader.is_empty() {
        (current_prompt.clone(), vec![])
    } else {
        let prompt = reader.last().cloned().expect("checked non-empty above");
        let history = reader[..reader.len() - 1].to_vec();
        (prompt, history)
    };
    drop(reader);
    hook.on_completion_call(&prompt_for_hook, &history_for_hook, cancel_signal.clone()).await;
}
```

**The bug**: When `with_hook()` was used and this was the first message (empty history), the code called `.last()` on an empty vector, causing a panic.

## File Summary

| File | Modification |
|------|--------------|
| `rig-core/src/providers/openrouter/streaming.rs` | Added telemetry span fields for tools and body |
| `rig-core/src/providers/openrouter/completion.rs` | Made `tools` field `pub(super)` |
| `rig-core/src/agent/prompt_request/streaming.rs` | Fixed empty history panic with hooks |

## Keeping Fork Updated

When updating the fork from upstream:

1. Check if our modifications are still needed
2. Resolve any merge conflicts in the modified files
3. Test streaming with telemetry enabled
4. Test multi-turn conversations with hooks
5. Verify cancellation still works

## Upstream Contributions

Consider contributing these fixes upstream:
- [ ] Empty history bug fix - clear bug, should be accepted
- [ ] Streaming telemetry fields - may need discussion on approach
