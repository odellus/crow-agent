//! LSP client for getting diagnostics from language servers
//!
//! Based on Zed's LSP implementation but simplified for diagnostics-only use.

mod client;

pub use client::{LspClient, LspError};
