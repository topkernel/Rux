//! aarch64 cpu — bring-up skeleton.
//!
//! Interface only; no implementation. Porting notes: cpu primitives: cache sync (dc cvau/ic ivau), barriers (dsb/dmb/isb), TLB ops (tlbi vmalle1), dsb(), current via per-CPU TPIDR_EL1.

pub fn placeholder() {
    unimplemented!("aarch64 backend bring-up: cpu")
}
