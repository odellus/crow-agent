# Session Management

ACP sessions maintain state across multiple prompts, including chat history and cancellation state.

## Session Structure

```rust
struct Session {
    agent: CrowAgent,
    history: RefCell<Vec<rig::message::Message>>,
    active_hook: RefCell<Option<TelemetryHook>>,
    cancelled: Cell<bool>,
}
```

### Fields

- **agent**: The CrowAgent instance for this session
- **history**: Chat history (user messages + assistant responses)
- **active_hook**: Current telemetry hook (for cancellation)
- **cancelled**: Flag checked in stream loop

## Session Lifecycle

### 1. Creation (`session/new`)

```rust
async fn new_session(&self, args: NewSessionRequest) -> acp::Result<NewSessionResponse> {
    let session_id = self.next_session_id.get();
    self.next_session_id.set(session_id + 1);
    
    let session_config = Config {
        working_dir: args.cwd.clone(),
        ..self.config.clone()
    };
    
    let agent = CrowAgent::new(session_config, self.telemetry.clone());
    
    self.sessions.borrow_mut().insert(
        session_id.to_string(),
        Session {
            agent,
            history: RefCell::new(Vec::new()),
            active_hook: RefCell::new(None),
            cancelled: Cell::new(false),
        },
    );
    
    Ok(NewSessionResponse::new(SessionId::new(session_id.to_string())))
}
```

### 2. Prompt Execution (`session/prompt`)

Each prompt:
1. Resets `cancelled` flag
2. Gets history from session
3. Creates streaming request with hook
4. Stores hook in `active_hook`
5. Processes stream, sending notifications
6. Updates history on completion
7. Clears `active_hook`

### 3. Cancellation (`session/cancel`)

Sets `cancelled` flag and triggers hook cancel (see [Cancellation](./cancellation.md)).

## History Management

History is updated only on successful completion (`FinalResponse`):

```rust
Ok(MultiTurnStreamItem::FinalResponse(final_resp)) => {
    let response_text = if !final_resp.response().is_empty() {
        final_resp.response().to_string()
    } else {
        accumulated_response.clone()
    };
    
    let sessions = self.sessions.borrow();
    if let Some(session) = sessions.get(&session_id) {
        let mut history = session.history.borrow_mut();
        
        // Add user message
        history.push(Message::User {
            content: OneOrMany::one(UserContent::text(&prompt_text)),
        });
        
        // Add assistant response
        history.push(Message::Assistant {
            id: None,
            content: OneOrMany::one(AssistantContent::Text(Text {
                text: response_text,
            })),
        });
    }
}
```

**Important**: History is NOT updated on cancellation or error. This ensures the conversation remains consistent.

## Multi-turn Conversations

With proper history management, the agent maintains context across prompts:

```
Prompt 1: "My name is Alice"
Response 1: "Nice to meet you, Alice!"

Prompt 2: "What's my name?"
Response 2: "Your name is Alice."  // Remembers from history
```

## Session Isolation

Each session has its own:
- CrowAgent instance
- Chat history
- Working directory
- Cancellation state

Sessions do not share state.
