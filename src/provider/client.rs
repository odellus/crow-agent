//! OpenAI-compatible provider client
//!
//! Handles streaming chat completions with tool support.
//! Uses raw HTTP for streaming to capture reasoning_content (extended thinking)
//! which async-openai doesn't support.

use super::ProviderConfig;
use async_openai::{
    config::OpenAIConfig,
    types::{
        ChatCompletionRequestMessage, ChatCompletionTool, CreateChatCompletionRequestArgs,
        CreateChatCompletionResponse,
    },
    Client,
};
use futures::StreamExt;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Delta events from streaming LLM response
#[derive(Debug, Clone)]
pub enum StreamDelta {
    /// Text content delta
    Text(String),
    /// Reasoning/thinking content delta (for models like claude with extended thinking)
    Reasoning(String),
    /// Tool call delta
    ToolCall {
        index: usize,
        id: Option<String>,
        name: Option<String>,
        arguments: String,
    },
    /// Usage info (sent at end of stream)
    Usage {
        input: u64,
        output: u64,
        reasoning: Option<u64>,
    },
    /// Stream finished
    Done,
}

// Internal types for parsing streaming responses
#[derive(Debug, serde::Deserialize)]
struct StreamChunkDelta {
    content: Option<String>,
    reasoning_content: Option<String>,
    tool_calls: Option<Vec<StreamToolCallChunk>>,
    #[allow(dead_code)]
    role: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct StreamToolCallChunk {
    index: usize,
    id: Option<String>,
    function: Option<StreamFunctionChunk>,
}

#[derive(Debug, serde::Deserialize)]
struct StreamFunctionChunk {
    name: Option<String>,
    arguments: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct StreamChoice {
    delta: StreamChunkDelta,
    #[allow(dead_code)]
    finish_reason: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct CompletionTokensDetails {
    reasoning_tokens: Option<u64>,
}

#[derive(Debug, serde::Deserialize)]
struct StreamUsage {
    prompt_tokens: u64,
    completion_tokens: u64,
    completion_tokens_details: Option<CompletionTokensDetails>,
}

#[derive(Debug, serde::Deserialize)]
struct StreamChunk {
    choices: Vec<StreamChoice>,
    usage: Option<StreamUsage>,
}

/// OpenAI-compatible client wrapper
#[derive(Clone)]
pub struct ProviderClient {
    config: ProviderConfig,
    client: Client<OpenAIConfig>,
    http_client: reqwest::Client,
}

impl ProviderClient {
    /// Create a new provider client from config
    pub fn new(config: ProviderConfig) -> Result<Self, String> {
        let api_key = Self::get_api_key(&config)?;

        let openai_config = OpenAIConfig::new()
            .with_api_key(&api_key)
            .with_api_base(&config.base_url);

        let client = Client::with_config(openai_config);
        // Build HTTP client with connection close behavior
        // This ensures connections are closed properly on drop
        let http_client = reqwest::Client::builder()
            .pool_max_idle_per_host(0) // Don't keep connections alive
            .build()
            .map_err(|e| format!("Failed to build HTTP client: {}", e))?;

        Ok(Self {
            config,
            client,
            http_client,
        })
    }

    /// Get API key from environment or XDG auth.json
    fn get_api_key(config: &ProviderConfig) -> Result<String, String> {
        // Try loading .env file
        let _ = dotenvy::dotenv();

        // First, try environment variable
        if let Ok(key) = std::env::var(&config.api_key_env) {
            return Ok(key);
        }

        // Second, try XDG auth.json
        if let Some(key) = Self::get_key_from_auth_json(&config.name) {
            return Ok(key);
        }

        Err(format!(
            "{} not found in environment or ~/.local/share/crow/auth.json",
            config.api_key_env
        ))
    }

    /// Try to read API key from XDG auth.json
    fn get_key_from_auth_json(provider_name: &str) -> Option<String> {
        let auth_path = dirs::data_dir()?.join("crow").join("auth.json");

        let content = std::fs::read_to_string(&auth_path).ok()?;
        let auth: serde_json::Value = serde_json::from_str(&content).ok()?;

        // Map provider names to auth.json keys
        let lowercase = provider_name.to_lowercase();
        let auth_key = match lowercase.as_str() {
            "openrouter" => "openrouter",
            "openai" => "openai",
            "anthropic" => "anthropic",
            "moonshot" => "moonshotai",
            "lm studio" | "lm-studio" => "lm-studio",
            _ => &lowercase,
        };

        auth.get(auth_key)?
            .get("key")?
            .as_str()
            .map(|s| s.to_string())
    }

    /// Get the provider config
    pub fn config(&self) -> &ProviderConfig {
        &self.config
    }

    /// Send a non-streaming chat completion request with tools
    pub async fn chat_with_tools(
        &self,
        messages: Vec<ChatCompletionRequestMessage>,
        tools: Vec<ChatCompletionTool>,
        model: Option<&str>,
    ) -> Result<CreateChatCompletionResponse, String> {
        let model = model.unwrap_or(&self.config.default_model);

        let mut request_builder = CreateChatCompletionRequestArgs::default();
        request_builder.model(model).messages(messages);

        if !tools.is_empty() {
            request_builder.tools(tools);
        }

        let request = request_builder
            .build()
            .map_err(|e| format!("Failed to build request: {}", e))?;

        self.client
            .chat()
            .create(request)
            .await
            .map_err(|e| format!("API call failed: {}", e))
    }

    /// Send a streaming chat completion request with tools
    ///
    /// Streams deltas through the provided channel.
    /// Supports cancellation via the cancellation token.
    pub async fn chat_stream(
        &self,
        messages: Vec<ChatCompletionRequestMessage>,
        tools: Vec<ChatCompletionTool>,
        model: Option<&str>,
        tx: mpsc::UnboundedSender<StreamDelta>,
        cancellation: Option<CancellationToken>,
    ) -> Result<(), String> {
        let model = model.unwrap_or(&self.config.default_model);
        let api_key = Self::get_api_key(&self.config)?;

        // Build request body manually for streaming
        // (async-openai's streaming doesn't capture reasoning_content)
        let tools_json: Vec<serde_json::Value> = tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": t.function.name,
                        "description": t.function.description,
                        "parameters": t.function.parameters
                    }
                })
            })
            .collect();

        let messages_json: Vec<serde_json::Value> =
            messages.iter().map(|m| self.message_to_json(m)).collect();

        let mut body = serde_json::json!({
            "model": model,
            "messages": messages_json,
            "stream": true,
            "stream_options": {"include_usage": true},
            // llama.cpp/LM Studio: enable prompt caching for faster responses
            "cache_prompt": true
        });

        if !tools.is_empty() {
            body["tools"] = serde_json::json!(tools_json);
        }

        // Make the HTTP request cancellable - this is critical for llama.cpp
        // which can take minutes during prompt processing
        // Use Connection: close to ensure server doesn't keep-alive
        let request_fut = self
            .http_client
            .post(format!("{}/chat/completions", self.config.base_url))
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json")
            .header("Connection", "close")
            .json(&body)
            .send();

        let response = if let Some(ref token) = cancellation {
            tokio::select! {
                biased;
                _ = token.cancelled() => {
                    let _ = tx.send(StreamDelta::Done);
                    return Err("Request cancelled during prompt processing".to_string());
                }
                result = request_fut => result.map_err(|e| format!("API request failed: {}", e))?,
            }
        } else {
            request_fut.await.map_err(|e| format!("API request failed: {}", e))?
        };

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(format!("API error {}: {}", status, text));
        }

        let mut stream = response.bytes_stream();
        let mut buffer = String::new();

        loop {
            // Check for cancellation
            if let Some(ref token) = cancellation {
                if token.is_cancelled() {
                    // Drop the stream explicitly to close the HTTP connection
                    // This signals llama.cpp to cancel the request
                    drop(stream);
                    let _ = tx.send(StreamDelta::Done);
                    return Err("Stream cancelled".to_string());
                }
            }

            // Get next chunk, with cancellation support
            let result = if let Some(ref token) = cancellation {
                tokio::select! {
                    biased;
                    _ = token.cancelled() => {
                        // Drop the stream explicitly to close the HTTP connection
                        drop(stream);
                        let _ = tx.send(StreamDelta::Done);
                        return Err("Stream cancelled".to_string());
                    }
                    item = stream.next() => item,
                }
            } else {
                stream.next().await
            };

            let Some(result) = result else {
                break;
            };

            let bytes = result.map_err(|e| format!("Stream read error: {}", e))?;
            buffer.push_str(&String::from_utf8_lossy(&bytes));

            // Process complete SSE lines
            while let Some(pos) = buffer.find('\n') {
                let line = buffer[..pos].trim().to_string();
                buffer = buffer[pos + 1..].to_string();

                if line.is_empty() || line.starts_with(':') {
                    continue;
                }

                if let Some(data) = line.strip_prefix("data: ") {
                    if data == "[DONE]" {
                        break;
                    }

                    if let Ok(chunk) = serde_json::from_str::<StreamChunk>(data) {
                        // Handle usage info
                        if let Some(usage) = &chunk.usage {
                            let reasoning = usage
                                .completion_tokens_details
                                .as_ref()
                                .and_then(|d| d.reasoning_tokens);
                            let _ = tx.send(StreamDelta::Usage {
                                input: usage.prompt_tokens,
                                output: usage.completion_tokens,
                                reasoning,
                            });
                        }

                        for choice in &chunk.choices {
                            // Handle reasoning content (extended thinking)
                            if let Some(reasoning) = &choice.delta.reasoning_content {
                                if !reasoning.is_empty() {
                                    let _ = tx.send(StreamDelta::Reasoning(reasoning.clone()));
                                }
                            }

                            // Handle text content
                            if let Some(content) = &choice.delta.content {
                                if !content.is_empty() {
                                    let _ = tx.send(StreamDelta::Text(content.clone()));
                                }
                            }

                            // Handle tool calls
                            if let Some(tool_calls) = &choice.delta.tool_calls {
                                for tc in tool_calls {
                                    let _ = tx.send(StreamDelta::ToolCall {
                                        index: tc.index,
                                        id: tc.id.clone(),
                                        name: tc.function.as_ref().and_then(|f| f.name.clone()),
                                        arguments: tc
                                            .function
                                            .as_ref()
                                            .and_then(|f| f.arguments.clone())
                                            .unwrap_or_default(),
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }

        let _ = tx.send(StreamDelta::Done);
        Ok(())
    }

    /// Send a non-streaming chat completion with structured JSON output
    ///
    /// Uses OpenAI's response_format with json_schema for guaranteed structured output.
    pub async fn chat_structured<T: serde::de::DeserializeOwned>(
        &self,
        messages: Vec<ChatCompletionRequestMessage>,
        schema_name: &str,
        schema: serde_json::Value,
        model: Option<&str>,
    ) -> Result<T, String> {
        let model = model.unwrap_or(&self.config.default_model);
        let api_key = Self::get_api_key(&self.config)?;

        let messages_json: Vec<serde_json::Value> =
            messages.iter().map(|m| self.message_to_json(m)).collect();

        let body = serde_json::json!({
            "model": model,
            "messages": messages_json,
            "response_format": {
                "type": "json_schema",
                "json_schema": {
                    "name": schema_name,
                    "strict": true,
                    "schema": schema
                }
            }
        });

        let start = std::time::Instant::now();
        tracing::info!(
            target: "llm",
            schema_name = schema_name,
            model = model,
            message_count = messages.len(),
            "Starting structured LLM call"
        );

        let response = self
            .http_client
            .post(format!("{}/chat/completions", self.config.base_url))
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                tracing::error!(target: "llm", error = %e, "Structured LLM call failed");
                format!("API request failed: {}", e)
            })?;

        let elapsed = start.elapsed();

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            tracing::error!(
                target: "llm",
                status = %status,
                error = %text,
                elapsed_ms = elapsed.as_millis() as u64,
                "Structured LLM call returned error"
            );
            return Err(format!("API error {}: {}", status, text));
        }

        let response_body: serde_json::Value = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse response: {}", e))?;

        // Extract usage if available
        let usage = response_body.get("usage");
        let input_tokens = usage.and_then(|u| u.get("prompt_tokens")).and_then(|v| v.as_u64());
        let output_tokens = usage.and_then(|u| u.get("completion_tokens")).and_then(|v| v.as_u64());

        // Extract the content from the response
        let content = response_body
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .ok_or_else(|| "No content in response".to_string())?;

        tracing::info!(
            target: "llm",
            schema_name = schema_name,
            model = model,
            elapsed_ms = elapsed.as_millis() as u64,
            input_tokens = input_tokens,
            output_tokens = output_tokens,
            "Structured LLM call completed"
        );

        // Parse the JSON content into the expected type
        serde_json::from_str(content)
            .map_err(|e| format!("Failed to parse structured response: {} (raw: {})", e, content))
    }

    /// Send a non-streaming chat completion with forced tool call for structured output
    ///
    /// Uses tool_choice: "required" with a single tool instead of response_format.
    /// This works with thinking models that don't support response_format prefill.
    pub async fn chat_tool_structured<T: serde::de::DeserializeOwned>(
        &self,
        messages: Vec<ChatCompletionRequestMessage>,
        tool_name: &str,
        tool_description: &str,
        schema: serde_json::Value,
        model: Option<&str>,
    ) -> Result<T, String> {
        let model = model.unwrap_or(&self.config.default_model);
        let api_key = Self::get_api_key(&self.config)?;

        let messages_json: Vec<serde_json::Value> =
            messages.iter().map(|m| self.message_to_json(m)).collect();

        // Create a tool with the schema as parameters
        let tool = serde_json::json!({
            "type": "function",
            "function": {
                "name": tool_name,
                "description": tool_description,
                "parameters": schema
            }
        });

        let body = serde_json::json!({
            "model": model,
            "messages": messages_json,
            "tools": [tool],
            "tool_choice": {"type": "function", "function": {"name": tool_name}}
        });

        let start = std::time::Instant::now();
        tracing::info!(
            target: "llm",
            tool_name = tool_name,
            model = model,
            message_count = messages.len(),
            "Starting tool-structured LLM call"
        );

        let response = self
            .http_client
            .post(format!("{}/chat/completions", self.config.base_url))
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                tracing::error!(target: "llm", error = %e, "Tool-structured LLM call failed");
                format!("API request failed: {}", e)
            })?;

        let elapsed = start.elapsed();

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            tracing::error!(
                target: "llm",
                status = %status,
                error = %text,
                elapsed_ms = elapsed.as_millis() as u64,
                "Tool-structured LLM call returned error"
            );
            return Err(format!("API error {}: {}", status, text));
        }

        let response_body: serde_json::Value = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse response: {}", e))?;

        // Extract usage if available
        let usage = response_body.get("usage");
        let input_tokens = usage.and_then(|u| u.get("prompt_tokens")).and_then(|v| v.as_u64());
        let output_tokens = usage.and_then(|u| u.get("completion_tokens")).and_then(|v| v.as_u64());

        // Extract the tool call arguments from the response
        let arguments = response_body
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("tool_calls"))
            .and_then(|tc| tc.get(0))
            .and_then(|tc| tc.get("function"))
            .and_then(|f| f.get("arguments"))
            .and_then(|a| a.as_str())
            .ok_or_else(|| "No tool call arguments in response".to_string())?;

        tracing::info!(
            target: "llm",
            tool_name = tool_name,
            model = model,
            elapsed_ms = elapsed.as_millis() as u64,
            input_tokens = input_tokens,
            output_tokens = output_tokens,
            "Tool-structured LLM call completed"
        );

        // Parse the arguments JSON into the expected type
        serde_json::from_str(arguments)
            .map_err(|e| format!("Failed to parse tool arguments: {} (raw: {})", e, arguments))
    }

    /// Convert a ChatCompletionRequestMessage to JSON
    fn message_to_json(&self, msg: &ChatCompletionRequestMessage) -> serde_json::Value {
        use async_openai::types::*;

        match msg {
            ChatCompletionRequestMessage::System(s) => {
                serde_json::json!({
                    "role": "system",
                    "content": s.content
                })
            }
            ChatCompletionRequestMessage::User(u) => {
                let content = match &u.content {
                    ChatCompletionRequestUserMessageContent::Text(t) => t.clone(),
                    ChatCompletionRequestUserMessageContent::Array(parts) => parts
                        .iter()
                        .filter_map(|p| {
                            if let ChatCompletionRequestUserMessageContentPart::Text(t) = p {
                                Some(t.text.clone())
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(""),
                };
                serde_json::json!({
                    "role": "user",
                    "content": content
                })
            }
            ChatCompletionRequestMessage::Assistant(a) => {
                let mut msg = serde_json::json!({ "role": "assistant" });
                if let Some(content) = &a.content {
                    msg["content"] = serde_json::json!(content);
                }
                if let Some(tool_calls) = &a.tool_calls {
                    msg["tool_calls"] = serde_json::json!(tool_calls);
                }
                msg
            }
            ChatCompletionRequestMessage::Tool(t) => {
                serde_json::json!({
                    "role": "tool",
                    "tool_call_id": t.tool_call_id,
                    "content": t.content
                })
            }
            _ => serde_json::json!({"role": "unknown"}),
        }
    }
}
