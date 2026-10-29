//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! AArch64 IPI support — stage-3 stub surface.
//!
//! IPIs are GICv3 SGIs (ICC_SGI1R_EL1 generation, INTID 0-15). The
//! interface mirrors the riscv64 backend (PLIC SGI twin); the functions
//! are stage-3 work — until SMP lands, flush_tlb_others() is a local-TLB
//! fallback and senders are no-ops (single CPU: correct by construction).

/// Number of IPI types (riscv64 parity)
pub const NR_IPI_TYPES: usize = 5;

/// IPI types (riscv64 parity)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpiType {
    Reschedule = 0,
    CallFunction = 1,
    TlbShootdown = 2,
    Wakeup = 3,
    Stop = 4,
}

/// Register a handler for an IPI type — stage 3 (GIC SGI routing)
pub fn request_ipi(_ipi_type: IpiType, _handler: fn()) {}

/// Send an IPI to a target cpu — stage 3 (ICC_SGI1R_EL1)
pub fn send_ipi_type(_target: usize, _ipi_type: IpiType) {}

/// Send a reschedule IPI — stage 3
pub fn send_reschedule_ipi(_target_cpu: usize) {}

/// TLB shootdown broadcast. Single-CPU stage-1/2: the local TLB flush in
/// the caller covers the whole machine.
pub fn flush_tlb_others() {
    crate::arch::cpu::local_flush_tlb_all();
}

/// Software-IPI handler entry (stage 3)
pub fn handle_software_ipi(_cpu: usize) {}

/// Call-single-data (riscv64 parity shape)
pub struct CallSingleData {
    pub func: fn(*mut core::ffi::c_void),
    pub info: *mut core::ffi::c_void,
    pub done: core::sync::atomic::AtomicBool,
}

impl CallSingleData {
    pub const fn new() -> Self {
        Self {
            func: |_| {},
            info: core::ptr::null_mut(),
            done: core::sync::atomic::AtomicBool::new(false),
        }
    }
}

/// smp_call_function — stage 3
pub fn smp_call_function(
    _target: usize,
    func: fn(*mut core::ffi::c_void),
    info: *mut core::ffi::c_void,
) {
    // Single CPU: execute inline (correct, if unremote)
    func(info);
}

/// IRQ handler registration — stage 3
pub fn register_irq_handlers() {}

/// IPI handler (IRQ path) — stage 3
pub fn handle_ipi(_irq: usize, _cpu: usize) {}

/// IPI subsystem init — stage 3
pub fn init() {}
