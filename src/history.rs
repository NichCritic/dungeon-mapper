use std::time::{Duration, Instant};

use crate::model::Dungeon;

const MAX_UNDO_ENTRIES: usize = 200;
const COALESCE_WINDOW: Duration = Duration::from_millis(500);

pub struct UndoHistory {
    undo_stack: Vec<Dungeon>,
    redo_stack: Vec<Dungeon>,
    /// The last committed state (what we'd push to undo stack on the next change).
    committed: Dungeon,
    committed_hash: u64,
    /// Hash of the dungeon at the end of the previous frame (for detecting per-frame changes).
    prev_frame_hash: u64,
    /// True when state changed while a pointer button was held (drag in progress).
    drag_dirty: bool,
    /// True when non-pointer changes are pending commit (waiting for coalesce window).
    pending_change: bool,
    /// Time of the last per-frame state change (for coalescing).
    last_change_time: Instant,
}

impl UndoHistory {
    pub fn new(initial: &Dungeon) -> Self {
        let hash = Self::hash_dungeon(initial);
        Self {
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            committed: initial.clone(),
            committed_hash: hash,
            prev_frame_hash: hash,
            drag_dirty: false,
            pending_change: false,
            last_change_time: Instant::now(),
        }
    }

    /// Call at the end of each frame to detect and record state changes.
    ///
    /// `pointer_down`: true if any pointer/mouse button is currently held.
    pub fn track(&mut self, dungeon: &Dungeon, pointer_down: bool) {
        let hash = Self::hash_dungeon(dungeon);
        let now = Instant::now();

        // Check if pending non-pointer changes should be committed.
        // This fires when state has been stable (unchanged between frames) for the
        // coalesce window duration, but still differs from the last committed state.
        if self.pending_change
            && hash == self.prev_frame_hash
            && hash != self.committed_hash
        {
            if now.duration_since(self.last_change_time) >= COALESCE_WINDOW {
                self.commit(dungeon, hash);
                self.prev_frame_hash = hash;
                return;
            }
        }

        if hash == self.committed_hash {
            // No net change from committed state.
            self.drag_dirty = false;
            self.pending_change = false;
            self.prev_frame_hash = hash;
            return;
        }

        // State differs from committed.

        if pointer_down {
            // Drag in progress - don't commit yet.
            self.drag_dirty = true;
            self.prev_frame_hash = hash;
            return;
        }

        if self.drag_dirty {
            // Pointer just released after a drag - commit immediately.
            self.commit(dungeon, hash);
            self.drag_dirty = false;
            self.prev_frame_hash = hash;
            return;
        }

        // Non-pointer change (typing, keyboard shortcut, etc.)
        if hash != self.prev_frame_hash {
            // State actually changed this frame - reset the coalesce timer.
            self.last_change_time = now;
        }
        self.pending_change = true;
        self.prev_frame_hash = hash;
    }

    /// Commit the current pending changes: push old committed state to undo stack,
    /// update committed to the new state.
    fn commit(&mut self, dungeon: &Dungeon, hash: u64) {
        self.undo_stack.push(self.committed.clone());
        if self.undo_stack.len() > MAX_UNDO_ENTRIES {
            self.undo_stack.remove(0);
        }
        self.committed = dungeon.clone();
        self.committed_hash = hash;
        self.redo_stack.clear();
        self.pending_change = false;
    }

    /// Undo the last change. Returns true if state was restored.
    pub fn undo(&mut self, dungeon: &mut Dungeon) -> bool {
        let current_hash = Self::hash_dungeon(dungeon);

        if current_hash != self.committed_hash {
            // There are uncommitted changes (e.g. pending coalesced edits or
            // mid-drag). Undo them by restoring the committed state and pushing
            // the current state to redo.
            self.redo_stack.push(dungeon.clone());
            *dungeon = self.committed.clone();
            self.sync_hashes(dungeon);
            return true;
        }

        // Normal undo: pop from undo stack.
        if let Some(prev) = self.undo_stack.pop() {
            self.redo_stack.push(self.committed.clone());
            self.committed = prev.clone();
            self.committed_hash = Self::hash_dungeon(&prev);
            self.prev_frame_hash = self.committed_hash;
            *dungeon = prev;
            self.pending_change = false;
            self.drag_dirty = false;
            true
        } else {
            false
        }
    }

    /// Redo the last undone change. Returns true if state was restored.
    pub fn redo(&mut self, dungeon: &mut Dungeon) -> bool {
        if let Some(next) = self.redo_stack.pop() {
            self.undo_stack.push(self.committed.clone());
            self.committed = next.clone();
            self.committed_hash = Self::hash_dungeon(&next);
            self.prev_frame_hash = self.committed_hash;
            *dungeon = next;
            self.pending_change = false;
            self.drag_dirty = false;
            true
        } else {
            false
        }
    }

    /// Reset the history (e.g. after New or Open).
    pub fn reset(&mut self, dungeon: &Dungeon) {
        let hash = Self::hash_dungeon(dungeon);
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.committed = dungeon.clone();
        self.committed_hash = hash;
        self.prev_frame_hash = hash;
        self.drag_dirty = false;
        self.pending_change = false;
    }

    fn sync_hashes(&mut self, dungeon: &Dungeon) {
        let hash = Self::hash_dungeon(dungeon);
        self.committed_hash = hash;
        self.prev_frame_hash = hash;
        self.pending_change = false;
        self.drag_dirty = false;
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty() || self.pending_change || self.drag_dirty
        || self.prev_frame_hash != self.committed_hash
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    fn hash_dungeon(dungeon: &Dungeon) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let bytes = serde_json::to_vec(dungeon).unwrap_or_default();
        let mut hasher = DefaultHasher::new();
        bytes.hash(&mut hasher);
        hasher.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Dungeon;

    #[test]
    fn test_undo_redo_basic() {
        let d0 = Dungeon::new("State 0".into());
        let mut history = UndoHistory::new(&d0);

        let mut d1 = Dungeon::new("State 1".into());
        // Simulate a non-drag change that gets committed
        history.commit(&d1, UndoHistory::hash_dungeon(&d1));

        // Undo should restore d0
        assert!(history.undo(&mut d1));
        assert_eq!(d1.name, "State 0");

        // Redo should restore d1
        assert!(history.redo(&mut d1));
        assert_eq!(d1.name, "State 1");
    }

    #[test]
    fn test_undo_with_uncommitted_changes() {
        let d0 = Dungeon::new("State 0".into());
        let mut history = UndoHistory::new(&d0);

        let mut current = Dungeon::new("Uncommitted".into());
        // Don't commit - simulate pending changes

        // Undo should restore committed state (d0)
        assert!(history.undo(&mut current));
        assert_eq!(current.name, "State 0");

        // Redo should restore the uncommitted state
        assert!(history.redo(&mut current));
        assert_eq!(current.name, "Uncommitted");
    }

    #[test]
    fn test_redo_cleared_on_new_change() {
        let d0 = Dungeon::new("State 0".into());
        let mut history = UndoHistory::new(&d0);

        let d1 = Dungeon::new("State 1".into());
        history.commit(&d1, UndoHistory::hash_dungeon(&d1));

        let mut current = d1.clone();
        assert!(history.undo(&mut current));
        assert_eq!(current.name, "State 0");

        // Make a new change instead of redoing
        let d2 = Dungeon::new("State 2".into());
        history.commit(&d2, UndoHistory::hash_dungeon(&d2));

        // Redo should not be available (redo stack cleared)
        current = d2;
        assert!(!history.redo(&mut current));
    }

    #[test]
    fn test_reset_clears_history() {
        let d0 = Dungeon::new("State 0".into());
        let mut history = UndoHistory::new(&d0);

        let d1 = Dungeon::new("State 1".into());
        history.commit(&d1, UndoHistory::hash_dungeon(&d1));

        let d_new = Dungeon::new("Fresh".into());
        history.reset(&d_new);

        let mut current = d_new;
        assert!(!history.undo(&mut current));
        assert!(!history.redo(&mut current));
    }
}
