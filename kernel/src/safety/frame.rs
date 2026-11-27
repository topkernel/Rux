//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Kernel-stack window primitive — the stack-forensics scanner family
//! (unsafe-slim round 4).
//!
//! The diagnostic stack walkers (dfx taskdump/slowtask, the timer
//! ghost-scan, the TXN-HANG entry-frame readers) all re-implement one
//! shape: given a stack's base/top bounds, walk candidate frame
//! addresses and `read_volatile` words out of possibly-stale kernel
//! stack memory. Each hand-rolled loop re-proved "the address is
//! inside the stack and 8-aligned" before the read; the proof lived
//! at every site and at least one class of it was wrong enough times
//! to earn hunt rounds of its own.
//!
//! [`KernelStackWindow`] owns the proof once:
//!
//! * **INV-WINDOW-1 (bounds)** — the window is built from the stack's
//!   own base/top; every accessor refuses addresses outside
//!   `[base, top)`, so a scan can never leave the stack.
//! * **INV-WINDOW-2 (alignment)** — word reads require the address to
//!   be 8-aligned; misaligned candidate frame pointers are refused,
//!   not rounded.
//! * **INV-WINDOW-3 (stale memory)** — the bytes are read
//!   volatile: the memory may be concurrently written by the task
//!   that owns the stack (or already dead); a read never tears under
//!   the kernel's 8-byte accesses.

/// A bounded view over one task's kernel stack (or any checked
/// kernel-memory window) for forensic word reads.
#[derive(Clone, Copy)]
pub struct KernelStackWindow {
    base: usize,
    top: usize,
}

impl KernelStackWindow {
    /// Build from the stack's base (low address) and top (high
    /// address). `None` when the pair is inverted or misaligned.
    pub fn new(base: usize, top: usize) -> Option<Self> {
        if top <= base || (top - base) % 8 != 0 || base & 7 != 0 {
            return None;
        }
        Some(KernelStackWindow { base, top })
    }

    pub fn base(&self) -> usize {
        self.base
    }

    pub fn top(&self) -> usize {
        self.top
    }

    /// Volatile read of one u64 at `addr` (INV-WINDOW-1/2/3). `None`
    /// when the address is outside the window or misaligned.
    #[inline]
    pub fn read_u64(&self, addr: usize) -> Option<u64> {
        // checked_add (Layer-2.2 Kani conviction #2): a hostile addr
        // near usize::MAX made the OLD `addr + 8` overflow inside this
        // guard BEFORE the refusal — the bounds check itself was
        // UB-prone for the very inputs it exists to reject.
        let end = match addr.checked_add(8) {
            Some(e) => e,
            None => return None,
        };
        if addr < self.base || end > self.top || addr & 7 != 0 {
            return None;
        }
        // SAFETY: INV-WINDOW-1/2 — bounds and alignment checked above;
        // INV-WINDOW-3 — volatile read of possibly-live stack memory.
        Some(super::edges::volatile_read(addr as *const u64))
    }

    /// Read the saved frame words at a candidate frame pointer into
    /// `out` (INV-WINDOW-1/2/3). Stops at the first refused address —
    /// partial fills are the caller's signal that the walk left the
    /// window.
    pub fn read_frame(&self, sp: usize, out: &mut [u64]) -> usize {
        let mut n = 0;
        for (i, w) in out.iter_mut().enumerate() {
            match self.read_u64(sp + i * 8) {
                Some(v) => *w = v,
                None => break,
            }
            n += 1;
        }
        n
    }
}
