# Cancellation

Cancellation allows clients to interrupt an ongoing operation. This is critical for good UX - users should be able to stop a long-running request.

## How It Works

Crow Agent implements cancellation using two mechanisms:

### 1. Session Cancelled Flag

A `Cell<bool>` on the session that's checked in the stream loop:

```rust
struct Session {
    // ...
    cancelled: Cell<bool>,
}
```

This flag is:
- Reset to `false` at the start of each prompt
- Set to `true` when `session/cancel` is received
- Checked at the start of each stream loop iteration

### 2. Rig CancelSignal

Rig provides a `CancelSignal` that's passed to hooks. When cancelled:
- The signal's internal `AtomicBool` is set to `true`
- Rig checks `is_cancelled()` after hook calls
- Returns `PromptError::PromptCancelled` if cancelled

## Implementation

### Cancel Handler

```rust
async fn cancel(&self, args: CancelNotification) -> acp::Result<()> {
    let session_id = args.session_id.0.to_string();
    
    let sessions = self.sessions.borrow();
    if let Some(session) = sessions.get(&session_id) {
        // Set the cancelled flag - checked in stream loop
        session.cancelled.set(true);
        
        // Trigger hook's cancel signal for tool interruption
        let hook = session.active_hook.borrow();
        if let Some(ref h) = *hook {
            h.cancel().await;
        }
    }
    
    Ok(())
}
```

### Stream Loop Check

```rust
while let Some(item) = stream.next().await {
    // Check if cancelled at start of each iteration
    {
        let sessions = self.sessions.borrow();
        if let Some(session) = sessions.get(&session_id) {
            if session.cancelled.get() {
                info!("Session {} was cancelled", session_id);
                *session.active_hook.borrow_mut() = None;
                return Ok(PromptResponse::new(StopReason::Cancelled));
            }
        }
    }
    
    match item {
        // ... handle stream items
    }
}
```

### Hook Cancel Signal Storage

The `TelemetryHook` stores the cancel signal received from rig:

```rust
impl<M> StreamingPromptHook<M> for TelemetryHook {
    async fn on_completion_call(
        &self,
        prompt: &Message,
        history: &[Message],
        cancel_sig: CancelSignal,
    ) {
        // Store for later cancellation
        self.store_cancel_signal(&cancel_sig).await;
        // ...
    }
}

impl TelemetryHook {
    pub async fn cancel(&self) {
        if let Some(ref signal) = *self.cancel_signal.lock().await {
            signal.cancel();
        }
    }
}
```

## Cancellation Flow

```
1. Client sends: {"method": "session/cancel", "params": {"sessionId": "0"}}

2. CrowAcpAgent.cancel() called:
   - session.cancelled.set(true)
   - hook.cancel().await  // triggers CancelSignal

3. Stream loop detects cancellation:
   - Either: cancelled flag is true at loop start
   - Or: rig yields PromptError::PromptCancelled

4. Agent returns: {"result": {"stopReason": "cancelled"}}
```

## Why Two Mechanisms?

The dual approach ensures responsiveness:

| Scenario | Cancelled Flag | CancelSignal |
|----------|----------------|--------------|
| Streaming text | Checked every iteration | Not checked during stream |
| Tool execution | Checked between tools | Checked after hook calls |
| Waiting for LLM | Checked on next yield | Not applicable |

The cancelled flag provides immediate responsiveness in the stream loop, while the CancelSignal ensures tool execution can be interrupted through rig's hook system.

## Comparison with claude-code-acp

Our implementation mirrors claude-code-acp's approach:

```typescript
// claude-code-acp
async cancel(params: CancelNotification): Promise<void> {
    this.sessions[params.sessionId].cancelled = true;
    await this.sessions[params.sessionId].query.interrupt();
}

// In prompt loop:
if (this.sessions[params.sessionId].cancelled) {
    return { stopReason: "cancelled" };
}
```

Both use:
1. A session-level cancelled flag
2. An interrupt mechanism for the underlying query/stream
3. Checks in the processing loop

## Edge Cases

### Cancel Before Stream Starts

If cancel arrives before streaming begins, the cancelled flag will be set and checked on the first loop iteration.

### Cancel During Tool Execution

The CancelSignal will cause rig to return `ToolSetError::Interrupted`, which propagates as `PromptError::PromptCancelled`.

### Cancel After Completion

If cancel arrives after `FinalResponse`, it's effectively a no-op - the prompt has already completed.
