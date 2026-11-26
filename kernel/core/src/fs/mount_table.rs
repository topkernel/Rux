//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! The rung-3 MOUNT SEAM (seed-core-design §4, crate-plan step 3).
//!
//! The VFS framework (mount.rs) must not reference filesystem leaves by
//! name; each leaf's mount logic lives leaf-side as a `mount_instance`
//! fn and registers its names here. `register_builtin_handlers()` runs
//! once at boot (lib.rs, before any mount syscall) — it is the ONE
//! framework-side place that names the leaves, i.e. the exact seam the
//! fs crate flip later replaces with each leaf crate's
//! self-registration. Behavior-identical to the old match arms.

#![forbid(unsafe_code)]
use super::mount::{self, MntFlags};

/// One leaf's mount entry point: performs the instance creation and
/// `vfs_mount`, and registers the mount row (the old match-arm body).
pub type MountFn = fn(source: &str, target: &str, fs_type: &str, data: Option<&str>, flags: MntFlags) -> Result<(), i32>;

const MAX_ENTRIES: usize = 32;

struct Entry {
    names: &'static [&'static str],
    f: MountFn,
}

static TABLE: crate::sync::types::Spinlock<[Option<Entry>; MAX_ENTRIES]> =
    crate::sync::types::Spinlock::new([const { None }; MAX_ENTRIES]);

/// Register `f` for every `names` alias (idempotent: a name registered
/// twice keeps its first entry — boot inits can run repeatedly).
pub fn register(names: &'static [&'static str], f: MountFn) {
    let mut t = TABLE.lock();
    for slot in t.iter_mut() {
        if slot.is_none() {
            *slot = Some(Entry { names, f });
            return;
        }
        if slot.as_ref().unwrap().names == names {
            return; // idempotent re-registration
        }
    }
    crate::pr_err!("mount_table: full ({} entries)", MAX_ENTRIES);
}

/// The framework's single lookup: O(entries).
pub fn lookup(fs_type: &str) -> Option<MountFn> {
    let t = TABLE.lock();
    for e in t.iter().flatten() {
        if e.names.contains(&fs_type) {
            return Some(e.f);
        }
    }
    None
}

/// The boot-time registration call (the crate-flip seam point). Each
/// leaf's `mount_instance` is the verbatim old match-arm body. EVERY
/// leaf's rows come from the service crates (fs via
/// LeafBootHooks.mount_rows_register, ext4 via its own row) and run
/// at THIS point — never at rust_main time (the mount table/allocator
/// are not up yet there).
pub fn register_builtin_handlers() {
    // The substrate/boot leaves' mount rows (tmpfs, procfs, devfs, sysfs,
    // cgroup, debugfs, tracefs, selinuxfs — fs-substrate tranche): ONE
    // entry from the fs crate, run at THIS boot point.
    (crate::api::fs::boot_hooks().mount_rows_register)();
    // The ext4 service crate's mount rows (rung 5): the SAME boot
    // point as the leaves above — NOT at rust_main registration time
    // (the fs 'R'-loop conviction; the row body self-guards).
    crate::api::ext4::mount_rows_register();
}

// Re-export for the leaf mount_instance bodies (register_mount row).
pub(crate) use mount::register_mount as row;
