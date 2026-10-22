//! MCP over Streamable HTTP, the small subset Claude Code needs: JSON-RPC requests
//! POSTed to `/mcp`, each answered with a single JSON response. No sessions and no
//! server-initiated messages, so the optional SSE stream (GET) is refused with 405.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use serde_json::{json, Value};

use super::{LinkStats, McpCall, ToolOutput};

/// Newest first; an unknown version from the client gets the first.
const PROTOCOL_VERSIONS: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];
/// How long a call may wait for the UI thread before Claude is told the app is stuck.
const CALL_TIMEOUT: Duration = Duration::from_secs(30);
/// An idle keep-alive connection is closed after this long.
const IDLE_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_HEADER_BYTES: usize = 64 * 1024;
const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;

const INSTRUCTIONS: &str = "\
Dungeon Mapper is open on the user's screen: these tools read and edit the map they \
have open, and every edit shows up live. Edits are undoable in the app with Ctrl+Z.

- Start with get_overview (rooms, connections, floors) and get_selection (what the \
user has clicked on; \"this room\" usually means the selection).
- A campaign has several maps; tools work on the open one. switch_map opens another \
(the user sees it change).
- Rooms are referred to by id or by label (case-insensitive, must be unique).
- One grid square is 5 feet. Room sizes are in grid squares.
- Decor positions are room-local grid units from the room's top-left corner; \
get_room gives the room's width and height.
- Floors are integers (0 is the ground floor). A room can span a range of floors.
- A room's description, read-aloud text and secrets live in its note (Markdown, \
Obsidian-style [[wikilinks]]). Use read_note/write_note for them.
- New rooms are placed next to the room they connect to; the existing layout is never \
re-solved, so placing rooms is cheap but positions are approximate.
- render_map returns a picture of the map when you need to see the layout.";

pub(super) struct Server {
    pub token: String,
    pub shutdown: Arc<AtomicBool>,
    pub calls: mpsc::Sender<McpCall>,
    pub ctx: egui::Context,
    pub stats: Arc<LinkStats>,
}

struct Request {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }
}

struct Response {
    status: u16,
    body: Option<String>,
}

impl Response {
    fn empty(status: u16) -> Self {
        Self { status, body: None }
    }

    fn json(value: &Value) -> Self {
        Self { status: 200, body: Some(value.to_string()) }
    }
}

impl Server {
    pub fn run(self, listener: TcpListener) {
        let server = Arc::new(self);
        listener.set_nonblocking(true).ok();
        while !server.shutdown.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((stream, _)) => {
                    let server = server.clone();
                    let _ = std::thread::Builder::new()
                        .name("claude-link-conn".into())
                        .spawn(move || server.serve_connection(stream));
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(_) => std::thread::sleep(Duration::from_millis(50)),
            }
        }
    }

    fn serve_connection(&self, stream: TcpStream) {
        stream.set_nonblocking(false).ok();
        stream.set_read_timeout(Some(IDLE_TIMEOUT)).ok();
        let Ok(write_half) = stream.try_clone() else { return };
        let mut writer = write_half;
        let mut reader = BufReader::new(stream);
        // Keep-alive: serve requests until the client closes or goes quiet.
        while let Ok(Some(request)) = read_request(&mut reader) {
            let response = if self.shutdown.load(Ordering::SeqCst) {
                Response::empty(503)
            } else {
                self.handle(&request)
            };
            if write_response(&mut writer, &response).is_err() || response.status == 503 {
                break;
            }
        }
    }

    fn handle(&self, request: &Request) -> Response {
        if request.path.split('?').next() != Some("/mcp") {
            return Response::empty(404);
        }
        // A web page can't reach us by DNS rebinding without the token, but turn away
        // browser origins outright anyway.
        if let Some(origin) = request.header("origin") {
            let local = ["http://127.0.0.1", "http://localhost"].iter().any(|o| origin.starts_with(o));
            if !local {
                return Response::empty(403);
            }
        }
        if request.header("authorization") != Some(format!("Bearer {}", self.token).as_str()) {
            return Response::empty(401);
        }
        match request.method.as_str() {
            "POST" => {}
            _ => return Response::empty(405),
        }
        let Ok(message) = serde_json::from_slice::<Value>(&request.body) else {
            return Response::json(&rpc_error(Value::Null, -32700, "Parse error"));
        };
        match message {
            Value::Array(batch) => {
                let replies: Vec<Value> = batch.iter().filter_map(|m| self.handle_message(m)).collect();
                if replies.is_empty() { Response::empty(202) } else { Response::json(&Value::Array(replies)) }
            }
            m => match self.handle_message(&m) {
                Some(reply) => Response::json(&reply),
                None => Response::empty(202),
            },
        }
    }

    /// Answer one JSON-RPC message; notifications and responses get no reply.
    fn handle_message(&self, message: &Value) -> Option<Value> {
        let id = message.get("id")?.clone();
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            return None;
        };
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        let result = match method {
            "initialize" => {
                let asked = params.get("protocolVersion").and_then(Value::as_str);
                let version = PROTOCOL_VERSIONS
                    .iter()
                    .find(|v| Some(**v) == asked)
                    .unwrap_or(&PROTOCOL_VERSIONS[0]);
                json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": { "name": "dungeon-mapper", "version": env!("CARGO_PKG_VERSION") },
                    "instructions": INSTRUCTIONS,
                })
            }
            "ping" => json!({}),
            "tools/list" => json!({ "tools": super::tools::definitions() }),
            "tools/call" => {
                let Some(name) = params.get("name").and_then(Value::as_str) else {
                    return Some(rpc_error(id, -32602, "tools/call needs a tool name"));
                };
                let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
                let output = self.call_tool(name, args);
                json!({ "content": output.content, "isError": output.is_error })
            }
            _ => return Some(rpc_error(id, -32601, &format!("Method not found: {}", method))),
        };
        Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
    }

    /// Hand a call to the UI thread and wait for its answer.
    fn call_tool(&self, name: &str, args: Value) -> ToolOutput {
        self.stats.calls.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut last) = self.stats.last_tool.lock() {
            *last = Some(name.to_string());
        }
        let (reply, answer) = mpsc::channel();
        let call = McpCall { tool: name.to_string(), args, reply };
        if self.calls.send(call).is_err() {
            return ToolOutput::error("Dungeon Mapper is shutting down the Claude link.");
        }
        self.ctx.request_repaint();
        answer.recv_timeout(CALL_TIMEOUT).unwrap_or_else(|_| {
            ToolOutput::error("Dungeon Mapper didn't answer in time (is a dialog open, or the app busy?).")
        })
    }
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// Read one HTTP/1.1 request. `Ok(None)` when the client closed the connection.
fn read_request(reader: &mut impl BufRead) -> std::io::Result<Option<Request>> {
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        return Ok(None);
    }
    let mut parts = line.split_whitespace();
    let (Some(method), Some(path)) = (parts.next(), parts.next()) else {
        return Err(std::io::Error::other("malformed request line"));
    };
    let (method, path) = (method.to_string(), path.to_string());

    let mut headers = Vec::new();
    let mut header_bytes = 0;
    loop {
        line.clear();
        let n = reader.read_line(&mut line)?;
        header_bytes += n;
        if n == 0 || header_bytes > MAX_HEADER_BYTES {
            return Err(std::io::Error::other("bad headers"));
        }
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            break;
        }
        if let Some((k, v)) = trimmed.split_once(':') {
            headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
        }
    }

    let length: usize = headers
        .iter()
        .find(|(k, _)| k == "content-length")
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    if length > MAX_BODY_BYTES {
        return Err(std::io::Error::other("body too large"));
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    Ok(Some(Request { method, path, headers, body }))
}

fn write_response(writer: &mut impl Write, response: &Response) -> std::io::Result<()> {
    let reason = match response.status {
        200 => "OK",
        202 => "Accepted",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Service Unavailable",
    };
    let body = response.body.as_deref().unwrap_or("");
    let mut head = format!("HTTP/1.1 {} {}\r\nContent-Length: {}\r\n", response.status, reason, body.len());
    if response.body.is_some() {
        head.push_str("Content-Type: application/json\r\n");
    }
    if response.status == 405 {
        head.push_str("Allow: POST\r\n");
    }
    head.push_str("\r\n");
    writer.write_all(head.as_bytes())?;
    writer.write_all(body.as_bytes())?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server() -> (Server, mpsc::Receiver<McpCall>) {
        let (tx, rx) = mpsc::channel();
        let server = Server {
            token: "secret".into(),
            shutdown: Arc::new(AtomicBool::new(false)),
            calls: tx,
            ctx: egui::Context::default(),
            stats: Arc::default(),
        };
        (server, rx)
    }

    fn post(body: &str, token: &str) -> Request {
        Request {
            method: "POST".into(),
            path: "/mcp".into(),
            headers: vec![("authorization".into(), format!("Bearer {}", token))],
            body: body.as_bytes().to_vec(),
        }
    }

    #[test]
    fn parses_a_request_and_keeps_the_next_one_buffered() {
        let raw = "POST /mcp HTTP/1.1\r\nHost: x\r\nContent-Length: 2\r\nAuthorization: Bearer t\r\n\r\n{}\
                   GET /mcp HTTP/1.1\r\n\r\n";
        let mut reader = BufReader::new(raw.as_bytes());
        let first = read_request(&mut reader).unwrap().unwrap();
        assert_eq!((first.method.as_str(), first.path.as_str()), ("POST", "/mcp"));
        assert_eq!(first.header("authorization"), Some("Bearer t"));
        assert_eq!(first.body, b"{}");
        let second = read_request(&mut reader).unwrap().unwrap();
        assert_eq!(second.method, "GET");
        assert!(read_request(&mut reader).unwrap().is_none());
    }

    #[test]
    fn refuses_without_the_token() {
        let (server, _rx) = server();
        let r = server.handle(&post(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#, "wrong"));
        assert_eq!(r.status, 401);
    }

    #[test]
    fn initializes_and_lists_tools() {
        let (server, _rx) = server();
        let r = server.handle(&post(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#,
            "secret",
        ));
        let v: Value = serde_json::from_str(r.body.as_deref().unwrap()).unwrap();
        assert_eq!(v["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(v["result"]["serverInfo"]["name"], "dungeon-mapper");

        let r = server.handle(&post(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#, "secret"));
        assert_eq!(r.status, 202);

        let r = server.handle(&post(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#, "secret"));
        let v: Value = serde_json::from_str(r.body.as_deref().unwrap()).unwrap();
        assert!(v["result"]["tools"].as_array().unwrap().iter().any(|t| t["name"] == "get_overview"));
    }

    #[test]
    fn serves_requests_over_a_keep_alive_socket() {
        use std::io::Read;
        let (server, _rx) = server();
        let shutdown = server.shutdown.clone();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let thread = std::thread::spawn(move || server.run(listener));

        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        for id in 1..=2 {
            let body = format!(r#"{{"jsonrpc":"2.0","id":{},"method":"ping"}}"#, id);
            write!(stream, "POST /mcp HTTP/1.1\r\nAuthorization: Bearer secret\r\nContent-Length: {}\r\n\r\n{}", body.len(), body).unwrap();
            let mut status = String::new();
            reader.read_line(&mut status).unwrap();
            assert!(status.starts_with("HTTP/1.1 200"), "{}", status);
            let mut length = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line.trim().is_empty() {
                    break;
                }
                if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = v.trim().parse().unwrap();
                }
            }
            let mut reply = vec![0; length];
            reader.read_exact(&mut reply).unwrap();
            let v: Value = serde_json::from_slice(&reply).unwrap();
            assert_eq!(v["id"], id);
        }
        shutdown.store(true, Ordering::SeqCst);
        thread.join().unwrap();
    }

    #[test]
    fn tool_calls_are_answered_by_the_ui_thread() {
        let (server, rx) = server();
        let ui = std::thread::spawn(move || {
            let call = rx.recv().unwrap();
            assert_eq!(call.tool, "get_overview");
            call.reply.send(ToolOutput::json(&json!({ "rooms": [] }))).unwrap();
        });
        let r = server.handle(&post(
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"get_overview","arguments":{}}}"#,
            "secret",
        ));
        ui.join().unwrap();
        let v: Value = serde_json::from_str(r.body.as_deref().unwrap()).unwrap();
        assert_eq!(v["result"]["isError"], false);
        assert!(v["result"]["content"][0]["text"].as_str().unwrap().contains("rooms"));
    }
}
