pub mod grid_math;

pub use grid_math::*;

/// Feed a value's JSON form into a hasher. The JSON goes through a reused buffer and is
/// hashed in one write, since serde_json's many tiny writes are slow to hash directly.
pub fn hash_serde<H: std::hash::Hasher>(h: &mut H, value: &impl serde::Serialize) {
    thread_local! {
        static BUF: std::cell::RefCell<Vec<u8>> = const { std::cell::RefCell::new(Vec::new()) };
    }
    BUF.with_borrow_mut(|buf| {
        buf.clear();
        let _ = serde_json::to_writer(&mut *buf, value);
        h.write(buf);
    });
}

/// FxHash-style hasher for grid-cell keys. Line-of-sight, lighting and hatching do
/// millions of cell-set lookups, where the default SipHash dominates the cost.
/// Cells are not attacker-controlled, so DoS resistance isn't needed.
#[derive(Default, Clone, Copy)]
pub struct CellHasher(u64);

impl std::hash::Hasher for CellHasher {
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.add(b as u64);
        }
    }
    fn write_i32(&mut self, i: i32) {
        self.add(i as u32 as u64);
    }
    fn finish(&self) -> u64 {
        self.0
    }
}

impl CellHasher {
    fn add(&mut self, word: u64) {
        self.0 = (self.0.rotate_left(5) ^ word).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }
}

pub type CellBuildHasher = std::hash::BuildHasherDefault<CellHasher>;
/// A set of grid cells with the fast [`CellHasher`].
pub type CellSet = std::collections::HashSet<(i32, i32), CellBuildHasher>;
/// A map keyed by grid cell with the fast [`CellHasher`].
pub type CellMap<V> = std::collections::HashMap<(i32, i32), V, CellBuildHasher>;
