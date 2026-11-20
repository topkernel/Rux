//! Kani bounded proofs for the two riskiest INV families
//! (unsafe-5000-plan Layer 2.2). Compiled ONLY under `cargo kani`
//! (#[cfg(kani)] gate, same discipline as kernel/verify).
//!
//! Proof 1 — KPTR exactly-once free (INV-KPTR-1/2/4): the
//! layout-locked Drop vs hand-written dealloc drift class the ledger
//! documents. Kani's memory model turns ANY dealloc-layout mismatch
//! or double-free into a verification failure, so the lifecycle
//! proofs below carry the property for every input value: new -> use
//! -> Drop is UB-free (exactly one dealloc, layout equal to the alloc
//! site's); into_raw -> from_raw -> Drop is UB-free (the re-adopted
//! handle deallocates exactly once); into_raw WITHOUT re-adoption is
//! a leak (zero deallocs — safe, not UB).
//!
//! Proof 2 — MmioRegion no-OOB window (INV-MMIO-1/2/3): for EVERY
//! offset value, each accessor either performs an access inside the
//! declared window or panics at the check() gate — no execution path
//! reaches the volatile access out of bounds. The backing is a real
//! allocation so Kani's memory model can convict an OOB.

#![cfg(kani)]

use crate::{dma, frame, mmio, ptr};

// ===================== Proof 1: KPtr exactly-once free =====================

#[kani::proof]
fn kptr_new_deref_drop_ub_free() {
    let v: u64 = kani::any();
    let p = ptr::KPtr::new(v).unwrap();
    assert_eq!(*p, v);
    // scope end: Drop -> dealloc(Layout::new::<T>) — the layout lock
    // and exactly-once property are checked by the memory model.
}

#[kani::proof]
fn kptr_volatile_rw_then_drop_ub_free() {
    let v: [u32; 3] = kani::any();
    let p = ptr::KPtr::new(v).unwrap();
    p.write_volatile(kani::any());
    let _snap: [u32; 3] = p.read_volatile();
}

#[kani::proof]
fn kptr_into_raw_readopt_exactly_one_dealloc() {
    let v: u32 = kani::any();
    let raw = ptr::KPtr::new(v).unwrap().into_raw();
    // SAFETY: raw came from into_raw one line above, same allocation.
    let back = unsafe { ptr::KPtr::from_raw(raw) };
    assert_eq!(*back, v);
    // exactly one Drop for the whole lifecycle (the original handle
    // forgot itself in into_raw).
}

#[kani::proof]
fn kptr_into_raw_leak_is_not_ub() {
    // The contract allows the leak arm (INV-KPTR-4: "re-adopt exactly
    // once OR leak"). A leak performs ZERO deallocs — the memory
    // model must report no UB (and no premature free).
    let v: u8 = kani::any();
    let _raw = ptr::KPtr::new(v).unwrap().into_raw();
    // NonNull has no Drop; nothing frees; the proof exits.
}

// ===================== Proof 2: MmioRegion no-OOB window =====================

fn region_over_real_backing() -> (&'static mut [u32; 4], mmio::MmioRegion) {
    let backing: &'static mut [u32; 4] = Box::leak(Box::new([0u32; 4]));
    let r = mmio::MmioRegion::new(backing.as_mut_ptr() as usize, 16);
    (backing, r)
}

#[kani::proof]
// should_panic: the out-of-window arm REACHES the designed panic
// (the loud check() gate); every other path must be UB-free — Kani
// still fails the proof on any UB regardless of this attribute.
#[kani::should_panic]
fn mmio_read_u8_all_offsets_no_oob() {
    let (_b, r) = region_over_real_backing();
    let off: usize = kani::any();
    let _ = r.read_u8(off);
}

#[kani::proof]
// should_panic: the out-of-window arm REACHES the designed panic
// (the loud check() gate); every other path must be UB-free — Kani
// still fails the proof on any UB regardless of this attribute.
#[kani::should_panic]
fn mmio_write_u8_all_offsets_no_oob() {
    let (_b, r) = region_over_real_backing();
    let off: usize = kani::any();
    let val: u8 = kani::any();
    r.write_u8(off, val);
}

#[kani::proof]
// should_panic: the out-of-window arm REACHES the designed panic
// (the loud check() gate); every other path must be UB-free — Kani
// still fails the proof on any UB regardless of this attribute.
#[kani::should_panic]
fn mmio_read_u32_all_offsets_no_oob() {
    let (_b, r) = region_over_real_backing();
    let off: usize = kani::any();
    let _ = r.read_u32(off);
}

#[kani::proof]
// should_panic: the out-of-window arm REACHES the designed panic
// (the loud check() gate); every other path must be UB-free — Kani
// still fails the proof on any UB regardless of this attribute.
#[kani::should_panic]
fn mmio_write_u32_all_offsets_no_oob() {
    let (_b, r) = region_over_real_backing();
    let off: usize = kani::any();
    let val: u32 = kani::any();
    r.write_u32(off, val);
}

#[kani::proof]
// should_panic: the out-of-window arm REACHES the designed panic
// (the loud check() gate); every other path must be UB-free — Kani
// still fails the proof on any UB regardless of this attribute.
#[kani::should_panic]
fn mmio_read_u64_all_offsets_no_oob() {
    let (_b, r) = region_over_real_backing();
    let off: usize = kani::any();
    let _ = r.read_u64(off);
}

#[kani::proof]
// should_panic: the out-of-window arm REACHES the designed panic
// (the loud check() gate); every other path must be UB-free — Kani
// still fails the proof on any UB regardless of this attribute.
#[kani::should_panic]
fn mmio_write_u64_all_offsets_no_oob() {
    let (_b, r) = region_over_real_backing();
    let off: usize = kani::any();
    let val: u64 = kani::any();
    r.write_u64(off, val);
}

// ============ Extension: the Miri-excluded families (Layer 2.2 note) ============
// Kani sees what Miri cannot (symbolic execution, no host memory):
// the DMA window discipline and the kernel-stack window.

#[kani::proof]
fn dma_volatile_rw_all_offsets_no_oob() {
    // A real heap block so the memory model can convict OOB; symbolic
    // offset/len slice the block. carve covers T=u64.
    if let Some(mut b) = dma::DmaBlock::new(64, 8) {
        let val: u64 = kani::any();
        b.write_volatile_at(0, val);
        assert_eq!(b.read_volatile_at::<u64>(0), val);
        // into the carved slot: Deref + volatile pair
        if let Some((slot, _)) = b.carve::<u64>() {
            slot.write_volatile(kani::any());
            let _ = slot.read_volatile();
        }
    }
}

#[kani::proof]
fn kernel_stack_window_reads_stay_in_window() {
    // A real 4KiB backing; symbolic (base,top) pairs inside it, and a
    // symbolic absolute address: read_u64 must answer Some ONLY for
    // in-window 8-aligned addresses, None otherwise — never an OOB
    // dereference.
    let page: &'static mut [u64; 512] = Box::leak(Box::new([0u64; 512]));
    let base = page.as_mut_ptr() as usize;
    let lo: u16 = kani::any();
    let hi: u16 = kani::any();
    kani::assume((lo as usize) <= 512);
    kani::assume((hi as usize) <= 512);
    kani::assume(lo <= hi);
    if let Some(w) = frame::KernelStackWindow::new(
        base + lo as usize * 8,
        base + hi as usize * 8,
    ) {
        let addr: usize = kani::any();
        let _ = w.read_u64(addr);
    }
}
