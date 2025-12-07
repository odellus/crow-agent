//! LSP client implementation
//!
//! Spawns a language server process, communicates via JSON-RPC over stdio,
//! and collects diagnostics via textDocument/publishDiagnostics notifications.

use lsp_types::{
    notification::{Initialized, Notification, PublishDiagnostics},
    request::{Initialize, Request, Shutdown},
    ClientCapabilities, ClientInfo, DiagnosticClientCapabilities, InitializeParams,
    InitializeResult, InitializedParams, PublishDiagnosticsClientCapabilities,
    PublishDiagnosticsParams, TextDocumentClientCapabilities, Uri, WorkspaceFolder,
};
use parking_lot::Mutex;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Read, Write},
    path::Path,
    process::{Child, Command, Stdio},
    str::FromStr,
    sync::{
        atomic::{AtomicI32, Ordering},
        Arc,
    },
};

const JSON_RPC_VERSION: &str = "2.0";
const CONTENT_LEN_HEADER: &str = "Content-Length: ";

#[derive(Debug, thiserror::Error)]
pub enum LspError {
    #[error("Failed to spawn language server: {0}")]
    Spawn(#[from] std::io::Error),
    #[error("Failed to serialize/deserialize: {0}")]
    Json(#[from] serde_json::Error),
    #[error("LSP error {code}: {message}")]
    Protocol { code: i64, message: String },
    #[error("Server not initialized")]
    NotInitialized,
    #[error("Invalid path: {0}")]
    InvalidPath(String),
    #[error("Request timeout")]
    Timeout,
    #[error("Server shutdown")]
    Shutdown,
}

/// Convert a file path to a file:// URI string
fn path_to_uri(path: &Path) -> Result<Uri, LspError> {
    let abs_path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };

    let path_str = abs_path
        .to_str()
        .ok_or_else(|| LspError::InvalidPath("Path contains invalid UTF-8".to_string()))?;

    // Construct file:// URI
    let uri_string = if cfg!(windows) {
        format!("file:///{}", path_str.replace('\\', "/"))
    } else {
        format!("file://{}", path_str)
    };

    Uri::from_str(&uri_string).map_err(|e| LspError::InvalidPath(format!("Invalid URI: {}", e)))
}

/// Extract path from a file:// URI
fn uri_to_path_string(uri: &Uri) -> String {
    let s = uri.as_str();
    if s.starts_with("file://") {
        let path = &s[7..];
        // Handle Windows paths like file:///C:/...
        if path.starts_with('/') && path.len() > 2 && path.chars().nth(2) == Some(':') {
            path[1..].to_string()
        } else {
            path.to_string()
        }
    } else {
        s.to_string()
    }
}

/// JSON-RPC request message
#[derive(Serialize)]
struct JsonRpcRequest<T> {
    jsonrpc: &'static str,
    id: i32,
    method: &'static str,
    params: T,
}

/// JSON-RPC notification message (no id)
#[derive(Serialize)]
struct JsonRpcNotification<T> {
    jsonrpc: &'static str,
    method: &'static str,
    params: T,
}

/// JSON-RPC error
#[derive(Deserialize)]
struct JsonRpcError {
    code: i64,
    message: String,
}

/// Incoming message (could be response or notification)
#[derive(Deserialize)]
struct IncomingMessage {
    #[allow(dead_code)]
    jsonrpc: String,
    id: Option<i32>,
    method: Option<String>,
    params: Option<Value>,
    result: Option<Value>,
    error: Option<JsonRpcError>,
}

/// LSP client that manages a language server process
pub struct LspClient {
    process: Child,
    next_id: AtomicI32,
    root_uri: Uri,
    diagnostics: Arc<Mutex<HashMap<String, Vec<lsp_types::Diagnostic>>>>,
    initialized: bool,
}

impl LspClient {
    /// Start a new language server process
    pub fn new(command: &str, args: &[&str], root_path: &Path) -> Result<Self, LspError> {
        let root_uri = path_to_uri(root_path)?;

        let process = Command::new(command)
            .args(args)
            .current_dir(root_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        Ok(Self {
            process,
            next_id: AtomicI32::new(1),
            root_uri,
            diagnostics: Arc::new(Mutex::new(HashMap::new())),
            initialized: false,
        })
    }

    /// Initialize the language server
    pub fn initialize(&mut self) -> Result<InitializeResult, LspError> {
        #[allow(deprecated)]
        let params = InitializeParams {
            process_id: Some(std::process::id()),
            root_path: None,
            root_uri: Some(self.root_uri.clone()),
            initialization_options: None,
            capabilities: ClientCapabilities {
                text_document: Some(TextDocumentClientCapabilities {
                    publish_diagnostics: Some(PublishDiagnosticsClientCapabilities {
                        related_information: Some(true),
                        version_support: Some(true),
                        tag_support: None,
                        code_description_support: Some(true),
                        data_support: Some(true),
                    }),
                    diagnostic: Some(DiagnosticClientCapabilities {
                        dynamic_registration: Some(false),
                        related_document_support: Some(false),
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            },
            trace: None,
            workspace_folders: Some(vec![WorkspaceFolder {
                uri: self.root_uri.clone(),
                name: "root".to_string(),
            }]),
            client_info: Some(ClientInfo {
                name: "crow-agent".to_string(),
                version: Some(env!("CARGO_PKG_VERSION").to_string()),
            }),
            locale: None,
            work_done_progress_params: Default::default(),
        };

        let result: InitializeResult = self.send_request::<Initialize>(params)?;

        // Send initialized notification
        self.send_notification::<Initialized>(InitializedParams {})?;
        self.initialized = true;

        Ok(result)
    }

    /// Send a request and wait for response
    fn send_request<R: Request>(&mut self, params: R::Params) -> Result<R::Result, LspError>
    where
        R::Params: Serialize,
        R::Result: DeserializeOwned,
    {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);

        let request = JsonRpcRequest {
            jsonrpc: JSON_RPC_VERSION,
            id,
            method: R::METHOD,
            params,
        };

        self.write_message(&request)?;

        // Read response, handling any notifications that come in
        loop {
            let msg = self.read_message()?;

            // Handle notifications
            if let Some(method) = &msg.method {
                self.handle_notification(method, msg.params)?;
                continue;
            }

            // Check if this is our response
            if msg.id == Some(id) {
                if let Some(error) = msg.error {
                    return Err(LspError::Protocol {
                        code: error.code,
                        message: error.message,
                    });
                }

                let result = msg.result.unwrap_or(Value::Null);
                return Ok(serde_json::from_value(result)?);
            }
        }
    }

    /// Send a notification (no response expected)
    fn send_notification<N: Notification>(&mut self, params: N::Params) -> Result<(), LspError>
    where
        N::Params: Serialize,
    {
        let notification = JsonRpcNotification {
            jsonrpc: JSON_RPC_VERSION,
            method: N::METHOD,
            params,
        };

        self.write_message(&notification)
    }

    /// Write a JSON-RPC message to stdin
    fn write_message<T: Serialize>(&mut self, message: &T) -> Result<(), LspError> {
        let stdin =
            self.process.stdin.as_mut().ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::BrokenPipe, "stdin closed")
            })?;

        let body = serde_json::to_string(message)?;
        let header = format!("{}{}\r\n\r\n", CONTENT_LEN_HEADER, body.len());

        stdin.write_all(header.as_bytes())?;
        stdin.write_all(body.as_bytes())?;
        stdin.flush()?;

        Ok(())
    }

    /// Read a JSON-RPC message from stdout
    fn read_message(&mut self) -> Result<IncomingMessage, LspError> {
        let stdout =
            self.process.stdout.as_mut().ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::BrokenPipe, "stdout closed")
            })?;

        let mut reader = BufReader::new(stdout);
        let mut header = String::new();
        let mut content_length: Option<usize> = None;

        // Read headers
        loop {
            header.clear();
            reader.read_line(&mut header)?;

            if header == "\r\n" {
                break;
            }

            if header.starts_with(CONTENT_LEN_HEADER) {
                content_length = Some(header[CONTENT_LEN_HEADER.len()..].trim().parse().map_err(
                    |_| {
                        std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            "Invalid content length",
                        )
                    },
                )?);
            }
        }

        let content_length = content_length.ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, "Missing Content-Length")
        })?;

        // Read body
        let mut body = vec![0u8; content_length];
        reader.read_exact(&mut body)?;

        let msg: IncomingMessage = serde_json::from_slice(&body)?;
        Ok(msg)
    }

    /// Handle incoming notification
    fn handle_notification(&self, method: &str, params: Option<Value>) -> Result<(), LspError> {
        if method == PublishDiagnostics::METHOD {
            if let Some(params) = params {
                let diag_params: PublishDiagnosticsParams = serde_json::from_value(params)?;
                let path = uri_to_path_string(&diag_params.uri);
                let mut diagnostics = self.diagnostics.lock();
                diagnostics.insert(path, diag_params.diagnostics);
            }
        }
        // Ignore other notifications
        Ok(())
    }

    /// Open a text document
    pub fn open_document(
        &mut self,
        path: &Path,
        text: &str,
        language_id: &str,
    ) -> Result<(), LspError> {
        if !self.initialized {
            return Err(LspError::NotInitialized);
        }

        let uri = path_to_uri(path)?;

        self.send_notification::<lsp_types::notification::DidOpenTextDocument>(
            lsp_types::DidOpenTextDocumentParams {
                text_document: lsp_types::TextDocumentItem {
                    uri,
                    language_id: language_id.to_string(),
                    version: 1,
                    text: text.to_string(),
                },
            },
        )
    }

    /// Get current diagnostics for a path
    pub fn get_diagnostics(&self, path: &Path) -> Vec<lsp_types::Diagnostic> {
        let path_str = path.to_string_lossy().to_string();
        self.diagnostics
            .lock()
            .get(&path_str)
            .cloned()
            .unwrap_or_default()
    }

    /// Get all diagnostics (keyed by path string)
    pub fn get_all_diagnostics(&self) -> HashMap<String, Vec<lsp_types::Diagnostic>> {
        self.diagnostics.lock().clone()
    }

    /// Shutdown the language server gracefully
    pub fn shutdown(&mut self) -> Result<(), LspError> {
        if !self.initialized {
            return Ok(());
        }

        // Send shutdown request
        let _: () = self.send_request::<Shutdown>(())?;

        // Send exit notification
        self.send_notification::<lsp_types::notification::Exit>(())?;

        self.initialized = false;
        Ok(())
    }
}

impl Drop for LspClient {
    fn drop(&mut self) {
        // Try graceful shutdown, then kill if needed
        let _ = self.shutdown();
        let _ = self.process.kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_path_to_uri() {
        let path = PathBuf::from("/home/user/project");
        let uri = path_to_uri(&path).unwrap();
        assert!(uri.as_str().starts_with("file://"));
        assert!(uri.as_str().contains("/home/user/project"));
    }

    #[test]
    #[ignore] // Requires rust-analyzer to be installed
    fn test_rust_analyzer() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let mut client = LspClient::new("rust-analyzer", &[], &root).unwrap();

        let result = client.initialize().unwrap();
        assert!(result.capabilities.text_document_sync.is_some());

        client.shutdown().unwrap();
    }
}
