//! The security subsystem's ONE audited unsafe file (mainline
//! minimal-first consolidation, 2026-11-27): the two raw-edge families
//! the LSM framework still needs live here behind one audited item per
//! family; lsm.rs, cap_lsm.rs, capability.rs and mod.rs are
//! `#![forbid(unsafe_code)]` safe code over them.
//!
//! Edge families:
//! - the init-time static-mut LSM chain (INV-SEC-LSMCHAIN-1)
//! - the HookArgs raw credential pointers (INV-SEC-HOOKARGS-1)

use super::lsm::{HookArgs, HookId, LsmResult, LsmHooks, MAX_LSM_COUNT};
use crate::process::task::Cred;

/// INV-SEC-LSMCHAIN-1: `LSM_CHAIN` / `LSM_COUNT` are `static mut`
/// storage written ONLY from `security_init()` during kernel init
/// (single-threaded boot, before the first syscall and before SMP
/// tasks exist), and frozen for the rest of the run. Post-init reads
/// are therefore aliasing-free shared reads; the register path is the
/// one writer and runs init-time.
static mut LSM_CHAIN: [Option<&'static dyn LsmHooks>; MAX_LSM_COUNT] = [None; MAX_LSM_COUNT];
static mut LSM_COUNT: usize = 0;

/// Register an LSM module (sorted-insert, init-time only). Called from
/// `lsm::register_lsm` during `security_init()` — kernel init,
/// single-threaded (INV-SEC-LSMCHAIN-1).
pub(crate) fn chain_register(lsm: &'static dyn LsmHooks) {
    // SAFETY: INV-SEC-LSMCHAIN-1 — the single init-time writer,
    // single-threaded boot context.
    unsafe {
        if LSM_COUNT >= MAX_LSM_COUNT {
            crate::pr_err!("security: too many LSM modules, ignoring {}", lsm.name());
            return;
        }
        // Insert in sorted order (by order value).
        let mut pos = LSM_COUNT;
        for i in 0..LSM_COUNT {
            if lsm.order() < LSM_CHAIN[i].unwrap().order() {
                pos = i;
                break;
            }
        }
        // Shift entries right to make room.
        for i in (pos..LSM_COUNT).rev() {
            LSM_CHAIN[i + 1] = LSM_CHAIN[i];
        }
        LSM_CHAIN[pos] = Some(lsm);
        LSM_COUNT += 1;
    }
}

/// Number of registered LSMs (frozen after init — INV-SEC-LSMCHAIN-1).
pub(crate) fn chain_len() -> usize {
    // SAFETY: INV-SEC-LSMCHAIN-1 — frozen-after-init read.
    unsafe { LSM_COUNT }
}

/// True when the capability LSM is the only registered module (the boot
/// default). Backs the has_capability fast path: its Capable verdict
/// (the effective-set bit test) IS the chain verdict in that case.
///
/// (perf-gap-hunt) Read on the DAC hot path — one usize load.
pub(crate) fn chain_is_capability_only() -> bool {
    // SAFETY: INV-SEC-LSMCHAIN-1 — frozen-after-init read.
    unsafe { LSM_COUNT == 1 }
}

/// Dispatch a security hook through the registered LSM chain.
/// Returns 0 if all LSMs allow (or have no opinion), the first negative
/// result if any LSM denies. Read-only chain walk (INV-SEC-LSMCHAIN-1).
pub(crate) fn chain_dispatch(hook: HookId, args: &mut HookArgs) -> LsmResult {
    // SAFETY: INV-SEC-LSMCHAIN-1 — frozen-after-init shared read of the
    // hook table; the `&dyn` entries point at machine-lifetime statics.
    unsafe {
        for i in 0..LSM_COUNT {
            if let Some(lsm) = LSM_CHAIN[i] {
                let result = lsm.call(hook, args);
                if result < 0 {
                    return result;
                }
            }
        }
    }
    0
}

/// INV-SEC-HOOKARGS-1: `HookArgs.cred` / `.target_cred` raw pointers
/// reference creds owned by live tasks and are dereferenced only inside
/// the synchronous `security_hook_call` dispatch started by
/// `security_capable` / `security_signal_send`, which hold ordinary
/// `&Cred` borrows for the whole call — no hook can observe a freed
/// cred. Null means "no credential supplied"; the caller decides the
/// verdict for that.
///
/// Shared view of the hook's credential (None when unsupplied).
pub(crate) fn cred_ref<'a>(args: &'a HookArgs) -> Option<&'a Cred> {
    if args.cred.is_null() {
        return None;
    }
    // SAFETY: INV-SEC-HOOKARGS-1 above (live-cred, synchronous dispatch).
    Some(unsafe { &*args.cred })
}
