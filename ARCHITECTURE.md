# Crow Agent Unified Architecture

## Overview

Two-layer architecture separating the ReAct loop from control flow orchestration.

```
┌─────────────────────────────────────────────────────────────┐
│                         OUTPUT ADAPTERS                      │
│      CLI Renderer    │    ACP Server    │    Future...      │
│                                                              │
│              All consume AgentEventStream                    │
└──────────────────────────────┬──────────────────────────────┘
                               │
┌──────────────────────────────┼──────────────────────────────┐
│                    CONTROL FLOW LAYER                        │
│                      (ACPAgent.run)                          │
│                                                              │
│   - Orchestrates TURNS between agents                        │
│   - Applies autonomy levels (ControlFlow enum)               │
│   - Hands off between primary ↔ coagent                      │
│   - This is the "external" boundary                          │
│                                                              │
│   loop {                                                     │
│       primary.execute_turn(session)                          │
│       match control_flow { ... }                             │
│   }                                                          │
│                                                              │
└──────────────────────────────┬──────────────────────────────┘
                               │
┌──────────────────────────────┼──────────────────────────────┐
│                    BASE AGENT LAYER                          │
│                  (BaseAgent.execute_turn)                    │
│                                                              │
│   - ReAct loop (LLM call → tool execution → repeat)         │
│   - Runs until: text response, task_complete, or cancelled  │
│   - Emits AgentEvents as it executes                         │
│   - Knows NOTHING about coagents or control flow             │
│                                                              │
└─────────────────────────────────────────────────────────────┘
                               │
              ┌────────────────┼────────────────┐
              ▼                ▼                ▼
       ┌───────────┐    ┌───────────┐    ┌───────────┐
       │ Provider  │    │   Tool    │    │  Session  │
       │  Client   │    │ Registry  │    │   Store   │
       └───────────┘    └───────────┘    └───────────┘
```

---

## 1. BaseAgent

The internal agent. Owns the ReAct loop. Dumb about orchestration.

```rust
pub struct BaseAgent {
    pub name: String,
    pub config: AgentConfig,
    provider: Arc<ProviderClient>,
    tools: Arc<ToolRegistry>,
    prompts: Arc<PromptRegistry>,
}

pub struct TurnResult {
    /// Text output (if any)
    pub text: Option<String>,
    /// Tool calls that were executed
    pub tool_calls: Vec<ExecutedToolCall>,
    /// Set if task_complete was called - ReAct loop ended early
    pub task_complete: Option<String>,
    /// Token usage for this turn
    pub usage: TokenUsage,
    /// Files modified during this turn
    pub files_changed: Vec<PathBuf>,
}

pub struct ExecutedToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
    pub result: ToolResult,
    pub duration_ms: u64,
}

impl BaseAgent {
    /// Execute a full turn (ReAct loop until done)
    /// 
    /// Runs until:
    /// - LLM responds with text only (no tool calls)
    /// - task_complete tool is called
    /// - Cancelled via token
    /// - Max iterations reached
    pub async fn execute_turn(
        &self,
        session: &mut InternalSession,
        event_tx: &mpsc::UnboundedSender<AgentEvent>,
        cancellation: CancellationToken,
    ) -> Result<TurnResult, AgentError>;
}
```

### What execute_turn does:

1. Build LLM context from session history
2. **Loop:**
   - Call LLM (streaming)
   - Emit text/thinking deltas as events
   - If no tool calls → break, return text
   - Execute each tool call:
     - Emit ToolCallStart
     - Run tool
     - Emit ToolCallEnd
     - If tool is `task_complete` → break, return with task_complete set
   - Add to session history
   - Continue loop
3. Return TurnResult

---

## 2. ACPAgent (Control Flow Layer)

The external agent. Orchestrates turns. Knows about coagents and autonomy.

```rust
pub struct ACPAgent {
    pub name: String,
    pub description: Option<String>,
    
    /// The primary agent that does the work
    primary: BaseAgent,
    
    /// Optional coagent for supervision/verification
    coagent: Option<BaseAgent>,
    
    /// How/when coagent intercedes
    control_flow: ControlFlow,
}

impl ACPAgent {
    /// Run the agent until completion or HITL break
    pub async fn run(
        &self,
        session: &mut ACPSession,
        event_tx: &mpsc::UnboundedSender<AgentEvent>,
        cancellation: CancellationToken,
    ) -> Result<RunResult, AgentError> {
        loop {
            // Primary agent takes a turn
            let result = self.primary.execute_turn(
                &mut session.primary_session,
                event_tx,
                cancellation.clone(),
            ).await?;
            
            // task_complete inside ReAct loop already broke out
            if result.task_complete.is_some() {
                return Ok(RunResult::Complete(result));
            }
            
            // Apply control flow
            match &self.control_flow {
                ControlFlow::HITL => {
                    return Ok(RunResult::NeedsInput(result));
                }
                
                ControlFlow::Loop => {
                    // Continue to next turn
                }
                
                ControlFlow::Static { message } => {
                    session.primary_session.add_user_message(message);
                }
                
                ControlFlow::Generated { generator_prompt } => {
                    let msg = self.generate_message(generator_prompt, session).await?;
                    session.primary_session.add_user_message(&msg);
                }
                
                ControlFlow::Coagent { tools, can_terminate } => {
                    event_tx.send(AgentEvent::CoagentStart { ... })?;
                    
                    let coagent = self.coagent.as_ref().unwrap();
                    let verdict = coagent.execute_turn(
                        &mut session.coagent_session,
                        event_tx,
                        cancellation.clone(),
                    ).await?;
                    
                    event_tx.send(AgentEvent::CoagentEnd { ... })?;
                    
                    // Coagent's output feeds back to primary
                    if let Some(text) = &verdict.text {
                        session.primary_session.add_user_message(text);
                    }
                }
            }
        }
    }
}

pub enum RunResult {
    /// Task completed (task_complete was called)
    Complete(TurnResult),
    /// HITL mode - needs user input to continue
    NeedsInput(TurnResult),
    /// Cancelled
    Cancelled,
}
```

---

## 3. ControlFlow

Your autonomy levels as a first-class type.

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ControlFlow {
    /// Level -1: Return to user after each turn
    HITL,
    
    /// Level 0: Loop until task_complete
    Loop,
    
    /// Level 1: Inject static message after each turn
    Static { message: String },
    
    /// Level 2: Generate contextual message (acceptance criteria)
    Generated { generator_prompt: String },
    
    /// Levels 3-5: Coagent intercedes
    Coagent {
        tools: CoagentTools,
        can_terminate: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CoagentTools {
    None,       // Level 3: Chat only
    ReadOnly,   // Level 4: Read, grep, list, etc.
    Full,       // Level 5: Same tools as primary
}

impl ControlFlow {
    pub fn from_level(level: i8) -> Self {
        match level {
            -1 => ControlFlow::HITL,
            0 => ControlFlow::Loop,
            1 => ControlFlow::Static { 
                message: "Continue with the task. Call task_complete when done.".into() 
            },
            2 => ControlFlow::Generated {
                generator_prompt: "Based on the original task, generate acceptance criteria...".into()
            },
            3 => ControlFlow::Coagent { tools: CoagentTools::None, can_terminate: false },
            4 => ControlFlow::Coagent { tools: CoagentTools::ReadOnly, can_terminate: true },
            5 => ControlFlow::Coagent { tools: CoagentTools::Full, can_terminate: true },
            _ => ControlFlow::Loop,
        }
    }
}
```

---

## 4. Sessions

Two levels matching the two agent layers.

```rust
/// Internal session - one per BaseAgent instance
pub struct InternalSession {
    pub id: String,
    pub agent_name: String,
    pub history: Vec<HistoryEntry>,
    pub created_at: DateTime<Utc>,
}

/// ACP session - wraps internal sessions for the ACPAgent
pub struct ACPSession {
    pub id: String,
    pub acp_agent_name: String,
    
    /// Primary agent's session
    pub primary_session: InternalSession,
    
    /// Coagent's session (if coagent exists)
    pub coagent_session: Option<InternalSession>,
    
    /// Snapshot tracking for undo
    pub snapshot_manager: SnapshotManager,
    pub initial_snapshot: Option<String>,
}
```

Key point: From outside (CLI/ACP protocol), there's ONE session ID. Internally, primary and coagent have separate histories but their events all stream to the same output.

---

## 5. Event Streaming

```rust
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    // Text
    TextDelta { agent: String, delta: String },
    TextComplete { agent: String, text: String },
    
    // Thinking/Reasoning
    ThinkingDelta { agent: String, delta: String },
    ThinkingComplete { agent: String, text: String },
    
    // Tools
    ToolCallStart { agent: String, call_id: String, tool: String, arguments: Value },
    ToolCallEnd { agent: String, call_id: String, tool: String, result: ToolResult, duration_ms: u64 },
    
    // File changes
    FilesChanged { agent: String, files: Vec<PathBuf>, snapshot_hash: String },
    
    // Control flow (emitted by ACPAgent, not BaseAgent)
    CoagentStart { primary: String, coagent: String },
    CoagentEnd { primary: String, coagent: String },
    SubagentStart { parent: String, subagent: String, task: String },
    SubagentEnd { parent: String, subagent: String, result: String },
    
    // Lifecycle
    TurnStart { agent: String },
    TurnComplete { agent: String },
    TaskComplete { agent: String, summary: String },
    
    // Errors
    Error { agent: String, error: String },
    Cancelled { agent: String },
    
    // Telemetry
    Usage { agent: String, input_tokens: u64, output_tokens: u64, reasoning_tokens: Option<u64> },
}

pub type AgentEventStream = Pin<Box<dyn Stream<Item = AgentEvent> + Send>>;
```

---

## 6. Registries

### AgentRegistry

```rust
pub struct AgentRegistry {
    agents: Arc<RwLock<HashMap<String, AgentConfig>>>,
}

impl AgentRegistry {
    pub fn new() -> Self;
    pub fn load_from_dirs(global: &Path, project: &Path) -> Self;
    pub async fn get(&self, name: &str) -> Option<AgentConfig>;
    pub async fn list_primary(&self) -> Vec<AgentConfig>;
    pub async fn list_subagents(&self) -> Vec<AgentConfig>;
}
```

### ToolRegistry

```rust
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new(working_dir: PathBuf) -> Self;
    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>>;
    pub fn for_agent(&self, config: &AgentConfig) -> Vec<Arc<dyn Tool>>;
    pub fn to_openai_tools(&self, config: &AgentConfig) -> Vec<ChatCompletionTool>;
}
```

### PromptRegistry

```rust
pub struct PromptRegistry {
    templates: Handlebars<'static>,
}

impl PromptRegistry {
    pub fn new() -> Self;
    pub fn load_from_dirs(global: &Path, project: &Path) -> Self;
    pub fn render(&self, name: &str, ctx: &PromptContext) -> Result<String>;
    pub fn for_agent(&self, config: &AgentConfig, ctx: &PromptContext) -> String;
}
```

---

## 7. Output Adapters

Both consume the same `AgentEventStream`.

### CLI

```rust
pub struct CliRenderer { ... }

impl CliRenderer {
    pub async fn render(&mut self, stream: AgentEventStream) -> Result<()>;
}
```

### ACP

```rust
pub struct AcpAdapter { ... }

impl AcpAdapter {
    pub async fn handle(&self, session_id: &str, stream: AgentEventStream) -> Result<()>;
}
```

---

## 8. Directory Structure

```
crow_agent/
├── src/
│   ├── lib.rs
│   ├── main.rs
│   │
│   ├── agent/
│   │   ├── mod.rs
│   │   ├── base.rs          # BaseAgent + execute_turn
│   │   ├── acp.rs           # ACPAgent + control flow
│   │   ├── config.rs        # AgentConfig
│   │   └── control_flow.rs  # ControlFlow enum
│   │
│   ├── registry/
│   │   ├── mod.rs
│   │   ├── agent.rs
│   │   ├── tool.rs
│   │   └── prompt.rs
│   │
│   ├── provider/
│   │   ├── mod.rs
│   │   ├── client.rs
│   │   └── streaming.rs
│   │
│   ├── tools/
│   │   └── ...
│   │
│   ├── session/
│   │   ├── mod.rs
│   │   ├── internal.rs      # InternalSession
│   │   ├── acp.rs           # ACPSession
│   │   └── store.rs
│   │
│   ├── events.rs            # AgentEvent enum
│   │
│   ├── adapters/
│   │   ├── mod.rs
│   │   ├── cli.rs
│   │   └── acp.rs
│   │
│   ├── telemetry/
│   │   └── ...
│   │
│   └── snapshot/
│       └── ...
│
├── prompts/
│   └── *.hbs
│
└── agents/
    └── *.md
```

---

## Summary

| Layer | Struct | Responsibility |
|-------|--------|----------------|
| **Adapter** | CliRenderer, AcpAdapter | Consume events, render to output |
| **Control Flow** | ACPAgent | Orchestrate turns, apply autonomy, handoff to coagent |
| **ReAct** | BaseAgent | LLM calls, tool execution, task_complete detection |
| **Infrastructure** | Registries, Provider, Session, Tools | Shared resources |
