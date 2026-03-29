use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use tungstenite::protocol::Message;
use tungstenite::{accept, WebSocket};

const CLIENT_HTML: &str = include_str!("client.html");

pub fn run_server(
    port: u16,
    shutdown: Arc<AtomicBool>,
    update_rx: mpsc::Receiver<Vec<u8>>,
    connected_clients: Arc<AtomicUsize>,
) {
    let listener = match TcpListener::bind(format!("0.0.0.0:{}", port)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("Failed to bind server on port {}: {}", port, e);
            return;
        }
    };
    listener.set_nonblocking(true).ok();

    let mut websocket_clients: Vec<WebSocket<TcpStream>> = Vec::new();
    let mut latest_png: Option<Vec<u8>> = None;

    while !shutdown.load(Ordering::SeqCst) {
        // Accept new connections (non-blocking)
        match listener.accept() {
            Ok((stream, _addr)) => {
                stream.set_nonblocking(false).ok();
                stream.set_read_timeout(Some(Duration::from_millis(100))).ok();

                // Peek at the request to decide HTTP vs WebSocket
                let mut peek_buf = [0u8; 4096];
                if let Ok(n) = stream.peek(&mut peek_buf) {
                    let request = String::from_utf8_lossy(&peek_buf[..n]);
                    if request.contains("Upgrade: websocket") || request.contains("upgrade: websocket") {
                        // WebSocket upgrade
                        match accept(stream) {
                            Ok(ws) => {
                                // Send latest PNG if available
                                let mut ws = ws;
                                if let Some(png) = &latest_png {
                                    let _ = ws.send(Message::Binary(png.clone().into()));
                                }
                                ws.get_ref().set_nonblocking(true).ok();
                                websocket_clients.push(ws);
                                connected_clients.store(websocket_clients.len(), Ordering::Relaxed);
                            }
                            Err(e) => {
                                eprintln!("WebSocket accept error: {}", e);
                            }
                        }
                    } else {
                        // Regular HTTP request — serve the HTML page
                        serve_html(stream);
                    }
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                // No pending connection, that's fine
            }
            Err(e) => {
                eprintln!("Accept error: {}", e);
            }
        }

        // Check for PNG updates from the main thread
        match update_rx.recv_timeout(Duration::from_millis(50)) {
            Ok(png_bytes) => {
                if png_bytes.is_empty() {
                    // Shutdown signal
                    continue;
                }
                latest_png = Some(png_bytes.clone());

                // Broadcast to all WebSocket clients
                let mut failed = Vec::new();
                for (i, ws) in websocket_clients.iter_mut().enumerate() {
                    ws.get_ref().set_nonblocking(false).ok();
                    ws.get_ref().set_write_timeout(Some(Duration::from_secs(2))).ok();
                    if ws.send(Message::Binary(png_bytes.clone().into())).is_err() {
                        failed.push(i);
                    }
                    ws.get_ref().set_nonblocking(true).ok();
                }

                // Remove disconnected clients (in reverse order to preserve indices)
                for i in failed.into_iter().rev() {
                    let _ = websocket_clients.remove(i);
                }
                connected_clients.store(websocket_clients.len(), Ordering::Relaxed);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }

        // Cleanup disconnected clients by trying to flush
        let mut disconnected = Vec::new();
        for (i, ws) in websocket_clients.iter_mut().enumerate() {
            // Try reading to detect disconnects
            match ws.read() {
                Ok(Message::Close(_)) | Err(_) => {
                    // Check if it's just a WouldBlock (non-blocking socket)
                    // tungstenite wraps WouldBlock in its own error
                    disconnected.push(i);
                }
                Ok(Message::Ping(data)) => {
                    let _ = ws.send(Message::Pong(data));
                }
                _ => {}
            }
        }
        // Only remove truly disconnected ones (skip WouldBlock)
        // Since we set non-blocking, read errors are expected for idle clients
        // We rely on send failures above for cleanup instead
    }

    // Close all clients
    for mut ws in websocket_clients {
        let _ = ws.close(None);
    }
}

fn serve_html(mut stream: TcpStream) {
    // Read the full request (we already peeked, now consume it)
    let mut buf = [0u8; 4096];
    let _ = stream.read(&mut buf);

    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        CLIENT_HTML.len(),
        CLIENT_HTML,
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}
