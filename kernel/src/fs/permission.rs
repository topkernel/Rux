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
    // (perf-gap-hunt) Capability-only fast path: the capability LSM is
    // always registered (order 0) and is the ONLY module on the
    // boot-default chain; its Capable verdict is exactly the
    // effective-set bit test. When it is the sole registered LSM, both
    // capability checks below resolve to inline bit tests — no HookArgs
    // build + dyn dispatch per check, which the DAC hot path (the
    // full-path memo's per-ancestor validation) paid 2-4x per lookup.
    // Multi-LSM and pre-init (empty chain) configurations take the
    // original has_capability calls, byte-for-byte the old semantics.
    // The fast path lives HERE (one body) rather than in
    // has_capability (~100 call sites): the .text growth of inlining
    // it there shifted the syscall dispatch neighborhood and cost the
    // null row 3ns (measured, bisected).
    let mode = inode_mode as u32;
    let cap_only = crate::security::lsm::chain_is_capability_only();

    // CAP_DAC_OVERRIDE: root bypasses DAC. Linux grants read/write/search
    // on DIRECTORIES unconditionally; on files, execute still needs at
    // least one x bit (LTP access01 creates files inside 0222 dirs as
    // root — search must be overridden).
    let dac_override = if cap_only {
        cred.cap_effective.has(crate::security::CAP_DAC_OVERRIDE)
    } else {
        crate::security::has_capability(cred, crate::security::CAP_DAC_OVERRIDE)
    };
    if dac_override {
        let is_dir = mode & 0o170000 == 0o040000; // S_IFMT == S_IFDIR
        if mask & MAY_EXEC != 0 && !is_dir {
            // DAC_OVERRIDE: can exec only if at least one x bit is set
            if (mode & 0o111) == 0 {
                return false;
            }
        }
        return true;
    }

    // CAP_DAC_READ_SEARCH (L3 wiring, sec-r1): on DIRECTORIES it overrides
    // read/search when the mask carries no write bit — the reference
    // generic_permission grants CAP_DAC_READ_SEARCH only for
    // `!(mask & MAY_WRITE)` and only for S_ISDIR; plain files never pass
    // through this cap here (a file read needs CAP_DAC_OVERRIDE; the
    // cap's other file use, linkat AT_EMPTY_PATH, is not implemented).
    if mask & MAY_WRITE == 0 && mode & 0o170000 == 0o040000 {
        // S_IFDIR
        let read_search = if cap_only {
            cred.cap_effective.has(crate::security::CAP_DAC_READ_SEARCH)
        } else {
            crate::security::has_capability(
                cred,
                crate::security::CAP_DAC_READ_SEARCH,
            )
        };
        if read_search {
            return true;
        }
    }

    // Linux's generic_permission compares against the FILESYSTEM ids
    // (current_fsuid/current_fsgid), not euid/egid — setfsuid/setfsgid
    // change exactly these (LTP setfsuid04: fsuid to nobody then open a
    // 0644 root file O_RDWR must EACCES).
    if cred.fsuid == inode_uid {
        // Owner permission bits (bits 8-6)
        ((mode >> 6) & 0o7) & mask == mask
    } else if cred.fsgid == inode_gid || cred.groups.iter().any(|&g| g == inode_gid) {
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
