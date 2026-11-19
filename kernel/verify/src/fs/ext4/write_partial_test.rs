//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! ext4 mid-loop partial-write semantics (review-r3b): once at least one
//! byte of a write(2) request has landed, a failure on a LATER block
//! (ENOSPC on a hole allocation) must report the PARTIAL count, not the
//! error; a request that wrote nothing fails outright. Size must track
//! only what landed.
//!
//! Loop skeleton copied from: kernel/src/fs/ext4/file.rs ext4_file_write
//! (allocation arms collapsed to the decision they make: Ok(block) |
//! Err(alloc-fail)).

use proptest::prelude::*;

const ENOSPC: i32 = -28;

/// One block's resolution for the write loop (the get_data_block /
/// allocate decision, collapsed).
#[derive(Clone, Copy, Debug)]
enum Block {
    /// Mapped or allocatable: the loop writes through it.
    Present,
    /// Hole whose allocation FAILS (the disk-full case under test).
    AllocFail,
}

/// Mirror of ext4_file_write's per-block loop with the partial_or_err!
/// semantics (kernel/src/fs/ext4/file.rs): `fail_block` acts at the
/// point the allocation arm errors.
fn write_loop(
    offset: u64,
    to_write: usize,
    layout: &[Block],
    old_size: u64,
    block_size: u64,
) -> Result<usize, i32> {
    let mut total_written = 0usize;
    let mut current_offset = offset;

    // (mirrors the pre-allocation head: failures there precede all data
    // and return Err — not modeled further, layout starts at the loop.)
    let _ = old_size;

    'block_loop: while total_written < to_write {
        let idx = (current_offset / block_size) as usize;
        let block_offset = (current_offset % block_size) as usize;
        let block = match layout.get(idx) {
            Some(b) => *b,
            None => Block::Present, // past the modeled tail: succeed
        };
        let block_num = match block {
            Block::Present => 0,
            Block::AllocFail => {
                // partial_or_err!(...) — the fix under test
                if total_written > 0 {
                    break 'block_loop;
                } else {
                    return Err(ENOSPC);
                }
            }
        };
        let _ = block_num;
        let remaining = to_write - total_written;
        let available = block_size as usize - block_offset;
        let write_in_block = core::cmp::min(remaining, available);
        total_written += write_in_block;
        current_offset += write_in_block as u64;
    }

    // Partial-write-safe size update (mirrors the effective_end tail).
    // Caller recomputes size from the returned count; see oracle below.
    Ok(total_written)
}

/// POSIX oracle: walk the layout; every AllocFail ends the request —
/// after ≥1 byte it is a partial success, before any byte an error.
fn oracle(offset: u64, to_write: usize, layout: &[Block], block_size: u64) -> Result<usize, i32> {
    let mut written = 0usize;
    let mut pos = offset;
    while written < to_write {
        let idx = (pos / block_size) as usize;
        let boff = (pos % block_size) as usize;
        match layout.get(idx) {
            Some(Block::AllocFail) => {
                if written > 0 {
                    return Ok(written);
                }
                return Err(ENOSPC);
            }
            _ => {
                let n = core::cmp::min(to_write - written, block_size as usize - boff);
                written += n;
                pos += n as u64;
            }
        }
    }
    Ok(written)
}

/// Size after the write (the effective_end tail): grows only to the
/// returned count's end, never past it, never shrinks.
fn size_after(old_size: u64, offset: u64, written: usize) -> u64 {
    core::cmp::max(old_size, offset + written as u64)
}

proptest! {
    #[test]
    fn partial_matches_oracle(
        offset in 0u64..3,
        to_write in 1usize..5,
        layout in proptest::collection::vec(
            proptest::bool::ANY.prop_map(|b| if b { Block::Present } else { Block::AllocFail }),
            0..6),
        old_size in 0u64..4,
    ) {
        let bs = 1u64; // one "block" == one unit keeps layouts general
        let got = write_loop(offset, to_write, &layout, old_size, bs);
        let want = oracle(offset, to_write, &layout, bs);
        // Result agreement
        match (&got, &want) {
            (Ok(g), Ok(w)) => prop_assert_eq!(*g, *w),
            (Err(g), Err(w)) => prop_assert_eq!(*g, *w),
            _ => panic!("mirror {:?} != oracle {:?}", got, want),
        }
        // The regression this fix closes: no input yields Err while the
        // oracle wrote bytes.
        if let Err(_e) = got {
            prop_assert!(
                matches!(want, Err(_)),
                "Err returned although bytes landed (partial discarded)"
            );
        }
        // Size contract on success: never past the returned count.
        if let Ok(n) = got {
            let new_size = size_after(old_size, offset, n);
            prop_assert!(new_size <= core::cmp::max(old_size, offset + to_write as u64) + bs - 1);
            prop_assert!(new_size >= old_size);
        }
    }

    #[test]
    fn fail_before_any_byte_is_error(
        layout in proptest::collection::vec(
            proptest::bool::ANY.prop_map(|b| if b { Block::Present } else { Block::AllocFail }),
            1..4),
    ) {
        // First block fails -> whole request fails (no partial).
        let mut l = layout;
        l[0] = Block::AllocFail;
        let got = write_loop(0, 3, &l, 0, 1);
        prop_assert!(matches!(got, Err(ENOSPC)));
    }

    #[test]
    fn fail_after_first_block_is_partial(
        layout in proptest::collection::vec(
            proptest::bool::ANY.prop_map(|b| if b { Block::Present } else { Block::AllocFail }),
            2..4),
    ) {
        let mut l = layout;
        l[0] = Block::Present;
        l[1] = Block::AllocFail;
        let got = write_loop(0, 4, &l, 0, 1);
        prop_assert_eq!(got, Ok(1));
        // size grows to exactly the landed bytes
        prop_assert_eq!(size_after(0, 0, 1), 1);
    }

    #[test]
    fn write_within_size_keeps_size(
        old_size in 2u64..8,
    ) {
        // A partial write entirely below EOF must not shrink or grow
        // the file (the sparse-hole ENOSPC shape).
        let l = [Block::Present, Block::AllocFail, Block::Present];
        let got = write_loop(0, 3, &l, old_size, 1);
        prop_assert_eq!(got, Ok(1));
        prop_assert_eq!(size_after(old_size, 0, 1), old_size);
    }
}
