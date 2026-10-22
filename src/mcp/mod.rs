//! The Claude link: an MCP server inside the app, so Claude Code can read and edit
//! the open map while the DM watches.
//!
//! The server thread speaks MCP's Streamable HTTP transport on 127.0.0.1 behind a
//! bearer token. It never touches the map itself: each tool call is handed to the UI
//! thread as a [`McpCall`] and answered from there, so Claude's edits land on screen
//! the frame they're made and the undo history picks them up like any other edit.

mod encounters;
mod http;
mod layout;
mod lighting;
pub mod tools;

use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;

use serde::{Deserialize, Serialize};

const DEFAULT_PORT: u16 = 8737;

/// A tool call waiting for the UI thread.
pub struct McpCall {
    pub tool: String,
    pub args: serde_json::Value,
    pub reply: mpsc::Sender<ToolOutput>,
}

/// A tool's answer: MCP content blocks, and whether the call failed.
pub struct ToolOutput {
    pub content: Vec<serde_json::Value>,
    pub is_error: bool,
}

impl ToolOutput {
    pub fn json(value: &serde_json::Value) -> Self {
        let text = serde_json::to_string_pretty(value).unwrap_or_default();
        Self { content: vec![serde_json::json!({ "type": "text", "text": text })], is_error: false }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            content: vec![serde_json::json!({ "type": "text", "text": message.into() })],
            is_error: true,
        }
    }

    pub fn png(png: &[u8], caption: String) -> Self {
        use base64::Engine;
        let data = base64::engine::general_purpose::STANDARD.encode(png);
        Self {
            content: vec![
                serde_json::json!({ "type": "image", "data": data, "mimeType": "image/png" }),
                serde_json::json!({ "type": "text", "text": caption }),
            ],
            is_error: false,
        }
    }
}

/// Saved between runs, so `claude mcp add` only has to be done once.
#[derive(Clone, Serialize, Deserialize)]
struct LinkConfig {
    enabled: bool,
    port: u16,
    token: String,
}

impl Default for LinkConfig {
    fn default() -> Self {
        Self { enabled: false, port: DEFAULT_PORT, token: uuid::Uuid::new_v4().simple().to_string() }
    }
}

fn config_path() -> std::path::PathBuf {
    crate::io::cloud_sync::config_dir().join("claude_link.json")
}

/// What the menu shows about recent use.
#[derive(Default)]
pub struct LinkStats {
    pub calls: AtomicU64,
    pub last_tool: Mutex<Option<String>>,
}

struct Running {
    shutdown: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    calls: mpsc::Receiver<McpCall>,
}

impl Drop for Running {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// The app's end of the link: settings, the server while it runs, and its status.
pub struct ClaudeLink {
    config: LinkConfig,
    running: Option<Running>,
    /// Why the server couldn't start (e.g. port taken); cleared when settings change.
    pub error: Option<String>,
    pub stats: Arc<LinkStats>,
    /// What Claude has changed this session, newest last.
    pub activity: Vec<(std::time::Instant, String)>,
}

/// "12s", "3m", "2h": how long ago, for the activity list.
pub fn ago(d: std::time::Duration) -> String {
    match d.as_secs() {
        s if s < 60 => format!("{}s", s),
        s if s < 3600 => format!("{}m", s / 60),
        s => format!("{}h", s / 3600),
    }
}

impl ClaudeLink {
    pub fn load() -> Self {
        let config = std::fs::read_to_string(config_path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Self { config, running: None, error: None, stats: Arc::default(), activity: Vec::new() }
    }

    fn save(&self) {
        if let Ok(json) = serde_json::to_string_pretty(&self.config) {
            let _ = std::fs::write(config_path(), json);
        }
    }

    pub fn enabled(&self) -> bool {
        self.config.enabled
    }

    pub fn port(&self) -> u16 {
        self.config.port
    }

    pub fn is_running(&self) -> bool {
        self.running.is_some()
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.config.enabled = enabled;
        self.error = None;
        if !enabled {
            self.running = None;
        }
        self.save();
    }

    pub fn set_port(&mut self, port: u16) {
        if port != self.config.port {
            self.config.port = port;
            self.error = None;
            self.running = None;
            self.save();
        }
    }

    /// The command that registers this app with Claude Code (once per machine).
    pub fn setup_command(&self) -> String {
        format!(
            "claude mcp add --transport http --scope user dungeon-mapper http://127.0.0.1:{}/mcp --header \"Authorization: Bearer {}\"",
            self.config.port, self.config.token
        )
    }

    /// Start the server if it's switched on and not yet running. Needs the egui context
    /// so the server can wake the UI thread when a call arrives.
    pub fn ensure_running(&mut self, ctx: &egui::Context) {
        if !self.config.enabled || self.running.is_some() || self.error.is_some() {
            return;
        }
        let listener = match TcpListener::bind(("127.0.0.1", self.config.port)) {
            Ok(l) => l,
            Err(e) => {
                self.error = Some(format!("Couldn't listen on port {}: {}", self.config.port, e));
                return;
            }
        };
        let shutdown = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let server = http::Server {
            token: self.config.token.clone(),
            shutdown: shutdown.clone(),
            calls: tx,
            ctx: ctx.clone(),
            stats: self.stats.clone(),
        };
        match std::thread::Builder::new().name("claude-link".into()).spawn(move || server.run(listener)) {
            Ok(thread) => self.running = Some(Running { shutdown, thread: Some(thread), calls: rx }),
            Err(e) => self.error = Some(format!("Couldn't start the server thread: {}", e)),
        }
    }

    /// Tool calls that arrived since the last frame.
    pub fn take_calls(&self) -> Vec<McpCall> {
        self.running.as_ref().map(|r| r.calls.try_iter().collect()).unwrap_or_default()
    }
}
