use std::sync::mpsc;

use crate::model::{DungeonGraph, SpatialLayout, Theme};
use crate::render::recording::{RecordingRenderer, RenderCommand};
use crate::render::themed::RenderOptions;

/// A render cache that builds on a background thread, showing a spinner while loading.
pub struct BackgroundRenderCache {
    /// Completed render commands.
    commands: Option<Vec<RenderCommand>>,
    /// Hash of inputs that produced the current commands.
    current_hash: u64,
    /// Pending background render job.
    pending: Option<PendingRender>,
}

struct PendingRender {
    rx: mpsc::Receiver<Vec<RenderCommand>>,
    hash: u64,
    label: String,
}

impl Default for BackgroundRenderCache {
    fn default() -> Self {
        Self {
            commands: None,
            current_hash: 0,
            pending: None,
        }
    }
}

impl BackgroundRenderCache {
    /// Ensure the cache is up-to-date for the given hash.
    /// If a rebuild is needed, spawns a background thread and returns false.
    /// Returns true if the cache is ready to use.
    pub fn ensure(
        &mut self,
        hash: u64,
        graph: &DungeonGraph,
        layout: &SpatialLayout,
        theme: &Theme,
        options: RenderOptions,
        label: &str,
    ) -> bool {
        // Poll pending job
        if let Some(pending) = &self.pending {
            if let Ok(commands) = pending.rx.try_recv() {
                let h = pending.hash;
                self.pending = None;
                self.commands = Some(commands);
                self.current_hash = h;
            }
        }

        // Cache is current
        if self.commands.is_some() && self.current_hash == hash {
            return true;
        }

        // Already building for this hash
        if self.pending.as_ref().is_some_and(|p| p.hash == hash) {
            return false;
        }

        // Spawn background render
        let (tx, rx) = mpsc::channel();
        let graph = graph.clone();
        let layout = layout.clone();
        let theme = theme.clone();
        std::thread::spawn(move || {
            let mut recorder = RecordingRenderer::new();
            crate::render::themed::render_themed(
                &mut recorder,
                &graph,
                &layout,
                &theme,
                &options,
            );
            let _ = tx.send(recorder.commands);
        });
        self.pending = Some(PendingRender { rx, hash, label: label.to_string() });
        false
    }

    /// Generic version: caller provides a closure that produces render commands.
    /// The closure is sent to a background thread, so captured data must be 'static + Send.
    pub fn ensure_with<F>(
        &mut self,
        hash: u64,
        label: &str,
        build: F,
    ) -> bool
    where
        F: FnOnce() -> Vec<RenderCommand> + Send + 'static,
    {
        // Poll pending job
        if let Some(pending) = &self.pending {
            if let Ok(commands) = pending.rx.try_recv() {
                let h = pending.hash;
                self.pending = None;
                self.commands = Some(commands);
                self.current_hash = h;
            }
        }

        if self.commands.is_some() && self.current_hash == hash {
            return true;
        }

        if self.pending.as_ref().is_some_and(|p| p.hash == hash) {
            return false;
        }

        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let commands = build();
            let _ = tx.send(commands);
        });
        self.pending = Some(PendingRender { rx, hash, label: label.to_string() });
        false
    }

    /// Get the cached commands (if ready).
    pub fn commands(&self) -> Option<&[RenderCommand]> {
        self.commands.as_deref()
    }

    /// Poll for completion without triggering new builds.
    pub fn poll(&mut self) {
        if let Some(pending) = &self.pending {
            if let Ok(commands) = pending.rx.try_recv() {
                let h = pending.hash;
                self.pending = None;
                self.commands = Some(commands);
                self.current_hash = h;
            }
        }
    }

    /// Check if the cache is current for the given hash.
    pub fn is_current(&self, hash: u64) -> bool {
        self.commands.is_some() && self.current_hash == hash
    }

    /// Get the label of the in-progress build, if any.
    pub fn pending_label(&self) -> Option<&str> {
        self.pending.as_ref().map(|p| p.label.as_str())
    }

}
