//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Security subsystem — capabilities and LSM framework.
//!
//! This module provides:
//! - POSIX.1e capability constants and the `Cap` bitmask type
//! - A minimal LSM (Linux Security Module) hook framework
//! - The `capable()` and `has_capability()` public API

pub mod capability;
pub mod lsm;
pub mod cap_lsm;

// Re-export commonly used items for convenience.
pub use capability::*;
pub use lsm::{security_capable, security_hook_call, security_signal_send, HookArgs, HookId, LsmHooks};

// ==================== Public API ====================

/// Check if the current task has the given capability.
///
/// This is the primary API used by syscall handlers throughout the kernel.
/// Goes through the LSM chain (capability LSM checks `cap_effective`).
///
/// # Example
/// ```ignore
/// if !crate::security::capable(crate::security::CAP_SYS_ADMIN) {
///     return -errno::EPERM as u64;
/// }
/// ```
#[inline]
pub fn capable(cap: u32) -> bool {
    match crate::sched::current() {
        Some(task) => {
            let cred = task.cred();
            security_capable(cred, cap) == 0
        }
        None => false,
    }
}

/// Check if a specific credential has the given capability.
///
/// Used when checking permissions on behalf of another context
/// (e.g., checking file opener's credentials).
#[inline]
pub fn has_capability(cred: &crate::process::task::Cred, cap: u32) -> bool {
    security_capable(cred, cap) == 0
}

/// Check if the current task can send a signal to the target task.
///
/// Returns true if:
/// - Same UID (euid/uid match), or
/// - CAP_KILL is set in current task's effective capabilities.
#[inline]
pub fn can_send_signal(target_cred: &crate::process::task::Cred) -> bool {
    match crate::sched::current() {
        Some(task) => {
            let cred = task.cred();
            // Same UID check
            if cred.euid == target_cred.euid
                || cred.uid == target_cred.uid
                || cred.suid == target_cred.uid
            {
                return true;
            }
            // CAP_KILL bypass
            has_capability(cred, CAP_KILL)
        }
        None => false,
    }
}

// ==================== Initialization ====================

/// Initialize the security subsystem.
///
/// Called during early boot to register the capability LSM.
/// Must be called before any user processes are created.
pub fn security_init() {
    static mut INIT_DONE: bool = false;
    unsafe {
        if INIT_DONE {
            return;
        }
        INIT_DONE = true;
    }

    lsm::register_lsm(&cap_lsm::CapLsm);
    crate::print_status("security", "capability LSM initialized", true);
}
