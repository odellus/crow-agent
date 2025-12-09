//! LLM Provider layer
//!
//! OpenAI-compatible provider that handles streaming chat completions.
//! Supports any API implementing the OpenAI chat completions spec.

mod client;
mod config;

pub use client::*;
pub use config::*;
