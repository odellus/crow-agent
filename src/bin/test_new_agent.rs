//! Quick test binary for new agent system
//!
//! Run with: cargo run --bin test_new_agent

use crow_agent::{
    agent::{AgentConfig, BaseAgent},
    events::AgentEvent,
    provider::{ProviderClient, ProviderConfig},
    tools2,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Setup
    let working_dir = std::env::current_dir()?;
    println!("Working directory: {}", working_dir.display());

    // Create provider using LM Studio from auth.json
    let provider_config = ProviderConfig::custom(
        "lm-studio",
        "http://192.168.1.175:1234/v1",
        "LM_STUDIO_API_KEY", // won't be found, will fallback to auth.json
        "qwen3-30b-a3b",
    );
    let provider = match ProviderClient::new(provider_config) {
        Ok(p) => std::sync::Arc::new(p),
        Err(e) => {
            eprintln!("Failed to create provider: {}", e);
            eprintln!("Make sure OPENROUTER_API_KEY is set");
            return Ok(());
        }
    };

    // Create tool registry
    let registry = tools2::create_registry(working_dir.clone());
    let tools = registry.to_openai_tools();
    println!("Registered {} tools: {:?}", tools.len(), registry.names());

    // Create agent config
    let config = AgentConfig::new("test-agent");

    // Create base agent
    let agent = BaseAgent::new(config, provider, working_dir);

    // Create message history with a simple prompt
    let mut messages = vec![
        async_openai::types::ChatCompletionRequestSystemMessageArgs::default()
            .content("You are a helpful assistant. You have access to tools. When you're done, call task_complete with a summary.")
            .build()?
            .into(),
        async_openai::types::ChatCompletionRequestUserMessageArgs::default()
            .content("Read the Cargo.toml file and tell me the package name. Then call task_complete.")
            .build()?
            .into(),
    ];

    // Create event channel
    let (tx, mut rx) = mpsc::unbounded_channel();

    // Spawn event printer
    let printer = tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            match &event {
                AgentEvent::TextDelta { delta, .. } => {
                    print!("{}", delta);
                    use std::io::Write;
                    std::io::stdout().flush().ok();
                }
                AgentEvent::ThinkingDelta { delta, .. } => {
                    print!("\x1b[90m{}\x1b[0m", delta); // Gray for thinking
                    use std::io::Write;
                    std::io::stdout().flush().ok();
                }
                AgentEvent::ToolCallStart { tool, .. } => {
                    println!("\n\x1b[33m▶ Calling tool: {}\x1b[0m", tool);
                }
                AgentEvent::ToolCallEnd {
                    tool,
                    output,
                    duration_ms,
                    is_error,
                    ..
                } => {
                    if *is_error {
                        println!(
                            "\x1b[31m✗ {} failed ({}ms): {}\x1b[0m",
                            tool, duration_ms, output
                        );
                    } else {
                        let preview = if output.len() > 200 {
                            format!("{}...", &output[..200])
                        } else {
                            output.clone()
                        };
                        println!("\x1b[32m✓ {} ({}ms): {}\x1b[0m", tool, duration_ms, preview);
                    }
                }
                AgentEvent::TurnComplete { reason, .. } => {
                    println!("\n\x1b[36m▣ Turn complete: {:?}\x1b[0m", reason);
                }
                AgentEvent::Usage {
                    input_tokens,
                    output_tokens,
                    ..
                } => {
                    println!(
                        "\x1b[90m  tokens: {} in, {} out\x1b[0m",
                        input_tokens, output_tokens
                    );
                }
                _ => {}
            }
        }
    });

    // Run the agent
    println!("\n--- Starting agent ---\n");
    let cancellation = CancellationToken::new();

    let result = agent
        .execute_turn(&mut messages, &tools, &registry, &tx, cancellation)
        .await;

    // Wait for printer to finish
    drop(tx);
    printer.await?;

    println!("\n--- Result ---");
    match result {
        Ok(turn_result) => {
            println!("Reason: {:?}", turn_result.reason);
            println!("Tool calls: {}", turn_result.tool_calls.len());
            if let Some(text) = &turn_result.text {
                println!("Final text: {}", text);
            }
        }
        Err(e) => {
            println!("Error: {}", e);
        }
    }

    Ok(())
}
