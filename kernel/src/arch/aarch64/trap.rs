//! aarch64 trap — bring-up skeleton.
//!
//! Interface only; no implementation. Porting notes: vector table (VBAR_EL1), exception routing (sync/irq/fiq/serror × current/el0), SVC dispatch, page-fault decode from ESR/FAR.

pub fn placeholder() {
    unimplemented!("aarch64 backend bring-up: trap")
}
