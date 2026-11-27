//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Bounds-checked volatile MMIO window — the device-register family's
//! primitive (unsafe-slim round 3 lane A).
//!
//! Device drivers previously did `(base + offset) as *const u32` +
//! `read_volatile` per register access, each in its own unsafe block,
//! with the bounds/aliasing argument re-stated per site (151 volatile
//! regions in mm+drivers alone). An `MmioRegion` states it once:
//! construction carries the mapped-window proof, accessors are safe.

/// A device MMIO register window.
///
/// Invariants:
///
/// * **INV-MMIO-1 (mapping)** — `base..base+len` is mapped device MMIO
///   for the machine's remaining lifetime. The ONE audited edge per
///   device is `new` at probe time, where the arch mapping (or the
///   PCI BAR) is established.
/// * **INV-MMIO-2 (bounds)** — every accessor checks `offset + width
///   <= len` before touching the address; an out-of-range offset is a
///   device-driver protocol bug that panics loudly instead of walking
///   foreign memory. Any in-range offset is sound to pass.
/// * **INV-MMIO-3 (volatility)** — all accesses are volatile; the
///   compiler may not elide, merge, or reorder the access itself.
///   Cross-access ordering still needs the driver's fences (device
///   protocols own their barriers).
#[derive(Clone, Copy, Debug)]
pub struct MmioRegion {
    base: usize,
    len: usize,
}

impl MmioRegion {
    /// The audited construction edge (INV-MMIO-1): call once per
    /// device at probe, where the mapping is proven.
    pub const fn new(base: usize, len: usize) -> Self {
        MmioRegion { base, len }
    }

    /// Base virtual address (for legacy code paths that still compute
    /// sub-window pointers); the region discipline applies.
    pub const fn base(&self) -> usize {
        self.base
    }

    /// Window length in bytes.
    pub const fn len(&self) -> usize {
        self.len
    }

    fn check(&self, off: usize, width: usize, what: &str) -> usize {
        // INV-MMIO alignment enforced: the SAFETY contract relied on
        // "every window Rux maps is naturally aligned" as an unforced
        // precondition — Miri caught that a misaligned base + read_u32
        // is UB (no in-tree tripper: real MMIO bases are page-aligned).
        // The assert turns the documented contract into a checked one.
        debug_assert!(
            // wrapping_add: alignment is a modulo property, and `off`
            // may be ANY usize here (the window check below rejects
            // the huge ones AFTER this line) — the Layer-2.2 Kani
            // proof convicted the plain `self.base + off` as an
            // overflow path in the checker itself (off near
            // usize::MAX before the guard ran).
            (self.base.wrapping_add(off)) % width == 0,
            "MMIO {} misaligned: base={:#x} off={:#x} width={}",
            what, self.base, off, width
        );
        let end = off.checked_add(width);
        match end {
            Some(e) if e <= self.len => self.base + off,
            _ => panic!("MMIO {} out of window: off={:#x} width={} len={:#x}", what, off, width, self.len),
        }
    }

    /// Volatile u8 read (INV-MMIO-2/3).
    #[inline]
    pub fn read_u8(&self, off: usize) -> u8 {
        let addr = self.check(off, 1, "read_u8") as *const u8;
        // SAFETY: INV-MMIO-1/2 — the address is inside the mapped
        // window and readable for the machine's lifetime; volatile so
        // the device access happens exactly once (INV-MMIO-3).
        super::edges::volatile_read(addr)
    }

    /// Volatile u16 read (INV-MMIO-2/3).
    #[inline]
    pub fn read_u16(&self, off: usize) -> u16 {
        let addr = self.check(off, 2, "read_u16") as *const u16;
        // SAFETY: INV-MMIO-1/2/3 — bounds-checked mapped window; volatile.
        super::edges::volatile_read(addr)
    }

    /// Volatile u32 read (INV-MMIO-2/3).
    #[inline]
    pub fn read_u32(&self, off: usize) -> u32 {
        let addr = self.check(off, 4, "read_u32") as *const u32;
        // SAFETY: INV-MMIO-1/2/3 — bounds-checked mapped window;
        // MMIO regions are not required to be aligned-mod-4 by
        // hardware spec in all cases, but every window Rux maps is
        // naturally aligned; volatile per INV-MMIO-3.
        super::edges::volatile_read(addr)
    }

    /// Volatile u64 read (INV-MMIO-2/3).
    #[inline]
    pub fn read_u64(&self, off: usize) -> u64 {
        let addr = self.check(off, 8, "read_u64") as *const u64;
        // SAFETY: INV-MMIO-1/2/3 — as read_u32, width 8.
        super::edges::volatile_read(addr)
    }

    /// Byte-composed u32 read for PACKED firmware-table fields (little
    /// endian). ACPI layouts pack their structs, so variable-offset
    /// fields (MADT entry bodies after odd-length predecessors, XSDT
    /// entries at the 4-mod-8 +36 body start) are legitimately NOT
    /// naturally aligned — Linux reads them through #repr(packed)
    /// structs, which compile to exactly this byte-wise composition.
    /// u8 reads are always alignment-valid (INV-MMIO holds), so this
    /// accessor is the contract-clean shape for those fields. Only for
    /// RAM-backed firmware tables — never device registers, where
    /// multi-byte access WIDTH is itself observable.
    #[inline]
    pub fn read_packed_u32(&self, off: usize) -> u32 {
        let b0 = self.read_u8(off) as u32;
        let b1 = self.read_u8(off + 1) as u32;
        let b2 = self.read_u8(off + 2) as u32;
        let b3 = self.read_u8(off + 3) as u32;
        b0 | (b1 << 8) | (b2 << 16) | (b3 << 24)
    }

    /// Byte-composed u64 read for packed firmware-table fields — the
    /// width-8 counterpart of `read_packed_u32` (XSDT entry pointers).
    #[inline]
    pub fn read_packed_u64(&self, off: usize) -> u64 {
        let lo = self.read_packed_u32(off) as u64;
        let hi = self.read_packed_u32(off + 4) as u64;
        lo | (hi << 32)
    }

    /// Volatile u8 write (INV-MMIO-2/3).
    #[inline]
    pub fn write_u8(&self, off: usize, val: u8) {
        let addr = self.check(off, 1, "write_u8") as *mut u8;
        // SAFETY: INV-MMIO-1/2/3 — bounds-checked mapped window;
        // volatile write reaches the device exactly once.
        super::edges::volatile_write(addr, val)
    }

    /// Volatile u16 write (INV-MMIO-2/3).
    #[inline]
    pub fn write_u16(&self, off: usize, val: u16) {
        let addr = self.check(off, 2, "write_u16") as *mut u16;
        // SAFETY: INV-MMIO-1/2/3 — as read_u16, write direction.
        super::edges::volatile_write(addr, val)
    }

    /// Volatile u32 write (INV-MMIO-2/3).
    #[inline]
    pub fn write_u32(&self, off: usize, val: u32) {
        let addr = self.check(off, 4, "write_u32") as *mut u32;
        // SAFETY: INV-MMIO-1/2/3 — as read_u32, write direction.
        super::edges::volatile_write(addr, val)
    }

    /// Volatile u64 write (INV-MMIO-2/3).
    #[inline]
    pub fn write_u64(&self, off: usize, val: u64) {
        let addr = self.check(off, 8, "write_u64") as *mut u64;
        // SAFETY: INV-MMIO-1/2/3 — as read_u64, write direction.
        super::edges::volatile_write(addr, val)
    }

    /// Typed sub-window pointer (escape hatch for register-file
    /// overlays: `ptr_at::<T>(off)` for a `T` the device overlays at
    /// `off`). INV-MMIO-1/2 still apply to the caller's accesses via
    /// that pointer; prefer the typed accessors above.
    #[inline]
    pub fn ptr_at<T>(&self, off: usize) -> *mut T {
        let addr = self.check(off, core::mem::size_of::<T>(), "ptr_at");
        addr as *mut T
    }
}
