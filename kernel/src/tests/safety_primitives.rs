//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Tests for the safety primitives (unsafe-slim round 3 lane A):
//! KPtr, StaticRef, DmaBlock/DmaSlot, MmioRegion bounds, safety_cast.

use crate::safety::call::safety_cast;
use crate::safety::dma::{DmaBlock, DmaSlot};
use crate::safety::mmio::MmioRegion;
use crate::safety::ptr::{KPtr, StaticRef};
use super::{test_pass, test_fail, test_group_start};

struct DropCounter {
    dropped: &'static core::sync::atomic::AtomicUsize,
}

impl Drop for DropCounter {
    fn drop(&mut self) {
        self.dropped.fetch_add(1, core::sync::atomic::Ordering::SeqCst);
    }
}

static DROP_COUNT: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);

pub fn test_safety_primitives() {
    test_group_start("safety primitives");

    // ---- KPtr: construction, deref, unique mutability ----
    let mut k = match KPtr::new(0x1234_5678u64) {
        Some(k) => k,
        None => {
            test_fail("kptr new", "allocation failed");
            return;
        }
    };
    if *k == 0x1234_5678 {
        test_pass("kptr new + deref");
    } else {
        test_fail("kptr new + deref", "value mismatch");
    }
    *k += 1;
    if *k == 0x1234_5679 {
        test_pass("kptr deref_mut");
    } else {
        test_fail("kptr deref_mut", "mutation lost");
    }

    // ---- KPtr: raw escape + re-adoption (INV-KPTR-4) ----
    let raw = k.into_raw();
    // SAFETY: re-adopting the pointer released one line above.
    let k2 = unsafe { KPtr::from_raw(raw) };
    if *k2 == 0x1234_5679 {
        test_pass("kptr into_raw/from_raw roundtrip");
    } else {
        test_fail("kptr into_raw/from_raw roundtrip", "value changed");
    }
    drop(k2);

    // ---- KPtr: drop glue runs exactly once (INV-KPTR-2/3) ----
    DROP_COUNT.store(0, core::sync::atomic::Ordering::SeqCst);
    {
        let _a = KPtr::new(DropCounter { dropped: &DROP_COUNT });
        let _b = KPtr::new(DropCounter { dropped: &DROP_COUNT });
    }
    if DROP_COUNT.load(core::sync::atomic::Ordering::SeqCst) == 2 {
        test_pass("kptr drop exactly once per value");
    } else {
        test_fail(
            "kptr drop exactly once per value",
            "drop count != 2 (double drop or leak)",
        );
    }

    // ---- KPtr: volatile whole-value read/write (INV-KPTR-5) ----
    let kv = match KPtr::new([1u8, 2, 3, 4]) {
        Some(kv) => kv,
        None => {
            test_fail("kptr volatile", "allocation failed");
            return;
        }
    };
    kv.write_volatile([9u8, 8, 7, 6]);
    if kv.read_volatile() == [9u8, 8, 7, 6] {
        test_pass("kptr volatile read/write");
    } else {
        test_fail("kptr volatile read/write", "value mismatch");
    }
    drop(kv);

    // ---- StaticRef: adopt a static, Copy clone, 'static get ----
    static VALUE: u32 = 42;
    let s = StaticRef::from_static(&VALUE);
    let s2 = s;
    if *s == 42 && *s2 == 42 {
        test_pass("staticref from_static + copy + deref");
    } else {
        test_fail("staticref from_static + copy + deref", "value mismatch");
    }
    static TABLE: [u16; 4] = [1, 2, 3, 4];
    let entry = StaticRef::from_static(&TABLE).map(|t| &t[2]);
    if *entry == 3 {
        test_pass("staticref map projection");
    } else {
        test_fail("staticref map projection", "projection wrong");
    }

    // ---- DmaBlock: carve alignment/disjointness (INV-DMA-2) ----
    let mut block = match DmaBlock::new(256, 16) {
        Some(b) => b,
        None => {
            test_fail("dmablock new", "allocation failed");
            return;
        }
    };
    let (mut descs, off_desc): (DmaSlot<[u64]>, usize) = match block.carve_array::<u64>(4) {
        Some(x) => x,
        None => {
            test_fail("dmablock carve_array", "carve failed");
            return;
        }
    };
    let (mut hdr, off_hdr): (DmaSlot<u32>, usize) = match block.carve() {
        Some(x) => x,
        None => {
            test_fail("dmablock carve", "carve failed");
            return;
        }
    };
    // First carve: aligned to 8 at frontier 0 => offset 0, 32 bytes.
    // Second carve: frontier 32, aligned to 4 => offset 32.
    if off_desc == 0 && off_hdr == 32 && descs.len() == 4 {
        test_pass("dmablock carve offsets disjoint+aligned");
    } else {
        test_fail("dmablock carve offsets disjoint+aligned", "offsets wrong");
    }
    descs[0] = 0xAA;
    *hdr = 0xBB;
    if descs[0] == 0xAA && *hdr == 0xBB {
        test_pass("dmaslot deref/deref_mut disjoint");
    } else {
        test_fail("dmaslot deref/deref_mut disjoint", "writes collided");
    }
    // Volatile at offsets (INV-DMA-3).
    block.write_volatile_at(off_hdr, 0xCCu32);
    if block.read_volatile_at::<u32>(off_hdr) == 0xCC {
        test_pass("dmablock volatile at");
    } else {
        test_fail("dmablock volatile at", "value mismatch");
    }
    drop(hdr);
    drop(descs);
    drop(block);

    // ---- MmioRegion: bounds discipline (INV-MMIO-2) ----
    // Use a plain kernel buffer as a stand-in window: the accesses are
    // ordinary memory through the volatile ops, proving the bounds and
    // read/write plumbing without a device.
    let mut window = [0u32; 8];
    let region = MmioRegion::new(
        window.as_mut_ptr() as usize,
        core::mem::size_of_val(&window),
    );
    region.write_u32(4, 0xDEAD_BEEF);
    let mut ok = region.read_u32(4) == 0xDEAD_BEEF && region.read_u32(0) == 0;
    region.write_u8(9, 0x55); // byte 1 of word 2
    ok = ok && region.read_u8(9) == 0x55;
    if ok {
        test_pass("mmio region u32/u8 access in bounds");
    } else {
        test_fail("mmio region u32/u8 access in bounds", "rw mismatch");
    }
    // Last-word access must be fine (the boundary condition itself).
    region.read_u32(region.len() - 4);
    test_pass("mmio region edge access");

    // ---- safety_cast: size-asserted cast (INV-SAFECAST-1) ----
    let bytes: [u8; 4] = safety_cast::<u32, [u8; 4]>(0x0403_0201);
    if bytes == [0x01, 0x02, 0x03, 0x04] {
        test_pass("safety_cast same-size layout cast");
    } else {
        test_fail("safety_cast same-size layout cast", "layout mismatch");
    }
}
