//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! File permission checking.
//!
//! Implements the standard Unix DAC (Discretionary Access Control) check:
//! 1. Root (euid==0) bypasses most checks
//! 2. If euid matches file uid, use owner permission bits
//! 3. If egid matches file gid, use group permission bits
//! 4. Otherwise, use other permission bits

/// Permission mask bits
pub const MAY_EXEC: u32 = 0o001;
pub const MAY_WRITE: u32 = 0o002;
pub const MAY_READ: u32 = 0o004;

/// Check generic file permission.
///
/// Returns `true` if access is allowed, `false` if denied.
pub fn generic_permission(
    inode_mode: u16,
    inode_uid: u32,
    inode_gid: u32,
    mask: u32,
    cred: &crate::process::task::Cred,
) -> bool {
    // CAP_DAC_OVERRIDE: root bypasses DAC. Linux grants read/write/search
    // on DIRECTORIES unconditionally; on files, execute still needs at
    // least one x bit (LTP access01 creates files inside 0222 dirs as
    // root — search must be overridden).
    if crate::security::has_capability(cred, crate::security::CAP_DAC_OVERRIDE) {
        let mode = inode_mode as u32;
        let is_dir = mode & 0o170000 == 0o040000; // S_IFMT == S_IFDIR
        if mask & MAY_EXEC != 0 && !is_dir {
            // DAC_OVERRIDE: can exec only if at least one x bit is set
            if (mode & 0o111) == 0 {
                return false;
            }
        }
        return true;
    }

    let mode = inode_mode as u32;

    if cred.euid == inode_uid {
        // Owner permission bits (bits 8-6)
        ((mode >> 6) & 0o7) & mask == mask
    } else if cred.in_group(inode_gid) {
        // Group permission bits (bits 5-3) — checks egid + supplementary groups
        ((mode >> 3) & 0o7) & mask == mask
    } else {
        // Other permission bits (bits 2-0)
        (mode & 0o7) & mask == mask
    }
}

/// Check `mask` (MAY_READ/WRITE/EXEC) against `inode` using the current
/// task's credentials.
///
/// Returns `true` when there is no current task (early boot / kernel-thread
/// context) so internal lookups keep working.
pub fn inode_permission(inode: &crate::fs::inode::Inode, mask: u32) -> bool {
    match crate::sched::current() {
        Some(task) => {
            // SAFETY: `task` is the current task pointer from sched::current();
            // its cred is immutable for the duration of this check.
            let cred = unsafe { (*task).cred() };
            generic_permission(
                inode.mode.bits() as u16,
                inode.uid.load(core::sync::atomic::Ordering::Relaxed),
                inode.gid.load(core::sync::atomic::Ordering::Relaxed),
                mask,
                cred,
            )
        }
        None => true,
    }
}
