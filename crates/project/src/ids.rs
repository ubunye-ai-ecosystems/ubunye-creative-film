// Copyright Ubunye AI Ecosystems, 2026. Licensed under MIT OR Apache-2.0.
//! Ids that stay unique when two computers edit copies of the same project.
//!
//! Every id in a project (items, bins, tracks, clips, transitions, markers, links, groups) comes
//! from one counter, [`Project::next_id`]. Two people who open the same file start from the same
//! counter, so both would make "clip 7" for two different clips, and joining their work would mix
//! them up. The fix keeps ids as plain numbers (old projects load unchanged) and gives each
//! editing session its own block of numbers:
//!
//! - an id is `block * 2^24 + n`. Block 0 holds every id made before this change.
//! - when a session opens or creates a project it moves the counter to the start of a block
//!   nobody uses yet ([`Project::enter_id_block`]): a random one, or one the caller derives from
//!   the device (Kopano passes one). New ids then come from that block only.
//! - blocks run up to `2^29 - 1`, so every id stays below `2^53` and survives JSON readers that
//!   hold numbers as doubles (JavaScript).
//!
//! Two sessions clash only if they pick the same random block: about one in 268 million per pair.
//! A block holds 16.7 million ids; a session that uses them all simply runs on into the next
//! block, which is as unlikely to be taken.

use std::collections::BTreeSet;

use crate::{Bin, BinEntry, ItemKind, Project};

/// Low bits of an id: the number inside its block.
pub const ID_BLOCK_BITS: u32 = 24;
/// Highest block number. Blocks go from 1 to here; ids stay below 2^53.
pub const MAX_ID_BLOCK: u64 = (1 << (53 - ID_BLOCK_BITS)) - 1;

/// The block an id belongs to (0 for ids made before blocks existed).
pub fn id_block(id: u64) -> u64 {
    id >> ID_BLOCK_BITS
}

/// Why a block could not be entered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IdBlockError {
    /// 0 is the shared block of old projects; above [`MAX_ID_BLOCK`] ids would pass 2^53.
    OutOfRange(u64),
    /// Some id in the project already lives in this block (another session made it).
    InUse(u64),
}

impl std::fmt::Display for IdBlockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IdBlockError::OutOfRange(b) => write!(f, "id block {b} is out of range (1 to {MAX_ID_BLOCK})"),
            IdBlockError::InUse(b) => write!(f, "id block {b} is already used in this project"),
        }
    }
}

impl std::error::Error for IdBlockError {}

fn bin_ids(bin: &Bin, out: &mut BTreeSet<u64>, depth: usize) {
    out.insert(bin.id.0);
    // Bins come from project files: never recurse without a bound.
    if depth > 256 {
        return;
    }
    for c in &bin.children {
        if let BinEntry::Bin(b) = c {
            bin_ids(b, out, depth + 1);
        }
    }
}

impl Project {
    /// The blocks that ids of this project already live in.
    pub fn id_blocks_in_use(&self) -> BTreeSet<u64> {
        let mut ids = BTreeSet::new();
        bin_ids(&self.root, &mut ids, 0);
        for (id, it) in &self.items {
            ids.insert(id.0);
            if let ItemKind::Media(m) = &it.kind {
                ids.extend(m.markers.iter().map(|k| k.id.0));
            }
            if let ItemKind::Sequence(seq) = &it.kind {
                ids.extend(seq.markers.iter().map(|k| k.id.0));
                for c in &seq.caption_tracks {
                    ids.insert(c.id.0);
                    ids.extend(c.captions.iter().map(|x| x.id.0));
                }
                for t in seq.video_tracks.iter().chain(seq.audio_tracks.iter()).chain(seq.submix_tracks.iter()) {
                    ids.insert(t.id.0);
                    ids.extend(t.transitions.iter().map(|x| x.id.0));
                    for c in &t.items {
                        ids.insert(c.id.0);
                        ids.extend(c.link);
                        ids.extend(c.group);
                        ids.extend(c.markers.iter().map(|k| k.id.0));
                    }
                }
            }
        }
        // the counter itself: the block the last session was allocating from
        ids.insert(self.next_id.saturating_sub(1));
        ids.into_iter().map(id_block).collect()
    }

    /// The block new ids come from now.
    pub fn current_id_block(&self) -> u64 {
        id_block(self.next_id)
    }

    /// Make new ids come from `block`. Refused for block 0, a block past [`MAX_ID_BLOCK`] and a
    /// block some id of the project already lives in. Existing ids never change.
    pub fn enter_id_block(&mut self, block: u64) -> Result<(), IdBlockError> {
        if block == 0 || block > MAX_ID_BLOCK {
            return Err(IdBlockError::OutOfRange(block));
        }
        if self.id_blocks_in_use().contains(&block) {
            return Err(IdBlockError::InUse(block));
        }
        // id `block << BITS` itself is never handed out, so a block in use always shows in
        // `next_id - 1` (see `id_blocks_in_use`).
        self.next_id = (block << ID_BLOCK_BITS) + 1;
        Ok(())
    }

    /// Enter a block drawn from `random` (any 64 random bits): retries with the next candidate
    /// when one is in use. Returns the block entered.
    pub fn enter_random_id_block(&mut self, mut random: u64) -> u64 {
        for _ in 0..64 {
            let block = 1 + random % MAX_ID_BLOCK;
            if self.enter_id_block(block).is_ok() {
                return block;
            }
            // splitmix64 step: a different candidate from the same seed
            random = random.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = random;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            random = z ^ (z >> 31);
        }
        self.current_id_block()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_sequence() -> (Project, crate::ItemId) {
        let mut p = Project::new("t");
        let seq = p.new_sequence("Seq", crate::SequenceSettings::default(), 2, 2, None);
        (p, seq)
    }

    #[test]
    fn old_projects_keep_their_ids_and_new_ones_come_from_the_block() {
        let (mut p, seq) = with_sequence();
        let before: Vec<u64> = p.items.keys().map(|k| k.0).collect();
        assert!(before.iter().all(|&i| id_block(i) == 0));
        p.enter_id_block(42).unwrap();
        assert_eq!(p.items.keys().map(|k| k.0).collect::<Vec<_>>(), before);
        let id = p.alloc_id();
        assert_eq!(id_block(id), 42);
        assert!(p.sequence(seq).is_some());
    }

    /// The bug: two people open the same file and each add a clip. Without blocks both get the
    /// same id, so the merged timeline has two different clips claiming one id.
    #[test]
    fn two_copies_of_one_project_never_make_the_same_id() {
        let (base, _) = with_sequence();
        let (mut alice, mut bob) = (base.clone(), base.clone());
        // what happened before: both counters equal
        assert_eq!(alice.clone().alloc_id(), bob.clone().alloc_id());
        alice.enter_random_id_block(0x1234_5678_9abc_def0);
        bob.enter_random_id_block(0x0fed_cba9_8765_4321);
        let a: BTreeSet<u64> = (0..1000).map(|_| alice.alloc_id()).collect();
        let b: BTreeSet<u64> = (0..1000).map(|_| bob.alloc_id()).collect();
        assert!(a.is_disjoint(&b));
        assert!(a.iter().chain(b.iter()).all(|&i| i < (1u64 << 53)));
    }

    #[test]
    fn a_block_in_use_or_out_of_range_is_refused() {
        let (mut p, seq) = with_sequence();
        assert_eq!(p.enter_id_block(0), Err(IdBlockError::OutOfRange(0)));
        assert_eq!(p.enter_id_block(MAX_ID_BLOCK + 1), Err(IdBlockError::OutOfRange(MAX_ID_BLOCK + 1)));
        p.enter_id_block(7).unwrap();
        // a bin made in block 7 (by this session)
        let bin = p.add_bin("Footage", None);
        assert_eq!(id_block(bin.0), 7);
        assert!(p.sequence(seq).is_some());
        // a later session must not enter block 7 again, even with the counter moved away
        p.enter_id_block(9).unwrap();
        assert_eq!(p.enter_id_block(7), Err(IdBlockError::InUse(7)));
        // and the block the counter sits in is in use too
        p.alloc_id();
        assert_eq!(p.enter_id_block(9), Err(IdBlockError::InUse(9)));
    }

    #[test]
    fn random_blocks_retry_past_a_taken_one() {
        let (mut p, _) = with_sequence();
        let first = p.enter_random_id_block(5);
        p.alloc_id();
        // same seed again: the first candidate is taken, a different block is entered
        let second = p.enter_random_id_block(5);
        assert_ne!(first, second);
        assert!((1..=MAX_ID_BLOCK).contains(&second));
    }

    #[test]
    fn ids_and_counter_survive_a_save_and_load() {
        let (mut p, _) = with_sequence();
        p.enter_id_block(123).unwrap();
        let id = p.alloc_id();
        let back: Project = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert_eq!(back.next_id, id + 1);
        assert!(back.id_blocks_in_use().contains(&123));
    }
}
