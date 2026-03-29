pub mod ws;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;

pub struct PresentationServer {
    shutdown: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    update_tx: mpsc::Sender<Vec<u8>>,
    pub connected_clients: Arc<AtomicUsize>,
    pub port: u16,
}

impl PresentationServer {
    pub fn start(port: u16) -> Result<Self, String> {
        let shutdown = Arc::new(AtomicBool::new(false));
        let connected_clients = Arc::new(AtomicUsize::new(0));
        let (update_tx, update_rx) = mpsc::channel::<Vec<u8>>();

        let shutdown_clone = shutdown.clone();
        let clients_clone = connected_clients.clone();

        let thread = std::thread::Builder::new()
            .name("presentation-server".into())
            .spawn(move || {
                ws::run_server(port, shutdown_clone, update_rx, clients_clone);
            })
            .map_err(|e| format!("Failed to spawn server thread: {}", e))?;

        Ok(Self {
            shutdown,
            thread: Some(thread),
            update_tx,
            connected_clients,
            port,
        })
    }

    pub fn stop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        // Send a dummy message to unblock the receiver
        let _ = self.update_tx.send(Vec::new());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }

    pub fn push_update(&self, png_bytes: Vec<u8>) {
        let _ = self.update_tx.send(png_bytes);
    }

    pub fn client_count(&self) -> usize {
        self.connected_clients.load(Ordering::Relaxed)
    }
}

impl Drop for PresentationServer {
    fn drop(&mut self) {
        self.stop();
    }
}
