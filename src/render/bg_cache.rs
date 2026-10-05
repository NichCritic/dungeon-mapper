use std::sync::mpsc;

use crate::model::{DungeonGraph, SpatialLayout, Theme};
use crate::render::recording::{RecordingRenderer, RenderCommand, ReplayCache};
use crate::render::themed::RenderOptions;

/// Fingerprint of everything the cached map render reads from the layout, graph and
/// theme. Text is drawn as a live overlay, so labels and notes are left out. Views
/// with a live decor overlay pass `include_decor = false` so dragging decor doesn't
/// trigger rebuilds. Views hash their own extras (grid, floor filter, fog) on top.
pub fn map_render_hash(
    h: &mut impl std::hash::Hasher,
    layout: &SpatialLayout,
    graph: &DungeonGraph,
    theme: &Theme,
    include_decor: bool,
) {
    use std::hash::Hash;
    use crate::util::hash_serde;
    let rooms: std::collections::HashMap<&str, &crate::model::Room> =
        graph.rooms.iter().map(|r| (r.id.as_str(), r)).collect();
    layout.rooms.len().hash(h);
    for rl in &layout.rooms {
        rl.room_id.hash(h);
        (rl.x, rl.y, rl.width, rl.height).hash(h);
        rl.rotation.to_bits().hash(h);
        for wp in &rl.wall_openings {
            (wp.x, wp.y).hash(h);
        }
        let Some(room) = rooms.get(rl.room_id.as_str()) else { continue };
        hash_serde(h, &room.shape);
        hash_serde(h, &room.floor);
        hash_serde(h, &room.environment);
        hash_serde(h, &room.open_walls);
        hash_serde(h, &room.sections);
        // Bumped on every cell edit, so the cells themselves needn't be hashed
        room.cave_data.as_ref().map(|c| c.generation).hash(h);
        if include_decor {
            room.decor.len().hash(h);
            for d in &room.decor {
                hash_serde(h, &d.decor_type);
                hash_serde(h, &d.cover);
                for v in [d.x, d.y, d.rotation, d.scale_x, d.scale_y] {
                    v.to_bits().hash(h);
                }
            }
        }
    }
    layout.corridors.len().hash(h);
    for c in &layout.corridors {
        c.connection_id.hash(h);
        c.width.hash(h);
        hash_serde(h, &c.floor);
        for wp in &c.waypoints {
            (wp.x, wp.y).hash(h);
        }
    }
    graph.connections.len().hash(h);
    for e in &graph.connections {
        hash_serde(h, e);
    }
    hash_serde(h, theme);
}

/// What a view's cached map render is built from: its key, options and label. Each
/// view builds this in one place, used by both the view and the app's pre-warming.
pub struct CacheSpec {
    pub hash: u64,
    pub options: RenderOptions,
    pub label: &'static str,
}

/// A render cache that builds on a background thread, showing a spinner while loading.
pub struct BackgroundRenderCache {
    /// Completed render commands.
    commands: Option<Vec<RenderCommand>>,
    /// Hash of inputs that produced the current commands.
    current_hash: u64,
    /// Pending background render job.
    pending: Option<PendingRender>,
    /// Inputs whose render crashed: not retried (it would crash again) until they change.
    failed_hash: Option<u64>,
    /// Bumped whenever `commands` is replaced, so the replay cache knows to rebuild.
    generation: u64,
    replay: ReplayCache,
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
            failed_hash: None,
            generation: 0,
            replay: ReplayCache::default(),
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
        self.poll();

        // Cache is current
        if self.commands.is_some() && self.current_hash == hash {
            return true;
        }

        // Already building for this hash, or its build crashed
        if self.pending.as_ref().is_some_and(|p| p.hash == hash) || self.failed_hash == Some(hash) {
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

    /// [`Self::ensure`] for a view's [`CacheSpec`].
    pub fn ensure_spec(&mut self, spec: &CacheSpec, graph: &DungeonGraph, layout: &SpatialLayout, theme: &Theme) -> bool {
        self.ensure(spec.hash, graph, layout, theme, spec.options, spec.label)
    }

    /// Generic version: `prepare` is called only when a rebuild is actually needed (so its
    /// snapshot clones aren't paid every frame) and returns the closure that produces the
    /// render commands on a background thread.
    pub fn ensure_with<F>(
        &mut self,
        hash: u64,
        label: &str,
        prepare: impl FnOnce() -> F,
    ) -> bool
    where
        F: FnOnce() -> Vec<RenderCommand> + Send + 'static,
    {
        // Poll pending job
        self.poll();

        if self.commands.is_some() && self.current_hash == hash {
            return true;
        }

        if self.pending.as_ref().is_some_and(|p| p.hash == hash) || self.failed_hash == Some(hash) {
            return false;
        }

        let build = prepare();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let commands = build();
            let _ = tx.send(commands);
        });
        self.pending = Some(PendingRender { rx, hash, label: label.to_string() });
        false
    }

    /// Paint the cached commands (if ready) through the view transform.
    pub fn paint(&mut self, painter: &egui::Painter, transform: &crate::util::ViewTransform) {
        if let Some(commands) = &self.commands {
            self.replay.paint(painter, transform, commands, self.generation);
        }
    }

    /// Poll for completion without triggering new builds.
    pub fn poll(&mut self) {
        let Some(pending) = &self.pending else { return };
        match pending.rx.try_recv() {
            Ok(commands) => {
                self.current_hash = pending.hash;
                self.pending = None;
                self.commands = Some(commands);
                self.generation += 1;
            }
            // The build thread panicked: stop waiting on it rather than "loading" forever
            Err(mpsc::TryRecvError::Disconnected) => {
                eprintln!("Background render '{}' failed", pending.label);
                self.failed_hash = Some(pending.hash);
                self.pending = None;
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
    }

    /// Check if the cache is current for the given hash.
    pub fn is_current(&self, hash: u64) -> bool {
        self.commands.is_some() && self.current_hash == hash
    }

    /// Whether the build for these inputs crashed (and so won't finish).
    pub fn has_failed(&self, hash: u64) -> bool {
        self.failed_hash == Some(hash)
    }

    /// Get the label of the in-progress build, if any.
    pub fn pending_label(&self) -> Option<&str> {
        self.pending.as_ref().map(|p| p.label.as_str())
    }

}
