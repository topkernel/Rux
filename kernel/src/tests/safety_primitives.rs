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

/// Round-4 primitive additions: CopyBytes byte views, SpscRing, and
/// KernelStackWindow (each: construction edge + bounds discipline).
pub fn test_safety_primitives_r4() {
    test_group_start("safety primitives r4");

    use crate::safety::bytes as cbytes;
    use crate::safety::frame::KernelStackWindow;
    use crate::safety::ring::SpscRing;

    // ---- CopyBytes: struct <-> byte image ----
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct DiskNode {
        magic: u32,
        count: u16,
        pad: u16,
    }
    let mut node = DiskNode { magic: 0xA1B2_C3D4, count: 7, pad: 0 };
    let img = cbytes::bytes_of(&node);
    if img.len() == 8 && img[0] == 0xD4 && img[4] == 0x07 {
        test_pass("copybytes bytes_of image");
    } else {
        test_fail("copybytes bytes_of image", "layout mismatch");
    }
    // Round-trip through a packed buffer at a misaligned offset.
    let mut buf = [0u8; 16];
    buf[3..11].copy_from_slice(cbytes::bytes_of(&node));
    let back: DiskNode = match cbytes::read_at(&buf, 3) {
        Some(v) => v,
        None => {
            test_fail("copybytes read_at", "refused in-bounds read");
            return;
        }
    };
    if back.magic == 0xA1B2_C3D4 && back.count == 7 {
        test_pass("copybytes read_at unaligned round-trip");
    } else {
        test_fail("copybytes read_at unaligned round-trip", "value mismatch");
    }
    if cbytes::read_at::<u64>(&buf, 9).is_none() {
        test_pass("copybytes read_at bounds refusal");
    } else {
        test_fail("copybytes read_at bounds refusal", "out-of-bounds accepted");
    }
    cbytes::write_at(&mut buf, 3, DiskNode { magic: 5, count: 6, pad: 0 });
    let img2 = cbytes::bytes_of_mut(&mut node);
    img2.copy_from_slice(&buf[3..11]);
    if node.magic == 5 && node.count == 6 {
        test_pass("copybytes write_at + bytes_of_mut");
    } else {
        test_fail("copybytes write_at + bytes_of_mut", "value mismatch");
    }
    if cbytes::from_bytes::<DiskNode>(&buf[0..2]).is_none() {
        test_pass("copybytes from_bytes short refusal");
    } else {
        test_fail("copybytes from_bytes short refusal", "short slice accepted");
    }

    // ---- SpscRing: IRQ-producer / CAS-consumer ----
    static RING: SpscRing<u8, 4> = SpscRing::new();
    RING.put(1);
    RING.put(2);
    if RING.depth_hint() == 2 {
        test_pass("spscring depth hint");
    } else {
        test_fail("spscring depth hint", "depth mismatch");
    }
    let got = (RING.get(), RING.get(), RING.get());
    if got == (Some(1), Some(2), None) {
        test_pass("spscring fifo + empty");
    } else {
        test_fail("spscring fifo + empty", "order mismatch");
    }
    for i in 0..6u8 {
        RING.put(i); // overflow: drops beyond capacity
    }
    let mut n = 0;
    while RING.get().is_some() {
        n += 1;
    }
    if n == 3 {
        // capacity 4: mask N-1 => at most N-1 buffered (3).
        test_pass("spscring drop-on-full respects capacity");
    } else {
        test_fail("spscring drop-on-full respects capacity", "count mismatch");
    }

    // ---- KernelStackWindow: bounds + alignment + frame walk ----
    let mut stack = [0u64; 16];
    let base = stack.as_mut_ptr() as usize;
    let top = base + stack.len() * 8;
    let win = match KernelStackWindow::new(base, top) {
        Some(w) => w,
        None => {
            test_fail("stack window construction", "refused valid window");
            return;
        }
    };
    if KernelStackWindow::new(top, base).is_none() {
        test_pass("stack window inversion refusal");
    } else {
        test_fail("stack window inversion refusal", "inverted accepted");
    }
    stack[2] = 0xFEED_FACE_BEEF_1234;
    if win.read_u64(base + 16) == Some(0xFEED_FACE_BEEF_1234)
        && win.read_u64(base + 4).is_none()
        && win.read_u64(top).is_none()
    {
        test_pass("stack window bounds/alignment");
    } else {
        test_fail("stack window bounds/alignment", "edge mismatch");
    }
    let mut frame = [0u64; 4];
    // sp=base+16 with a 4-word frame stays inside the 16-word window —
    // the walk fills all 4 slots and frame[0] is the marked word.
    let n = win.read_frame(base + 16, &mut frame);
    if n == 4 && frame[0] == 0xFEED_FACE_BEEF_1234 {
        test_pass("stack window frame walk in-bounds fill");
    } else {
        test_fail("stack window frame walk in-bounds fill", "walk mismatch");
    }
    // A walk starting at top - 16 crosses the top boundary after two
    // words — partial fill is the caller's stop signal.
    let n2 = win.read_frame(top - 16, &mut frame);
    if n2 == 2 {
        test_pass("stack window frame walk stops at top");
    } else {
        test_fail("stack window frame walk stops at top", "walk mismatch");
    }
}
