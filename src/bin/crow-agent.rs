//! Crow Agent CLI (new agent system)
//!
//! Run with: cargo run --bin crow

use anyhow::Result;
use clap::{Parser, Subcommand};
use crow_agent::{
    agent::{build_system_prompt, Agent, AgentRegistry, RunResult},
    config::Config,
    events::AgentEvent,
    provider::{ProviderClient, ProviderConfig},
    run_stdio_server,
    tools2, Telemetry,
};
use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Global cancellation token for signal handling
static SHUTDOWN: std::sync::OnceLock<CancellationToken> = std::sync::OnceLock::new();

/// Global interaction guard - set during run_agent, flushed on signal
static INTERACTION_GUARD: std::sync::OnceLock<std::sync::Mutex<Option<Arc<tokio::sync::Mutex<crow_agent::InteractionGuard>>>>> = std::sync::OnceLock::new();

fn get_shutdown_token() -> CancellationToken {
    SHUTDOWN.get_or_init(CancellationToken::new).clone()
}

fn set_global_guard(guard: Arc<tokio::sync::Mutex<crow_agent::InteractionGuard>>) {
    let slot = INTERACTION_GUARD.get_or_init(|| std::sync::Mutex::new(None));
    if let Ok(mut g) = slot.lock() {
        *g = Some(guard);
    }
}

fn clear_global_guard() {
    if let Some(slot) = INTERACTION_GUARD.get() {
        if let Ok(mut g) = slot.lock() {
            *g = None;
        }
    }
}

fn flush_global_guard() {
    if let Some(slot) = INTERACTION_GUARD.get() {
        if let Ok(g) = slot.lock() {
            if let Some(ref guard) = *g {
                // Try to flush - use blocking lock since we're in signal context
                if let Ok(inner) = guard.try_lock() {
                    inner.flush();
                }
            }
        }
    }
}

/// Get the default data directory
fn default_data_dir() -> PathBuf {
    dirs::data_dir()
        .map(|d| d.join("crow"))
        .unwrap_or_else(|| PathBuf::from(".crow"))
}

#[derive(Parser)]
#[command(name = "crow")]
#[command(about = "Crow Agent - A standalone LLM agent with tools", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Working directory for the agent
    #[arg(short = 'd', long, default_value = ".")]
    working_dir: PathBuf,

    /// LLM model to use
    #[arg(short, long, default_value = "qwen3-30b-a3b")]
    model: String,

    /// Provider name (for auth.json lookup)
    #[arg(short, long, default_value = "lm-studio")]
    provider: String,

    /// Base URL override (skips auth.json lookup)
    #[arg(long)]
    base_url: Option<String>,

    /// Verbose output (show thinking, usage)
    #[arg(short, long)]
    verbose: bool,

    /// Agent type to use (default: build)
    #[arg(short, long, default_value = "build")]
    agent: String,

    /// List available agents
    #[arg(long)]
    list_agents: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Start an interactive REPL session
    Repl,

    /// Run as ACP server over stdio (for editor integration)
    Acp,

    /// Run a single prompt
    Prompt {
        /// The prompt to send to the agent
        message: String,

        /// Continue an existing session instead of creating a new one
        #[arg(short, long)]
        session: Option<String>,
    },

    /// Show session statistics
    Stats {
        /// Number of recent sessions to show
        #[arg(short = 'n', long, default_value = "10")]
        limit: usize,
    },

    /// Show tool usage statistics
    Tools,

    /// Query the telemetry database with SQL
    Query {
        /// SQL query to run
        sql: String,
    },

    /// Show recent traces (full LLM calls)
    Traces {
        /// Number of traces to show
        #[arg(short = 'n', long, default_value = "10")]
        limit: usize,

        /// Output as JSON
        #[arg(short, long)]
        json: bool,
    },

    /// Show a specific trace's full details
    Trace {
        /// Trace ID (or prefix)
        id: String,

        /// Show full content (don't truncate)
        #[arg(short, long)]
        full: bool,

        /// Output as JSON
        #[arg(short, long)]
        json: bool,
    },

    /// Replay a session - show what happened and optionally continue
    Replay {
        /// Session ID (or prefix)
        session: String,

        /// Continue the session after replay
        #[arg(short, long)]
        continue_session: bool,
    },
}

struct CrowCli {
    agent: Agent,
    tool_registry: crow_agent::tool::ToolRegistry,
    tools: Vec<async_openai::types::ChatCompletionTool>,
    /// Coagent tool registry (if using coagent mode)
    coagent_tool_registry: Option<crow_agent::tool::ToolRegistry>,
    /// Coagent tools (if using coagent mode)
    coagent_tools: Vec<async_openai::types::ChatCompletionTool>,
    telemetry: Arc<Telemetry>,
    verbose: bool,
    data_dir: PathBuf,
    model: String,
}

impl CrowCli {
    async fn new(
        provider: Arc<ProviderClient>,
        working_dir: PathBuf,
        telemetry: Arc<Telemetry>,
        verbose: bool,
        data_dir: PathBuf,
        model: String,
        agent_name: &str,
    ) -> Result<Self> {
        // Load agent registry
        let agent_registry = AgentRegistry::new_with_config(&working_dir).await;

        // Get agent config (or fall back to a default)
        let agent_config = match agent_registry.get(agent_name).await {
            Some(config) => config,
            None => {
                // If agent not found, show available agents
                let available = agent_registry.list_ids().await;
                anyhow::bail!(
                    "Unknown agent '{}'. Available agents: {}",
                    agent_name,
                    available.join(", ")
                );
            }
        };

        // Get control flow from agent config
        let control_flow = agent_config.get_control_flow();

        // Create shared TodoStore
        let todo_store = tools2::TodoStore::new();
        let session_id = telemetry.session_id().to_string();

        // Check if we need a coagent
        let (agent, coagent_tool_registry, coagent_tools) = if agent_config.has_coagent() {
            // Load coagent config
            let coagent_name = agent_config.coagent_name();
            let coagent_config = match agent_registry.get(coagent_name).await {
                Some(config) => config,
                None => {
                    anyhow::bail!(
                        "Coagent '{}' not found. Create {}.yaml in .crow/agents/ or ~/.config/crow/agents/",
                        coagent_name,
                        coagent_name
                    );
                }
            };

            // Generate unique coagent session ID
            let coagent_session_id = format!("{}-coagent", session_id);

            // Create agent with coagent and shared todos
            let agent = Agent::with_coagent_and_telemetry(
                agent_name,
                agent_config.clone(),
                coagent_config.clone(),
                provider.clone(),
                working_dir.clone(),
                control_flow,
                telemetry.clone(),
            )
            .with_shared_todos(todo_store.clone(), &session_id, &coagent_session_id);

            // Create coagent tool registry with shared TodoStore
            // Coagent gets read-only tools for now (permissions can be wired later)
            let coagent_registry = tools2::create_coagent_registry(
                working_dir.clone(),
                coagent_session_id,
                todo_store.clone(),
                false,
            );

            // Filter coagent tools based on coagent config permissions
            let coagent_tools: Vec<_> = coagent_registry
                .to_openai_tools()
                .into_iter()
                .filter(|t| coagent_config.is_tool_enabled(&t.function.name))
                .collect();

            (agent, Some(coagent_registry), coagent_tools)
        } else {
            // No coagent - create simple agent
            let agent = Agent::with_telemetry(
                agent_name,
                agent_config.clone(),
                provider.clone(),
                working_dir.clone(),
                control_flow,
                telemetry.clone(),
            );
            (agent, None, vec![])
        };

        // Create primary tool registry with task tool for subagent spawning
        let tool_registry = tools2::create_full_registry_async(
            working_dir,
            session_id,
            todo_store,
            agent_registry,
            provider,
        )
        .await;

        // Filter tools based on agent permissions
        let tools: Vec<_> = tool_registry
            .to_openai_tools()
            .into_iter()
            .filter(|t| agent_config.is_tool_enabled(&t.function.name))
            .collect();

        Ok(Self {
            agent,
            tool_registry,
            tools,
            coagent_tool_registry,
            coagent_tools,
            telemetry,
            verbose,
            data_dir,
            model,
        })
    }

    fn system_prompt(&self) -> String {
        // Use agent's custom prompt if available, otherwise use model-based prompt
        let custom_prompt = self.agent.config.system_prompt.as_deref();
        let model_id = self.model.as_str();
        let working_dir = self.agent.working_dir();

        build_system_prompt(model_id, working_dir, custom_prompt)
    }

    /// Run the agent until completion or user input needed
    /// Returns RunResult. Interaction logging is handled by InteractionGuard which
    /// saves to DB on drop, surviving cancellation.
    async fn run_agent(
        &mut self,
        messages: &mut Vec<async_openai::types::ChatCompletionRequestMessage>,
        cancellation: CancellationToken,
    ) -> Result<RunResult> {
        use crow_agent::agent::init_coagent_session;
        use crow_agent::InteractionGuard;
        use std::sync::Arc;
        use tokio::sync::Mutex;

        let (tx, mut rx) = mpsc::unbounded_channel();

        let tools = self.tools.clone();
        let verbose = self.verbose;
        let telemetry = self.telemetry.clone();
        let model = self.model.clone();

        // Initialize coagent messages if we have a coagent
        let mut coagent_messages: Option<Vec<async_openai::types::ChatCompletionRequestMessage>> =
            if self.coagent_tool_registry.is_some() {
                // Initialize coagent session with inverted roles from primary
                Some(init_coagent_session(messages))
            } else {
                None
            };

        // Use coagent tools from struct (or empty if no coagent)
        let coagent_tools = &self.coagent_tools;

        // Get coagent registry for tool execution (or fall back to primary registry)
        let coagent_registry = self
            .coagent_tool_registry
            .as_ref()
            .unwrap_or(&self.tool_registry);

        // Run agent and event processing concurrently
        // Pass tx directly to agent - when agent.run returns, tx will be dropped
        let agent_fut = self.agent.run(
            messages,
            &tools,
            &mut coagent_messages,
            coagent_tools,
            coagent_registry,
            tx,  // Move tx into run, it will be dropped when run returns
            cancellation,
        );

        // InteractionGuard accumulates the full turn and saves on drop
        // Register globally so signal handler can flush on Ctrl+C/SIGTERM
        let guard = Arc::new(Mutex::new(InteractionGuard::new(telemetry.clone(), Some(model))));
        set_global_guard(guard.clone());
        let guard_clone = guard.clone();

        let event_fut = async move {
            while let Some(event) = rx.recv().await {
                match &event {
                    AgentEvent::TextDelta { delta, .. } => {
                        print!("{}", delta);
                        std::io::stdout().flush().ok();
                        guard_clone.lock().await.push_text(delta);
                    }
                    AgentEvent::ThinkingDelta { delta, .. } => {
                        print!("\x1b[90m{}\x1b[0m", delta);
                        std::io::stdout().flush().ok();
                        guard_clone.lock().await.push_thinking(delta);
                    }
                    AgentEvent::ThinkingComplete { .. } => {
                        guard_clone.lock().await.end_thinking();
                    }
                    AgentEvent::ToolCallStart { tool, arguments, .. } => {
                        let args_preview = arguments.to_string();
                        let args_short = if args_preview.len() > 60 {
                            format!("{}...", &args_preview[..60])
                        } else {
                            args_preview
                        };
                        println!("\n\x1b[33m▶ {}: {}\x1b[0m", tool, args_short);
                        guard_clone.lock().await.push_tool_call(tool, &args_short);
                    }
                    AgentEvent::ToolCallEnd {
                        tool,
                        output,
                        duration_ms,
                        is_error,
                        arguments,
                        ..
                    } => {
                        // Log to telemetry
                        let result = if *is_error {
                            Err(output.as_str())
                        } else {
                            Ok(output.as_str())
                        };
                        telemetry
                            .log_tool_call(tool, arguments, result, *duration_ms)
                            .await;

                        // Print to console and accumulate
                        if *is_error {
                            println!("\x1b[31m✗ {} ({}ms): {}\x1b[0m", tool, duration_ms, output);
                            guard_clone.lock().await.push_tool_error(output);
                        } else {
                            let preview = if output.len() > 100 {
                                format!("{}...", &output[..100])
                            } else {
                                output.clone()
                            };
                            println!("\x1b[32m✓ {} ({}ms): {}\x1b[0m", tool, duration_ms, preview);
                            guard_clone.lock().await.push_tool_result(&preview);
                        }
                    }
                    AgentEvent::TurnComplete { .. } => {
                        println!();
                        // Flush to DB periodically (every turn)
                        guard_clone.lock().await.flush();
                    }
                    AgentEvent::Usage {
                        input_tokens,
                        output_tokens,
                        reasoning_tokens,
                        ..
                    } => {
                        if verbose {
                            let reasoning = reasoning_tokens
                                .map(|r| format!(", {} reasoning", r))
                                .unwrap_or_default();
                            println!(
                                "\x1b[90m[{} in, {} out{}]\x1b[0m",
                                input_tokens, output_tokens, reasoning
                            );
                        }
                    }
                    AgentEvent::CoagentStart { primary, coagent } => {
                        println!("\x1b[35m⟳ Coagent {} reviewing {}...\x1b[0m", coagent, primary);
                        guard_clone.lock().await.push_coagent_start(coagent, primary);
                    }
                    AgentEvent::CoagentEnd { coagent, .. } => {
                        println!("\x1b[35m✓ Coagent {} done\x1b[0m", coagent);
                        guard_clone.lock().await.push_coagent_end(coagent);
                    }
                    AgentEvent::Error { error, .. } => {
                        eprintln!("\x1b[31mError: {}\x1b[0m", error);
                        telemetry.log_error("agent", error).await;
                        guard_clone.lock().await.push_error(error);
                    }
                    _ => {}
                }
            }
        };

        // Run both concurrently - agent produces events, we consume them
        // tx was moved into agent.run, so when it returns the channel closes
        let (result, _) = tokio::join!(agent_fut, event_fut);

        // Clear global guard and explicitly complete (saves to DB)
        clear_global_guard();
        if let Ok(g) = Arc::try_unwrap(guard) {
            g.into_inner().complete();
        }

        Ok(result)
    }

    /// Load messages from a previous session to continue it
    fn load_session_messages(&self, session_prefix: &str) -> Result<Vec<async_openai::types::ChatCompletionRequestMessage>> {
        use rusqlite::Connection;

        let conn = Connection::open(self.telemetry.db_path())?;

        // Find session matching prefix
        let session_id: String = conn.query_row(
            "SELECT DISTINCT session_id FROM traces WHERE session_id LIKE ?1 ORDER BY started_at DESC LIMIT 1",
            [format!("{}%", session_prefix)],
            |row| row.get(0),
        ).map_err(|_| anyhow::anyhow!("Session not found: {}", session_prefix))?;

        println!("Continuing session: {}", &session_id[..8]);

        // Get the last trace's request_messages - this contains the full conversation history
        let request_messages: String = conn.query_row(
            r#"SELECT request_messages FROM traces WHERE session_id = ?1 ORDER BY started_at DESC LIMIT 1"#,
            [&session_id],
            |row| row.get(0),
        )?;

        // Parse the messages
        let json_messages: Vec<serde_json::Value> = serde_json::from_str(&request_messages)?;

        let mut messages: Vec<async_openai::types::ChatCompletionRequestMessage> = Vec::new();

        // Replace system prompt with current one (in case it changed)
        messages.push(
            async_openai::types::ChatCompletionRequestSystemMessageArgs::default()
                .content(self.system_prompt())
                .build()?
                .into(),
        );

        // Add all non-system messages from the history
        for msg in json_messages {
            let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("");

            match role {
                "system" => continue, // Skip, we added fresh system prompt
                "user" => {
                    if let Some(content) = msg.get("content").and_then(|c| c.as_str()) {
                        messages.push(
                            async_openai::types::ChatCompletionRequestUserMessageArgs::default()
                                .content(content)
                                .build()?
                                .into(),
                        );
                    }
                }
                "assistant" => {
                    if let Some(content) = msg.get("content").and_then(|c| c.as_str()) {
                        messages.push(
                            async_openai::types::ChatCompletionRequestAssistantMessageArgs::default()
                                .content(content)
                                .build()?
                                .into(),
                        );
                    }
                }
                _ => {}
            }
        }

        // Also get the last response and add it
        let last_response: Option<String> = conn.query_row(
            r#"SELECT response_content FROM traces WHERE session_id = ?1 ORDER BY started_at DESC LIMIT 1"#,
            [&session_id],
            |row| row.get(0),
        ).ok().flatten();

        if let Some(response) = last_response {
            messages.push(
                async_openai::types::ChatCompletionRequestAssistantMessageArgs::default()
                    .content(response)
                    .build()?
                    .into(),
            );
        }

        println!("Loaded {} messages from previous session", messages.len() - 1);
        Ok(messages)
    }

    /// Chat with optional session continuation
    async fn chat_with_history(&mut self, message: &str, session_id: Option<&str>) -> Result<()> {
        // Log user message
        self.telemetry.log_user_message(message).await;

        let mut messages = if let Some(sid) = session_id {
            self.load_session_messages(sid)?
        } else {
            vec![
                async_openai::types::ChatCompletionRequestSystemMessageArgs::default()
                    .content(self.system_prompt())
                    .build()?
                    .into(),
            ]
        };

        // Add the new user message
        messages.push(
            async_openai::types::ChatCompletionRequestUserMessageArgs::default()
                .content(message)
                .build()?
                .into(),
        );

        let cancellation = get_shutdown_token();

        // InteractionGuard handles logging (saves on drop, survives cancellation)
        let result = self.run_agent(&mut messages, cancellation).await?;

        match result {
            RunResult::Complete { summary, total_turns } => {
                println!("\x1b[36m✓ Task complete ({} turns): {}\x1b[0m", total_turns, summary);
            }
            RunResult::NeedsInput { .. } => {
                // Passthrough mode - agent returned control to user
                println!();
            }
            RunResult::MaxTurns { turns } => {
                println!("\x1b[33m⚠ Max turns reached ({})\x1b[0m", turns);
            }
            RunResult::Cancelled => {
                println!("\x1b[33m⚠ Cancelled\x1b[0m");
            }
            RunResult::Error(e) => {
                eprintln!("\x1b[31mError: {}\x1b[0m", e);
            }
        }

        Ok(())
    }

    async fn run_repl(&mut self) -> Result<()> {
        println!("Crow Agent REPL");
        println!("Working directory: {}", self.agent.working_dir().display());
        println!("Session: {}", self.telemetry.session_id());
        println!("Database: {}", self.telemetry.db_path().display());
        println!();
        println!("Commands: /quit, /clear, /stats, /help");
        println!();

        let mut rl = DefaultEditor::new()?;
        let history_path = self.data_dir.join("history.txt");
        let _ = rl.load_history(&history_path);

        let mut messages: Vec<async_openai::types::ChatCompletionRequestMessage> = vec![
            async_openai::types::ChatCompletionRequestSystemMessageArgs::default()
                .content(self.system_prompt())
                .build()?
                .into(),
        ];

        loop {
            let prompt = if messages.len() <= 1 {
                "crow> "
            } else {
                "crow>> "
            };

            match rl.readline(prompt) {
                Ok(line) => {
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }

                    rl.add_history_entry(line)?;

                    // Handle commands
                    match line {
                        "/quit" | "/exit" => {
                            println!("Goodbye!");
                            break;
                        }
                        "/clear" => {
                            messages.truncate(1);
                            println!("History cleared.");
                            continue;
                        }
                        "/stats" => {
                            let stats = self.telemetry.stats();
                            println!("{}", stats);
                            continue;
                        }
                        "/help" => {
                            println!("Commands:");
                            println!("  /quit, /exit  - Exit");
                            println!("  /clear        - Clear chat history");
                            println!("  /stats        - Show session stats");
                            println!("  /help         - Show this");
                            continue;
                        }
                        _ if line.starts_with('/') => {
                            println!("Unknown command: {}", line);
                            continue;
                        }
                        _ => {}
                    }

                    // Log and add user message
                    self.telemetry.log_user_message(line).await;
                    messages.push(
                        async_openai::types::ChatCompletionRequestUserMessageArgs::default()
                            .content(line)
                            .build()?
                            .into(),
                    );

                    let cancellation = get_shutdown_token();

                    // InteractionGuard handles logging (saves on drop, survives cancellation)
                    match self.run_agent(&mut messages, cancellation).await {
                        Ok(result) => {
                            match result {
                                RunResult::Complete { summary, .. } => {
                                    println!("\x1b[36m✓ {}\x1b[0m", summary);
                                }
                                RunResult::Cancelled => {
                                    println!("\x1b[33m⚠ Cancelled\x1b[0m");
                                }
                                RunResult::Error(e) => {
                                    eprintln!("\x1b[31mError: {}\x1b[0m", e);
                                }
                                _ => {}
                            }
                        }
                        Err(e) => {
                            eprintln!("Error: {}", e);
                            self.telemetry.log_error("repl", &e.to_string()).await;
                        }
                    }
                    println!();
                }
                Err(ReadlineError::Interrupted) => {
                    // Ctrl+C with no active turn - just show prompt again
                    println!("^C");
                    continue;
                }
                Err(ReadlineError::Eof) => {
                    println!("Goodbye!");
                    break;
                }
                Err(e) => {
                    eprintln!("Error: {:?}", e);
                    break;
                }
            }
        }

        let _ = rl.save_history(&history_path);

        // Print final stats
        let stats = self.telemetry.stats();
        println!("\nSession: {}", stats);

        Ok(())
    }
}

fn build_provider(cli: &Cli) -> Result<Arc<ProviderClient>> {
    // If base_url is provided, use it directly
    if let Some(ref base_url) = cli.base_url {
        let config = ProviderConfig::custom(&cli.provider, base_url, "UNUSED_API_KEY_ENV", &cli.model);
        return Ok(Arc::new(
            ProviderClient::new(config).map_err(|e| anyhow::anyhow!(e))?,
        ));
    }

    // Otherwise, try to get base_url from auth.json
    let auth_path = default_data_dir().join("auth.json");
    if let Ok(content) = std::fs::read_to_string(&auth_path) {
        if let Ok(auth) = serde_json::from_str::<serde_json::Value>(&content) {
            if let Some(entry) = auth.get(&cli.provider) {
                if let Some(base_url) = entry.get("base_url").and_then(|v| v.as_str()) {
                    let config =
                        ProviderConfig::custom(&cli.provider, base_url, "UNUSED", &cli.model);
                    return Ok(Arc::new(
                        ProviderClient::new(config).map_err(|e| anyhow::anyhow!(e))?,
                    ));
                }
            }
        }
    }

    // Fall back to common providers
    let config = match cli.provider.as_str() {
        "openrouter" => ProviderConfig::openrouter(),
        "openai" => ProviderConfig::openai(),
        "anthropic" => ProviderConfig::anthropic(),
        "moonshot" => ProviderConfig::moonshot(),
        _ => {
            anyhow::bail!(
                "Provider '{}' not found in auth.json and no base_url provided",
                cli.provider
            );
        }
    };

    Ok(Arc::new(
        ProviderClient::new(config).map_err(|e| anyhow::anyhow!(e))?,
    ))
}

fn build_acp_config(cli: &Cli, working_dir: PathBuf) -> Result<Config> {
    let data_dir = default_data_dir();

    // Get base_url from CLI or auth.json
    let base_url = if let Some(ref url) = cli.base_url {
        Some(url.clone())
    } else {
        let auth_path = data_dir.join("auth.json");
        if let Ok(content) = std::fs::read_to_string(&auth_path) {
            if let Ok(auth) = serde_json::from_str::<serde_json::Value>(&content) {
                auth.get(&cli.provider)
                    .and_then(|e| e.get("base_url"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            } else {
                None
            }
        } else {
            None
        }
    };

    Ok(Config::lm_studio(
        base_url.as_deref().unwrap_or("http://localhost:1234/v1"),
        &cli.model,
        working_dir,
    )
    .with_verbose(cli.verbose)
    .with_log_dir(data_dir))
}

fn show_stats(telemetry: &Telemetry, limit: usize) -> Result<()> {
    println!("Database: {}\n", telemetry.db_path().display());

    let sessions = telemetry.recent_sessions(limit)?;
    if sessions.is_empty() {
        println!("No sessions found.");
        return Ok(());
    }

    println!("Recent Sessions:");
    println!("{:-<80}", "");
    for s in sessions {
        println!(
            "  {} | {} | {} interactions | {} tokens",
            &s.id[..8],
            s.started_at,
            s.interaction_count,
            s.total_tokens
        );
        if let Some(ref model) = s.model {
            println!("    Model: {}", model);
        }
    }
    Ok(())
}

fn show_tools(telemetry: &Telemetry) -> Result<()> {
    let tools = telemetry.tool_stats()?;
    if tools.is_empty() {
        println!("No tool calls recorded.");
        return Ok(());
    }

    println!("Tool Usage:");
    println!("{:-<80}", "");
    for t in tools {
        let success_rate = if t.call_count > 0 {
            (t.success_count as f64 / t.call_count as f64) * 100.0
        } else {
            0.0
        };
        println!(
            "  {:20} | {:5} calls | {:6.1}ms avg | {:.0}% success",
            t.tool_name, t.call_count, t.avg_duration_ms, success_rate
        );
    }
    Ok(())
}

fn run_query(telemetry: &Telemetry, sql: &str) -> Result<()> {
    use rusqlite::Connection;

    let conn = Connection::open(telemetry.db_path())?;
    let mut stmt = conn.prepare(sql)?;
    let column_count = stmt.column_count();
    let column_names: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();

    println!("{}", column_names.join(" | "));
    println!("{:-<80}", "");

    let rows = stmt.query_map([], |row| {
        let mut values = Vec::new();
        for i in 0..column_count {
            let value: String = row
                .get::<_, rusqlite::types::Value>(i)
                .map(|v| match v {
                    rusqlite::types::Value::Null => "NULL".to_string(),
                    rusqlite::types::Value::Integer(i) => i.to_string(),
                    rusqlite::types::Value::Real(f) => format!("{:.2}", f),
                    rusqlite::types::Value::Text(s) => {
                        if s.len() > 50 {
                            format!("{}...", &s[..50])
                        } else {
                            s
                        }
                    }
                    rusqlite::types::Value::Blob(_) => "[BLOB]".to_string(),
                })
                .unwrap_or_else(|_| "?".to_string());
            values.push(value);
        }
        Ok(values.join(" | "))
    })?;

    for row in rows {
        println!("{}", row?);
    }
    Ok(())
}

async fn replay_session(telemetry: &Telemetry, session_prefix: &str, _continue_session: bool) -> Result<()> {
    use rusqlite::Connection;

    let conn = Connection::open(telemetry.db_path())?;

    // Find session matching prefix
    let session_id: String = conn.query_row(
        "SELECT DISTINCT session_id FROM traces WHERE session_id LIKE ?1 ORDER BY started_at DESC LIMIT 1",
        [format!("{}%", session_prefix)],
        |row| row.get(0),
    ).map_err(|_| anyhow::anyhow!("Session not found: {}", session_prefix))?;

    println!("Replaying session: {}", session_id);
    println!("{:=<80}", "");
    println!();

    // Get all traces for this session in order
    let mut stmt = conn.prepare(
        r#"SELECT started_at, latency_ms, request_messages, response_content, response_tool_calls, error
           FROM traces
           WHERE session_id = ?1
           ORDER BY started_at ASC"#
    )?;

    let rows = stmt.query_map([&session_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<i64>>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, Option<String>>(5)?,
        ))
    })?;

    for (i, row) in rows.enumerate() {
        let (started_at, latency_ms, request_messages, response_content, tool_calls, error) = row?;

        let time = started_at
            .split('T')
            .nth(1)
            .and_then(|t| t.split('.').next())
            .unwrap_or(&started_at);

        let latency = latency_ms
            .map(|ms| format!("{}ms", ms))
            .unwrap_or_else(|| "?".to_string());

        println!("\x1b[36m--- Turn {} ({} @ {}) ---\x1b[0m", i + 1, latency, time);

        // Parse and show the last user message from request_messages
        if let Ok(messages) = serde_json::from_str::<Vec<serde_json::Value>>(&request_messages) {
            // Find last user message
            for msg in messages.iter().rev() {
                if msg.get("role").and_then(|r| r.as_str()) == Some("user") {
                    if let Some(content) = msg.get("content").and_then(|c| c.as_str()) {
                        println!("\x1b[33mUser:\x1b[0m {}", content);
                        break;
                    }
                }
            }
        }

        // Show response
        if let Some(ref content) = response_content {
            // Check if there's thinking
            if content.starts_with("<thinking>") {
                if let Some(end) = content.find("</thinking>") {
                    let thinking = &content[10..end].trim();
                    let rest = content[end + 11..].trim();
                    println!("\x1b[90mThinking: {}...\x1b[0m", &thinking[..thinking.len().min(100)]);
                    if !rest.is_empty() {
                        println!("\x1b[32mAssistant:\x1b[0m {}", rest);
                    }
                } else {
                    println!("\x1b[90mThinking: {}\x1b[0m", content);
                }
            } else {
                println!("\x1b[32mAssistant:\x1b[0m {}", content);
            }
        }

        // Show tool calls
        if let Some(ref tc) = tool_calls {
            if let Ok(calls) = serde_json::from_str::<Vec<serde_json::Value>>(tc) {
                for call in calls {
                    let name = call.get("name").and_then(|n| n.as_str()).unwrap_or("?");
                    let args = call.get("arguments").and_then(|a| a.as_str()).unwrap_or("{}");
                    let args_short = if args.len() > 50 {
                        format!("{}...", &args[..50])
                    } else {
                        args.to_string()
                    };
                    println!("\x1b[35mTool:\x1b[0m {} {}", name, args_short);
                }
            }
        }

        // Show error if any
        if let Some(ref err) = error {
            println!("\x1b[31mError:\x1b[0m {}", err);
        }

        println!();
    }

    println!("{:=<80}", "");
    println!("End of session {}", &session_id[..8]);

    // TODO: if continue_session, reconstruct messages and start REPL

    Ok(())
}

fn show_traces(telemetry: &Telemetry, limit: usize, json: bool) -> Result<()> {
    use rusqlite::Connection;

    let conn = Connection::open(telemetry.db_path())?;
    let mut stmt = conn.prepare(&format!(
        r#"SELECT id, session_id, started_at, latency_ms, model_id,
                  substr(response_content, 1, 60) as preview
           FROM traces
           ORDER BY started_at DESC
           LIMIT {}"#,
        limit
    ))?;

    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<i64>>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, Option<String>>(5)?,
        ))
    })?;

    if json {
        let mut traces = Vec::new();
        for row in rows {
            let (id, session_id, started_at, latency_ms, model, _preview) = row?;
            traces.push(serde_json::json!({
                "id": id,
                "session_id": session_id,
                "started_at": started_at,
                "latency_ms": latency_ms,
                "model_id": model,
            }));
        }
        println!("{}", serde_json::to_string_pretty(&traces)?);
        return Ok(());
    }

    println!("Recent Traces:");
    println!("{:-<100}", "");

    for row in rows {
        let (id, session_id, started_at, latency_ms, model, preview) = row?;
        let latency = latency_ms
            .map(|ms| format!("{}ms", ms))
            .unwrap_or_else(|| "?".to_string());
        let model = model.unwrap_or_else(|| "?".to_string());
        let preview = preview.unwrap_or_default();

        let time = started_at
            .split('T')
            .nth(1)
            .and_then(|t| t.split('.').next())
            .unwrap_or(&started_at);

        println!(
            "{} {} {:>8} [{}] {} {}",
            time,
            &id[..8],
            latency,
            &session_id[..8],
            model,
            preview
        );
    }
    Ok(())
}

fn show_trace(telemetry: &Telemetry, id_prefix: &str, full: bool, json: bool) -> Result<()> {
    use rusqlite::Connection;

    let conn = Connection::open(telemetry.db_path())?;

    // Find trace matching prefix
    let row: (String, String, String, Option<String>, String, String, String, Option<String>, Option<String>, Option<String>, Option<i64>, Option<i64>, Option<i64>, Option<i64>, Option<String>, Option<String>) = conn.query_row(
        r#"SELECT id, session_id, started_at, completed_at, model_provider, model_id,
                  request_messages, request_tools, response_content, response_tool_calls,
                  input_tokens, output_tokens, total_tokens, latency_ms, error, agent_name
           FROM traces
           WHERE id LIKE ?1
           ORDER BY started_at DESC
           LIMIT 1"#,
        [format!("{}%", id_prefix)],
        |row| Ok((
            row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?,
            row.get(4)?, row.get(5)?, row.get(6)?, row.get(7)?,
            row.get(8)?, row.get(9)?, row.get(10)?, row.get(11)?,
            row.get(12)?, row.get(13)?, row.get(14)?, row.get(15)?,
        )),
    ).map_err(|_| anyhow::anyhow!("Trace not found: {}", id_prefix))?;

    let (id, session_id, started_at, completed_at, model_provider, model_id,
         request_messages, request_tools, response_content, response_tool_calls,
         input_tokens, output_tokens, total_tokens, latency_ms, error, agent_name) = row;

    if json {
        let trace = serde_json::json!({
            "id": id,
            "session_id": session_id,
            "agent_name": agent_name,
            "started_at": started_at,
            "completed_at": completed_at,
            "model_provider": model_provider,
            "model_id": model_id,
            "request_messages": serde_json::from_str::<serde_json::Value>(&request_messages).unwrap_or(serde_json::Value::Null),
            "request_tools": request_tools.as_ref().and_then(|t| serde_json::from_str::<serde_json::Value>(t).ok()),
            "response_content": response_content,
            "response_tool_calls": response_tool_calls.as_ref().and_then(|t| serde_json::from_str::<serde_json::Value>(t).ok()),
            "input_tokens": input_tokens,
            "output_tokens": output_tokens,
            "total_tokens": total_tokens,
            "latency_ms": latency_ms,
            "error": error,
        });
        println!("{}", serde_json::to_string_pretty(&trace)?);
        return Ok(());
    }

    // Pretty print
    println!("Trace Details");
    println!("{:─<80}", "");
    println!("ID: {}", id);
    println!("Session ID: {}", session_id);
    println!("Agent: {}", agent_name.unwrap_or_else(|| "unknown".to_string()));
    println!("Model: {}/{}", model_provider, model_id);
    println!("Started: {}", started_at);
    if let Some(ref completed) = completed_at {
        println!("Completed: {}", completed);
    }
    if let Some(ms) = latency_ms {
        println!("Latency: {}ms", ms);
    }
    println!();

    // Token usage
    println!("Token Usage");
    println!("{:─<40}", "");
    println!("  Input:  {}", input_tokens.map(|t| t.to_string()).unwrap_or_else(|| "-".to_string()));
    println!("  Output: {}", output_tokens.map(|t| t.to_string()).unwrap_or_else(|| "-".to_string()));
    println!("  Total:  {}", total_tokens.map(|t| t.to_string()).unwrap_or_else(|| "-".to_string()));
    println!();

    // Request messages
    println!("Request Messages");
    println!("{:─<80}", "");
    if let Ok(messages) = serde_json::from_str::<Vec<serde_json::Value>>(&request_messages) {
        for msg in &messages {
            let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("?");
            let content = msg.get("content").and_then(|c| c.as_str()).unwrap_or("");
            let display = if full || content.len() <= 500 {
                content.to_string()
            } else {
                format!("{}...\n({} chars total, use --full to see all)", &content[..500], content.len())
            };
            println!("[{}]: {}", role, display);
            println!();
        }
    } else {
        println!("{}", request_messages);
    }

    // Request tools
    if let Some(ref tools) = request_tools {
        println!("Request Tools");
        println!("{:─<80}", "");
        if let Ok(tools_arr) = serde_json::from_str::<Vec<serde_json::Value>>(tools) {
            let tool_names: Vec<String> = tools_arr.iter()
                .filter_map(|t| t.get("function").and_then(|f| f.get("name")).and_then(|n| n.as_str()))
                .map(|s| s.to_string())
                .collect();
            println!("{} tools: {}", tool_names.len(), tool_names.join(", "));
            if full {
                println!();
                println!("{}", serde_json::to_string_pretty(&tools_arr)?);
            } else {
                println!("(use --full to see tool schemas)");
            }
        } else {
            println!("{}", tools);
        }
        println!();
    }

    // Response content
    println!("Response Content");
    println!("{:─<80}", "");
    if let Some(ref content) = response_content {
        let display = if full || content.len() <= 1000 {
            content.clone()
        } else {
            format!("{}...\n({} chars total, use --full to see all)", &content[..1000], content.len())
        };
        println!("{}", display);
    } else {
        println!("(no content)");
    }
    println!();

    // Response tool calls
    if let Some(ref tool_calls) = response_tool_calls {
        println!("Response Tool Calls");
        println!("{:─<80}", "");
        if let Ok(calls) = serde_json::from_str::<Vec<serde_json::Value>>(tool_calls) {
            for call in &calls {
                let name = call.get("name").and_then(|n| n.as_str()).unwrap_or("?");
                let args = call.get("arguments").and_then(|a| a.as_str()).unwrap_or("{}");
                println!("  {} {}", name, if full { args.to_string() } else {
                    if args.len() > 100 { format!("{}...", &args[..100]) } else { args.to_string() }
                });
            }
        } else {
            println!("{}", tool_calls);
        }
        println!();
    }

    // Error
    if let Some(ref err) = error {
        println!("Error");
        println!("{:─<80}", "");
        println!("\x1b[31m{}\x1b[0m", err);
        println!();
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    // Set up signal handling for graceful shutdown
    let shutdown = get_shutdown_token();
    tokio::spawn(async move {
        use tokio::signal::unix::{signal, SignalKind};

        let mut sigint = signal(SignalKind::interrupt()).expect("SIGINT handler");
        let mut sigterm = signal(SignalKind::terminate()).expect("SIGTERM handler");

        tokio::select! {
            _ = sigint.recv() => {
                eprintln!("\n[SIGINT received, cancelling...]");
            }
            _ = sigterm.recv() => {
                eprintln!("\n[SIGTERM received, cancelling...]");
            }
        }

        // Flush any pending interaction data before shutdown
        flush_global_guard();
        shutdown.cancel();
    });

    let cli = Cli::parse();

    // Resolve working directory
    let working_dir = if cli.working_dir.is_absolute() {
        cli.working_dir.clone()
    } else {
        std::env::current_dir()?.join(&cli.working_dir)
    }
    .canonicalize()?;

    let data_dir = default_data_dir();
    std::fs::create_dir_all(&data_dir)?;

    // Check if ACP mode (needs special telemetry - logs to stderr since stdout is JSON-RPC)
    let is_acp_mode = matches!(cli.command, Some(Commands::Acp));

    // Initialize telemetry
    let telemetry = if is_acp_mode {
        Arc::new(Telemetry::init_for_serve(
            data_dir.clone(),
            cli.verbose,
            None, // No OTel endpoint for now
            Some(&working_dir.display().to_string()),
            Some(&cli.model),
            Some(&cli.provider),
        )?)
    } else {
        Arc::new(Telemetry::init(
            data_dir.clone(),
            cli.verbose,
            None, // No OTel endpoint for now
            Some(&working_dir.display().to_string()),
            Some(&cli.model),
            Some(&cli.provider),
        )?)
    };

    // Handle ACP mode first (special handling for stdio server)
    if let Some(Commands::Acp) = cli.command {
        // Build config for ACP server
        let config = build_acp_config(&cli, working_dir)?;

        // Run as ACP server - use LocalSet for !Send futures
        let local = tokio::task::LocalSet::new();
        return local
            .run_until(run_stdio_server(config, telemetry))
            .await
            .map_err(|e| anyhow::anyhow!("ACP server error: {:?}", e));
    }

    // Handle non-agent commands first (don't need provider)
    match &cli.command {
        Some(Commands::Stats { limit }) => {
            return show_stats(&telemetry, *limit);
        }
        Some(Commands::Tools) => {
            return show_tools(&telemetry);
        }
        Some(Commands::Query { sql }) => {
            return run_query(&telemetry, sql);
        }
        Some(Commands::Traces { limit, json }) => {
            return show_traces(&telemetry, *limit, *json);
        }
        Some(Commands::Trace { id, full, json }) => {
            return show_trace(&telemetry, id, *full, *json);
        }
        Some(Commands::Replay { session, continue_session }) => {
            return replay_session(&telemetry, session, *continue_session).await;
        }
        _ => {}
    }

    // Handle --list-agents
    if cli.list_agents {
        let agent_registry = AgentRegistry::new_with_config(&working_dir).await;
        println!("Available Agents:");
        println!("{:-<60}", "");

        let mut agents = agent_registry.get_all().await;
        agents.sort_by(|a, b| a.name.cmp(&b.name));

        for agent in agents {
            let mode = match agent.mode {
                crow_agent::agent::AgentMode::Primary => "primary",
                crow_agent::agent::AgentMode::Subagent => "subagent",
                crow_agent::agent::AgentMode::Coagent => "coagent",
                crow_agent::agent::AgentMode::All => "all",
            };
            let builtin = if agent.built_in { " (built-in)" } else { "" };
            let desc = agent
                .description
                .as_deref()
                .unwrap_or("No description");

            println!(
                "  {:12} [{:8}]{} - {}",
                agent.name, mode, builtin, desc
            );
        }
        return Ok(());
    }

    // Build provider for agent commands
    let provider = build_provider(&cli)?;

    if cli.verbose {
        println!("Provider: {} ({})", cli.provider, cli.model);
        println!("Agent: {}", cli.agent);
        println!("Session: {}", telemetry.session_id());
        println!("Database: {}", telemetry.db_path().display());
        println!();
    }

    // Create CLI with selected agent
    let mut crow = CrowCli::new(
        provider,
        working_dir,
        telemetry.clone(),
        cli.verbose,
        data_dir,
        cli.model.clone(),
        &cli.agent,
    )
    .await?;

    match cli.command {
        Some(Commands::Prompt { message, session }) => {
            crow.chat_with_history(&message, session.as_deref()).await?;
        }
        Some(Commands::Repl) | None => {
            crow.run_repl().await?;
        }
        _ => unreachable!(),
    }

    Ok(())
}
