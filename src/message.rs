//! Semantic message types for conversation history.
//!
//! Following Zed's pattern: store semantic message structures, not wire format.
//! On every request, call `to_request()` to generate the LLM wire format.
//! This ensures deterministic serialization for prompt caching.

use async_openai::types::{
    ChatCompletionMessageToolCall, ChatCompletionRequestAssistantMessageArgs,
    ChatCompletionRequestMessage, ChatCompletionRequestSystemMessageArgs,
    ChatCompletionRequestToolMessageArgs, ChatCompletionRequestUserMessageArgs,
};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Unique identifier for a tool use
pub type ToolUseId = Arc<str>;

/// A message in the conversation
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Message {
    User(UserMessage),
    Agent(AgentMessage),
    Resume,
}

impl Message {
    /// Convert to LLM request messages.
    /// AgentMessage can produce multiple messages (assistant + tool results as user).
    pub fn to_request(&self) -> Vec<ChatCompletionRequestMessage> {
        match self {
            Message::User(msg) => vec![msg.to_request()],
            Message::Agent(msg) => msg.to_request(),
            Message::Resume => {
                vec![ChatCompletionRequestUserMessageArgs::default()
                    .content("Continue where you left off")
                    .build()
                    .unwrap()
                    .into()]
            }
        }
    }
}

/// A user message
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserMessage {
    pub content: String,
}

impl UserMessage {
    pub fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
        }
    }

    pub fn to_request(&self) -> ChatCompletionRequestMessage {
        ChatCompletionRequestUserMessageArgs::default()
            .content(self.content.clone())
            .build()
            .unwrap()
            .into()
    }
}

/// An agent (assistant) message
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentMessage {
    /// Text and tool use content
    pub content: Vec<AgentMessageContent>,
    /// Tool results, keyed by tool_use_id
    pub tool_results: IndexMap<ToolUseId, ToolResult>,
}

impl Default for AgentMessage {
    fn default() -> Self {
        Self {
            content: Vec::new(),
            tool_results: IndexMap::new(),
        }
    }
}

impl AgentMessage {
    /// Convert to LLM request messages.
    /// Returns assistant message (with tool_calls if any) followed by
    /// tool result messages.
    pub fn to_request(&self) -> Vec<ChatCompletionRequestMessage> {
        let mut messages = Vec::new();

        // Build assistant message content
        let mut text_content = String::new();
        let mut tool_calls: Vec<ChatCompletionMessageToolCall> = Vec::new();

        for content in &self.content {
            match content {
                AgentMessageContent::Text(text) => {
                    if !text_content.is_empty() {
                        text_content.push_str("\n");
                    }
                    text_content.push_str(text);
                }
                AgentMessageContent::Thinking { text, .. } => {
                    // Include thinking in content for models that support it
                    // For now, we'll include it as text (can be enhanced later)
                    if !text_content.is_empty() {
                        text_content.push_str("\n");
                    }
                    text_content.push_str("<thinking>");
                    text_content.push_str(text);
                    text_content.push_str("</thinking>");
                }
                AgentMessageContent::ToolUse(tool_use) => {
                    // Only include tool_use if we have a result for it
                    if self.tool_results.contains_key(&tool_use.id) {
                        tool_calls.push(ChatCompletionMessageToolCall {
                            id: tool_use.id.to_string(),
                            r#type: async_openai::types::ChatCompletionToolType::Function,
                            function: async_openai::types::FunctionCall {
                                name: tool_use.name.clone(),
                                arguments: tool_use.arguments.clone(),
                            },
                        });
                    }
                }
            }
        }

        // Add assistant message if we have content or tool calls
        if !text_content.is_empty() || !tool_calls.is_empty() {
            let mut builder = ChatCompletionRequestAssistantMessageArgs::default();

            if !text_content.is_empty() {
                builder.content(text_content);
            }

            if !tool_calls.is_empty() {
                builder.tool_calls(tool_calls);
            }

            if let Ok(msg) = builder.build() {
                messages.push(msg.into());
            }
        }

        // Add tool result messages
        for (tool_use_id, result) in &self.tool_results {
            let content = if result.content.is_empty() {
                "<Tool returned an empty string>".to_string()
            } else {
                result.content.clone()
            };

            if let Ok(msg) = ChatCompletionRequestToolMessageArgs::default()
                .tool_call_id(tool_use_id.to_string())
                .content(content)
                .build()
            {
                messages.push(msg.into());
            }
        }

        messages
    }

    /// Add text content
    pub fn push_text(&mut self, text: impl Into<String>) {
        self.content.push(AgentMessageContent::Text(text.into()));
    }

    /// Add a tool use
    pub fn push_tool_use(&mut self, id: impl Into<Arc<str>>, name: String, arguments: String) {
        self.content.push(AgentMessageContent::ToolUse(ToolUse {
            id: id.into(),
            name,
            arguments,
        }));
    }

    /// Add a tool result
    pub fn add_tool_result(
        &mut self,
        tool_use_id: impl Into<Arc<str>>,
        tool_name: String,
        content: String,
        is_error: bool,
    ) {
        self.tool_results.insert(
            tool_use_id.into(),
            ToolResult {
                tool_name,
                content,
                is_error,
            },
        );
    }
}

/// Content within an agent message
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentMessageContent {
    Text(String),
    Thinking {
        text: String,
        signature: Option<String>,
    },
    ToolUse(ToolUse),
}

/// A tool use request from the model
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolUse {
    pub id: Arc<str>,
    pub name: String,
    pub arguments: String,
}

/// Result from a tool execution
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolResult {
    pub tool_name: String,
    pub content: String,
    pub is_error: bool,
}

/// A thread of messages (conversation history)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Thread {
    pub id: String,
    pub messages: Vec<Message>,
    #[serde(default)]
    pub updated_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl Thread {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            messages: Vec::new(),
            updated_at: Some(chrono::Utc::now()),
        }
    }

    /// Build request messages from thread history.
    /// System prompt is provided separately (can be swapped without affecting history).
    pub fn to_request_messages(
        &self,
        system_prompt: &str,
    ) -> Vec<ChatCompletionRequestMessage> {
        let mut messages = Vec::new();

        // Add system prompt
        if let Ok(msg) = ChatCompletionRequestSystemMessageArgs::default()
            .content(system_prompt)
            .build()
        {
            messages.push(msg.into());
        }

        // Add all messages from history
        for message in &self.messages {
            messages.extend(message.to_request());
        }

        messages
    }

    /// Add a user message
    pub fn push_user(&mut self, content: impl Into<String>) {
        self.messages.push(Message::User(UserMessage::new(content)));
        self.updated_at = Some(chrono::Utc::now());
    }

    /// Add an agent message
    pub fn push_agent(&mut self, message: AgentMessage) {
        self.messages.push(Message::Agent(message));
        self.updated_at = Some(chrono::Utc::now());
    }

    /// Add a resume marker
    pub fn push_resume(&mut self) {
        self.messages.push(Message::Resume);
        self.updated_at = Some(chrono::Utc::now());
    }

    /// Get the pending agent message (last message if it's an Agent message)
    pub fn pending_agent_mut(&mut self) -> Option<&mut AgentMessage> {
        match self.messages.last_mut() {
            Some(Message::Agent(msg)) => Some(msg),
            _ => None,
        }
    }

    /// Start a new agent message
    pub fn start_agent_message(&mut self) {
        self.messages.push(Message::Agent(AgentMessage::default()));
        self.updated_at = Some(chrono::Utc::now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_conversation() {
        let mut thread = Thread::new("test-session");

        thread.push_user("Hello");
        thread.start_agent_message();
        if let Some(agent) = thread.pending_agent_mut() {
            agent.push_text("Hi there!");
        }

        let messages = thread.to_request_messages("You are a helpful assistant.");
        assert_eq!(messages.len(), 3); // system, user, assistant
    }

    #[test]
    fn test_tool_use_conversation() {
        let mut thread = Thread::new("test-session");

        thread.push_user("What files are here?");
        thread.start_agent_message();

        if let Some(agent) = thread.pending_agent_mut() {
            agent.push_tool_use("call_123", "list_directory".to_string(), r#"{"path":"."}"#.to_string());
            agent.add_tool_result("call_123", "list_directory".to_string(), "file1.txt\nfile2.txt".to_string(), false);
        }

        let messages = thread.to_request_messages("You are a helpful assistant.");
        // system, user, assistant (with tool_call), tool result
        assert_eq!(messages.len(), 4);
    }
}
