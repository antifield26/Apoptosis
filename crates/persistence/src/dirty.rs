//! Dirty chunk tracking (P03-11).
//!
//! A save is defined by a **snapshot protocol** rather than a set lookup:
//!
//! 1. the saver takes [`DirtyTracker::snapshot`], which clears the tracker
//!    (changes made during the save belong to the next save, not this one);
//! 2. it writes the snapshot;
//! 3. on failure it feeds the snapshot back with [`DirtyTracker::restore`], so
//!    nothing is silently dropped.
//!
//! Iteration order is deterministic (`BTreeSet` over dimension and chunk
//! coordinates), which keeps the byte layout of a save reproducible for the
//! same dirty set (AGENTS.md section 3.6).

use crate::chunk::{ChunkKey, ChunkPos};
use crate::dimension::Dimension;
use std::collections::{BTreeMap, BTreeSet};

/// Region coordinate of a region file.
pub type RegionCoord = (i32, i32);

/// Tracks which chunks (and whether `level.dat`) still need writing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DirtyTracker {
    chunks: BTreeSet<ChunkKey>,
    level: bool,
}

impl DirtyTracker {
    /// An empty tracker.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Mark a chunk as needing a save.
    pub fn mark_chunk(&mut self, dimension: &Dimension, pos: ChunkPos) {
        self.chunks.insert(ChunkKey::new(dimension, pos));
    }

    /// Mark `level.dat` as needing a save.
    pub fn mark_level(&mut self) {
        self.level = true;
    }

    /// Whether `level.dat` needs a save.
    #[must_use]
    pub const fn is_level_dirty(&self) -> bool {
        self.level
    }

    /// Number of dirty chunks.
    #[must_use]
    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    /// Whether nothing is pending.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty() && !self.level
    }

    /// Whether a specific chunk is dirty.
    #[must_use]
    pub fn contains(&self, dimension: &Dimension, pos: ChunkPos) -> bool {
        self.chunks.contains(&ChunkKey::new(dimension, pos))
    }

    /// Dirty chunks in deterministic order.
    pub fn chunks(&self) -> impl Iterator<Item = &ChunkKey> {
        self.chunks.iter()
    }

    /// Take everything pending and reset the tracker.
    #[must_use]
    pub fn snapshot(&mut self) -> DirtySnapshot {
        DirtySnapshot {
            chunks: std::mem::take(&mut self.chunks),
            level: std::mem::replace(&mut self.level, false),
        }
    }

    /// Re-mark everything in `snapshot` as dirty (used after a failed save).
    pub fn restore(&mut self, snapshot: DirtySnapshot) {
        self.chunks.extend(snapshot.chunks);
        self.level |= snapshot.level;
    }
}

/// A point-in-time set of pending writes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DirtySnapshot {
    chunks: BTreeSet<ChunkKey>,
    level: bool,
}

impl DirtySnapshot {
    /// Number of chunks in the snapshot.
    #[must_use]
    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    /// Whether the snapshot holds nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty() && !self.level
    }

    /// Whether `level.dat` is part of the snapshot.
    #[must_use]
    pub const fn level(&self) -> bool {
        self.level
    }

    /// Chunks in deterministic order.
    pub fn chunks(&self) -> impl Iterator<Item = &ChunkKey> {
        self.chunks.iter()
    }

    /// Group the snapshot by region file, so each `r.X.Z.mca` is opened,
    /// written and synced once.
    #[must_use]
    pub fn by_region(&self) -> BTreeMap<(Dimension, RegionCoord), Vec<ChunkPos>> {
        let mut grouped: BTreeMap<(Dimension, RegionCoord), Vec<ChunkPos>> = BTreeMap::new();
        for key in &self.chunks {
            grouped
                .entry((key.dimension.clone(), key.region()))
                .or_default()
                .push(key.pos);
        }
        grouped
    }
}

#[cfg(test)]
mod tests {
    use super::DirtyTracker;
    use crate::chunk::{ChunkKey, ChunkPos};
    use crate::dimension::Dimension;

    #[test]
    fn marking_and_snapshotting() {
        let mut tracker = DirtyTracker::new();
        assert!(tracker.is_empty());
        tracker.mark_chunk(&Dimension::Overworld, ChunkPos::new(1, 2));
        tracker.mark_chunk(&Dimension::Overworld, ChunkPos::new(1, 2));
        tracker.mark_chunk(&Dimension::Nether, ChunkPos::new(1, 2));
        tracker.mark_level();
        assert_eq!(tracker.chunk_count(), 2, "duplicates collapse");
        assert!(tracker.is_level_dirty());
        assert!(tracker.contains(&Dimension::Overworld, ChunkPos::new(1, 2)));
        assert!(!tracker.contains(&Dimension::End, ChunkPos::new(1, 2)));

        let snapshot = tracker.snapshot();
        assert_eq!(snapshot.chunk_count(), 2);
        assert!(snapshot.level());
        assert!(tracker.is_empty(), "snapshot clears the tracker");
    }

    #[test]
    fn failed_save_restores_everything() {
        let mut tracker = DirtyTracker::new();
        tracker.mark_chunk(&Dimension::Overworld, ChunkPos::new(4, 4));
        tracker.mark_level();
        let snapshot = tracker.snapshot();
        // A change arrives while the save is in flight.
        tracker.mark_chunk(&Dimension::Overworld, ChunkPos::new(9, 9));
        // The save fails; the snapshot goes back.
        tracker.restore(snapshot);
        assert_eq!(tracker.chunk_count(), 2);
        assert!(tracker.is_level_dirty());
        assert!(tracker.contains(&Dimension::Overworld, ChunkPos::new(4, 4)));
        assert!(tracker.contains(&Dimension::Overworld, ChunkPos::new(9, 9)));
    }

    #[test]
    fn grouping_preserves_deterministic_order() {
        let mut tracker = DirtyTracker::new();
        for (dimension, x, z) in [
            (Dimension::Nether, 33, 0),
            (Dimension::Overworld, 40, 3),
            (Dimension::Overworld, -1, -1),
            (Dimension::Overworld, 2, 2),
        ] {
            tracker.mark_chunk(&dimension, ChunkPos::new(x, z));
        }
        let snapshot = tracker.snapshot();
        let grouped = snapshot.by_region();
        // Regions: overworld (-1,-1), (0,0), (1,0) and nether (1,0).
        assert_eq!(grouped.len(), 4);
        let keys: Vec<_> = grouped.keys().cloned().collect();
        // BTreeMap ordering: dimension first (Overworld < Nether by enum order),
        // then region coordinates.
        assert_eq!(keys[0].0, Dimension::Overworld);
        assert_eq!(keys[0].1, (-1, -1));
        assert_eq!(keys[1].0, Dimension::Overworld);
        assert_eq!(keys[1].1, (0, 0));
        assert_eq!(keys[2].0, Dimension::Overworld);
        assert_eq!(keys[2].1, (1, 0));
        assert_eq!(keys[3].0, Dimension::Nether);
        let overworld_second = grouped
            .get(&(Dimension::Overworld, (1, 0)))
            .expect("region");
        assert_eq!(overworld_second, &vec![ChunkPos::new(40, 3)]);

        // Snapshot chunk order is stable across identical inputs.
        let mut again = DirtyTracker::new();
        for (dimension, x, z) in [
            (Dimension::Overworld, 2, 2),
            (Dimension::Overworld, -1, -1),
            (Dimension::Nether, 33, 0),
            (Dimension::Overworld, 40, 3),
        ] {
            again.mark_chunk(&dimension, ChunkPos::new(x, z));
        }
        let a: Vec<ChunkKey> = snapshot.chunks().cloned().collect();
        let b: Vec<ChunkKey> = again.snapshot().chunks().cloned().collect();
        assert_eq!(a, b, "mark order must not affect save order");
    }
}
