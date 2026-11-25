//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Capability LSM — always-loaded, order 0 (first in the chain).
//!
//! Implements the core capability checks that mirror Linux's
//! `security/commoncap.c`.  This LSM is the foundation of the
//! security subsystem and is always registered during boot.

use super::capability::*;
use super::lsm::*;
use crate::syscall::errno;

/// The capability LSM module.
pub struct CapLsm;

impl LsmHooks for CapLsm {
    fn name(&self) -> &'static str {
        "capability"
    }

    fn order(&self) -> u32 {
        0 // Always first
    }

    fn call(&self, hook: HookId, args: &mut HookArgs) -> LsmResult {
        match hook {
            HookId::Capable => {
                if args.cred.is_null() {
                    return -errno::EPERM;
                }
                let cred = unsafe { &*args.cred };
                if cred.cap_effective.has(args.cap) {
                    0
                } else {
                    -errno::EPERM
                }
            }
            HookId::SignalSend => {
                // CAP_KILL bypasses all signal permission checks.
                if args.cred.is_null() {
                    return 0; // No opinion — let caller handle null
                }
                let cred = unsafe { &*args.cred };
                if cred.cap_effective.has(CAP_KILL) {
                    0 // Allowed
                } else {
                    0 // No opinion — caller does UID-based check
                }
            }
            HookId::InodePermission => {
                // CAP_DAC_OVERRIDE is checked by generic_permission() directly.
                // This hook is a placeholder for future MAC modules.
                0
            }
            HookId::Execve => {
                // Capability transformation on execve is handled by
                // the execve code path directly (setuid/setgid logic).
                0
            }
            HookId::IpcPermission => {
                // CAP_IPC_OWNER is checked by ipc_check_permissions() directly.
                0
            }
            HookId::Mount | HookId::Umount => {
                // CAP_SYS_ADMIN is checked by mount/umount syscall handlers.
                0
            }
        }
    }
}
