//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Kani proof harnesses for PID bitmap allocator.
//!
//! Types copied from: kernel/src/process/pid.rs

#![cfg(kani)]

pub const PID_MAX_DEFAULT: u32 = 4096;
pub const RESERVED_PIDS: u32 = 16;

pub struct PidAllocator {
    bitmap: [u64; 64],
    next: u32,
    nr_allocated: u32,
}

impl PidAllocator {
    pub fn new() -> Self {
        Self {
            bitmap: [0u64; 64],
            next: RESERVED_PIDS,
            nr_allocated: 0,
        }
    }

    pub fn alloc_pid(&mut self) -> Option<u32> {
        if self.nr_allocated >= PID_MAX_DEFAULT - RESERVED_PIDS {
            return None;
        }
        let start = self.next;
        for pid in start..PID_MAX_DEFAULT {
            let word_idx = pid as usize / 64;
            let bit_idx = pid as usize % 64;
            if self.bitmap[word_idx] & (1u64 << bit_idx) == 0 {
                self.bitmap[word_idx] |= 1u64 << bit_idx;
                self.nr_allocated += 1;
                self.next = if pid + 1 >= PID_MAX_DEFAULT { RESERVED_PIDS } else { pid + 1 };
                return Some(pid);
            }
        }
        for pid in RESERVED_PIDS..start {
            let word_idx = pid as usize / 64;
            let bit_idx = pid as usize % 64;
            if self.bitmap[word_idx] & (1u64 << bit_idx) == 0 {
                self.bitmap[word_idx] |= 1u64 << bit_idx;
                self.nr_allocated += 1;
                self.next = if pid + 1 >= PID_MAX_DEFAULT { RESERVED_PIDS } else { pid + 1 };
                return Some(pid);
            }
        }
        None
    }

    pub fn free_pid(&mut self, pid: u32) {
        if pid < RESERVED_PIDS || pid >= PID_MAX_DEFAULT { return; }
        let word_idx = pid as usize / 64;
        let bit_idx = pid as usize % 64;
        let mask = 1u64 << bit_idx;
        if self.bitmap[word_idx] & mask != 0 {
            self.bitmap[word_idx] &= !mask;
            self.nr_allocated -= 1;
        }
    }
}

/// INV-PID-K1: allocated PID >= RESERVED_PIDS.
#[kani::proof]
fn verify_pid_reserved() {
    let mut alloc = PidAllocator::new();
    for _ in 0..100 {
        let pid = alloc.alloc_pid().unwrap();
        assert!(pid >= RESERVED_PIDS);
    }
}

/// INV-PID-K2: double-free is safe (no panic, no double-decrement).
#[kani::proof]
fn verify_pid_double_free() {
    let mut alloc = PidAllocator::new();
    let pid = alloc.alloc_pid().unwrap();
    alloc.free_pid(pid);
    alloc.free_pid(pid); // should be no-op
    assert_eq!(alloc.nr_allocated, 0);
}

/// INV-PID-K3: free makes PID available again (nr_allocated consistent).
#[kani::proof]
fn verify_pid_free_consistency() {
    let mut alloc = PidAllocator::new();
    let mut pids = [0u32; 50];
    let n: usize = kani::any();
    kani::assume(n > 0 && n <= 50);
    for i in 0..n {
        pids[i] = alloc.alloc_pid().unwrap();
    }
    assert_eq!(alloc.nr_allocated, n as u32);
    for i in 0..n {
        alloc.free_pid(pids[i]);
    }
    assert_eq!(alloc.nr_allocated, 0);
}
