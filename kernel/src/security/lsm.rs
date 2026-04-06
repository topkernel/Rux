//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Minimal LSM (Linux Security Module) hook framework.
//!
//! Provides a simple hook registration and dispatch mechanism that allows
//! multiple security modules to be stacked.  The capability LSM is always
//! loaded first (order 0).

/// Hook return value: 0 = allow, negative = deny.
pub type LsmResult = i32;

/// Security hook identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum HookId {
    /// Capability check (capable() / has_capability()).
    Capable = 0,
    /// Signal send permission.
    SignalSend = 1,
    /// Inode permission (DAC + MAC).
    InodePermission = 2,
    /// Binary execution (execve).
    Execve = 3,
    /// IPC object permission.
    IpcPermission = 4,
    /// Mount operation.
    Mount = 5,
    /// Umount operation.
    Umount = 6,
}

/// Hook arguments — a data bag for different hook types.
///
/// Each hook only reads/writes the fields it cares about; unused fields
/// are zeroed.  This avoids defining dozens of separate trait methods.
#[derive(Debug, Clone)]
pub struct HookArgs {
    pub hook_id: HookId,
    /// Current task's credentials.
    pub cred: *const crate::process::task::Cred,
    /// Target task's credentials (for signal, ptrace).
    pub target_cred: *const crate::process::task::Cred,
    /// Capability number (for HookId::Capable).
    pub cap: u32,
    /// Inode owner UID (for HookId::InodePermission).
    pub inode_uid: u32,
    /// Inode owner GID.
    pub inode_gid: u32,
    /// Inode mode bits.
    pub inode_mode: u16,
    /// Permission mask (MAY_READ/WRITE/EXEC).
    pub mask: u32,
    /// Signal number (for HookId::SignalSend).
    pub signal: i32,
}

impl HookArgs {
    /// Create HookArgs with safe defaults (null pointers, zeroed fields).
    pub fn new(hook_id: HookId) -> Self {
        Self {
            hook_id,
            cred: core::ptr::null(),
            target_cred: core::ptr::null(),
            cap: 0,
            inode_uid: 0,
            inode_gid: 0,
            inode_mode: 0,
            mask: 0,
            signal: 0,
        }
    }
}

impl Default for HookArgs {
    fn default() -> Self {
        Self::new(HookId::Capable)
    }
}

/// Trait that each LSM module must implement.
pub trait LsmHooks {
    /// Human-readable name (e.g., "capability").
    fn name(&self) -> &'static str;

    /// Dispatch order — lower values are called first.
    /// The capability LSM uses 0 to guarantee it is always first.
    fn order(&self) -> u32 {
        100
    }

    /// Dispatch a security hook.
    ///
    /// Return 0 to allow, negative (e.g. -EPERM) to deny.
    /// Return 0 for unimplemented hooks (no opinion).
    fn call(&self, hook: HookId, args: &mut HookArgs) -> LsmResult;
}

// ==================== Global LSM chain ====================

/// Maximum number of LSMs that can be registered.
const MAX_LSM_COUNT: usize = 4;

/// Registered LSM modules (ordered by priority).
static mut LSM_CHAIN: [Option<&'static dyn LsmHooks>; MAX_LSM_COUNT] = [None; MAX_LSM_COUNT];

/// Number of registered LSMs.
static mut LSM_COUNT: usize = 0;

/// Register an LSM module.  Called during `security_init()`.
///
/// # Safety
/// Must be called only during kernel init (single-threaded).
pub fn register_lsm(lsm: &'static dyn LsmHooks) {
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

/// Dispatch a security hook through the registered LSM chain.
///
/// Returns 0 if all LSMs allow (or have no opinion).
/// Returns the first negative result if any LSM denies.
pub fn security_hook_call(hook: HookId, args: &mut HookArgs) -> LsmResult {
    unsafe {
        for i in 0..LSM_COUNT {
            if let Some(lsm) = LSM_CHAIN[i] {
                let result = lsm.call(hook, args);
                if result < 0 {
                    return result;
                }
            }
        }
        0
    }
}

/// Check if a credential has the given capability, going through the LSM chain.
///
/// Returns 0 if the capability is granted, -EPERM if denied.
pub fn security_capable(cred: &crate::process::task::Cred, cap: u32) -> LsmResult {
    let mut args = HookArgs::new(HookId::Capable);
    args.cred = cred;
    args.cap = cap;
    security_hook_call(HookId::Capable, &mut args)
}

/// Check signal send permission through the LSM chain.
///
/// Returns 0 if allowed, -EPERM if denied.
pub fn security_signal_send(
    cred: &crate::process::task::Cred,
    target_cred: &crate::process::task::Cred,
    sig: i32,
) -> LsmResult {
    let mut args = HookArgs::new(HookId::SignalSend);
    args.cred = cred;
    args.target_cred = target_cred;
    args.signal = sig;
    security_hook_call(HookId::SignalSend, &mut args)
}

/// Return the number of registered LSMs (for diagnostics).
pub fn lsm_count() -> usize {
    unsafe { LSM_COUNT }
}
