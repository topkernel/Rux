//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Mount point and namespace management
//!
//!
//! Core concepts:
//! - `VfsMount`: Per-mount metadata (used by procfs/rootfs for local state)
//! - `MntFlags`: Mount flags (read-only, noexec, etc.)
//! - `do_mount()`: Unified mount entry point for sys_mount

use alloc::sync::Arc;
use alloc::vec::Vec;
use alloc::string::String;
use crate::errno;
use crate::sync::spinlock::Spinlock;

/// Global mount registry: (device, mount_point, fs_type, flags_str).
/// U1c: still maintained (boot-time bookkeeping and init-ns seeding) but
/// the AUTHORITATIVE per-namespace view lives in the mount namespace
/// (process::ns::MountNamespace); get_mounts() reads that.
static MOUNT_TABLE: Spinlock<Vec<(String, String, String, String)>> = Spinlock::new(Vec::new());

/// Register a mount in the current mount namespace AND the global registry.
/// U1c: callers in a private (CLONE_NEWNS) namespace only mutate their own
/// table; the global registry stays the init-namespace baseline.
/// sc98: that contract is now ENFORCED — private-namespace mounts (OH
/// appspawn children's ~50 sandbox binds per spawn) used to push a row
/// into the global table too, where the O(rows) retain() per register
/// made every later mount slower while the launcher restart loop pumped
/// thousands of dead rows in.
/// sc115/sc116: the irqsave discipline is dropped for a plain preempt-off
/// lock. Every user is syscall context (mount/umount/procfs reads) — no
/// IRQ-context consumer exists — but the guarded section runs an O(rows)
/// STRING retain (per-byte compare loop) plus four heap allocations, and
/// under allocator contention that section was measured as a >=1 s cli
/// window (WW IRQ-OFF-WINDOW samples landing in atomic_load<u8> string
/// compares; one boot printed 637 >=100 ms inter-tick gaps). The ns.rs
/// namespace tables already use plain .lock(); MOUNT_TABLE now matches.
pub fn register_mount(device: &str, mount_point: &str, fs_type: &str, flags: &str) {
    if crate::process::hotpath::in_init_mnt_ns() {
        let mut table = MOUNT_TABLE.lock();
        table.retain(|(_, mp, _, _)| mp != mount_point);
        table.push((
            String::from(device),
            String::from(mount_point),
            String::from(fs_type),
            String::from(flags),
        ));
    }
    crate::process::hotpath::ns_register_mount(device, mount_point, fs_type, flags);
}

/// Is `target` already registered as a mount of filesystem type
/// `fs_type`? (launcher-errno lane: the single-instance filesystems the
/// kernel automounts — debugfs at /sys/kernel/debug — answer EBUSY to a
/// second mount(2) at the same point on the OH reference; a stacked
/// second mount would shadow the kernel's own submounts, e.g. the
/// tracefs instance at /sys/kernel/debug/tracing.)
pub fn mounted_as(fs_type: &str, target: &str) -> bool {
    let table = MOUNT_TABLE.lock();
    table
        .iter()
        .any(|(_, mp, f, _)| f == fs_type && mp == target)
}

/// Raw global registry snapshot + the hardcoded rootfs row (init-namespace
/// seed for process::ns::init_mnt_ns).
pub fn get_global_mounts() -> Vec<(String, String, String, String)> {
    let mut result = Vec::new();
    // Always include rootfs (registered before allocator is fully stable)
    result.push((String::from("rootfs"), String::from("/"), String::from("rootfs"), String::from("rw")));
    // Add dynamically registered mounts
    let table = MOUNT_TABLE.lock();
    for (d, m, f, fl) in table.iter() {
        result.push((d.clone(), m.clone(), f.clone(), fl.clone()));
    }
    result
}

/// Get the mounts visible to the CALLER (U1c: the current mount namespace;
/// the init namespace table is seeded from the boot registry).
pub fn get_mounts() -> Vec<(String, String, String, String)> {
    crate::process::hotpath::ns_get_mounts()
        .into_iter()
        .map(|e| (e.device, e.mount_point, e.fs_type, e.flags))
        .collect()
}

/// Same view as get_mounts(), keeping the rows' PERMANENT mount ids —
/// /proc/self/mountinfo numbers rows by struct mount.mnt_id, never by
/// output position (registry churn re-pairing positional ids with
/// different mounts made systemd's monitor see phantom changes).
pub fn get_mount_entries() -> Vec<crate::process::types::MountEntry> {
    crate::process::hotpath::ns_get_mounts()
}

#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct MntFlags(u64);

impl MntFlags {
    pub const MNT_READONLY: u64 = 0x01;
    pub const MNT_NOATIME: u64 = 0x02;
    pub const MNT_NODIRATIME: u64 = 0x04;
    pub const MNT_SYNCHRONOUS: u64 = 0x08;
    pub const MNT_NOEXEC: u64 = 0x10;
    pub const MNT_NOSUID: u64 = 0x20;
    pub const MNT_NODEV: u64 = 0x40;
    pub const MNT_PRIVATE: u64 = 0x80;
    pub const MNT_SHARED: u64 = 0x100;
    pub const MNT_SLAVE: u64 = 0x200;
    pub const MNT_UNBINDABLE: u64 = 0x400;
    pub const MNT_FORCE: u64 = 0x800;
    /// SB_NOSYMFOLLOW (mount_setattr MOUNT_ATTR_NOSYMFOLLOW, 5.10+).
    pub const MNT_NOSYMFOLLOW: u64 = 0x1000;

    pub fn new(flags: u64) -> Self {
        Self(flags)
    }

    pub fn is_readonly(&self) -> bool {
        (self.0 & Self::MNT_READONLY) != 0
    }

    pub fn is_noexec(&self) -> bool {
        (self.0 & Self::MNT_NOEXEC) != 0
    }

    pub fn is_nosuid(&self) -> bool {
        (self.0 & Self::MNT_NOSUID) != 0
    }

    pub fn bits(&self) -> u64 {
        self.0
    }
}

/// Per-mount metadata (used by individual filesystems for local state).
#[repr(C)]
pub struct VfsMount {
    pub mnt_id: u64,
    pub mnt_flags: MntFlags,
    pub mnt_mountpoint: Option<Arc<Vec<u8>>>,
    pub mnt_root: Option<Arc<Vec<u8>>>,
    pub mnt_sb: Option<*mut u8>,
    mnt_count: core::sync::atomic::AtomicU64,
    mnt_expired: core::sync::atomic::AtomicU64,
}

unsafe impl Send for VfsMount {}
unsafe impl Sync for VfsMount {}

impl VfsMount {
    pub fn new(mountpoint: Vec<u8>, root: Vec<u8>, flags: MntFlags, sb: Option<*mut u8>) -> Self {
        Self {
            mnt_id: 0,
            mnt_flags: flags,
            mnt_mountpoint: Some(Arc::new(mountpoint)),
            mnt_root: Some(Arc::new(root)),
            mnt_sb: sb,
            mnt_count: core::sync::atomic::AtomicU64::new(1),
            mnt_expired: core::sync::atomic::AtomicU64::new(0),
        }
    }

    pub fn get_superblock(&self) -> Option<*mut u8> {
        self.mnt_sb
    }

    pub fn set_superblock(&mut self, sb: *mut u8) {
        self.mnt_sb = Some(sb);
    }
}

// ============================================================================
// Unified mount entry point
// ============================================================================

/// Linux MS_* mount flags (UAPI subset).
const MS_RDONLY: u64 = 1;
const MS_NOSUID: u64 = 2;
const MS_NODEV: u64 = 4;
const MS_NOEXEC: u64 = 8;
const MS_SYNCHRONOUS: u64 = 16;
const MS_NOATIME: u64 = 1024;

/// Translate Linux MS_* flags into the internal MntFlags (review 5.1:
/// MNT_* flags 存储 — do_mount used to drop the flags entirely).
fn translate_mnt_flags(ms_flags: u64) -> MntFlags {
    let mut bits = 0u64;
    if ms_flags & MS_RDONLY != 0 { bits |= MntFlags::MNT_READONLY; }
    if ms_flags & MS_NOSUID != 0 { bits |= MntFlags::MNT_NOSUID; }
    if ms_flags & MS_NODEV != 0 { bits |= MntFlags::MNT_NODEV; }
    if ms_flags & MS_NOEXEC != 0 { bits |= MntFlags::MNT_NOEXEC; }
    if ms_flags & MS_SYNCHRONOUS != 0 { bits |= MntFlags::MNT_SYNCHRONOUS; }
    if ms_flags & MS_NOATIME != 0 { bits |= MntFlags::MNT_NOATIME; }
    MntFlags::new(bits)
}

/// Public wrapper for sys_mount's MS_REMOUNT path (flags translation without
/// re-running the filesystem mount callback).
pub fn translate_mnt_flags_public(ms_flags: u64) -> MntFlags {
    translate_mnt_flags(ms_flags)
}

/// Perform a real mount: call the filesystem's mount callback, then build
/// the dentry tree via `vfs_mount()`.
///
/// `data` is the mount(2) options string (may be None). Only tmpfs
/// consumes it today (its `size=` page budget); other filesystems accept
/// and ignore it, like a filesystem with no .parse_monolithic hook.
///
/// This is the single entry point for both boot-time mounts and sys_mount().
pub fn do_mount(
    source: &str,
    target: &str,
    fs_type: &str,
    flags: u64,
    data: Option<&str>,
) -> Result<(), i32> {
    let mnt_flags = translate_mnt_flags(flags);

    // Rung-3 MOUNT SEAM: consult the registration table first; the
    // leaf's mount_instance holds the old arm's body verbatim. The
    // ext family joined at rung 5 (its row carries the loop-device
    // special case, the named-disk resolution and the legacy boot-disk
    // path — the old arms below, crate-side).
    if let Some(mount_fn) = crate::fs::mount_table::lookup(fs_type) {
        return mount_fn(source, target, fs_type, data, mnt_flags);
    }

    match fs_type {
        // POSIX mqueue: no queue semantics in Rux yet (mq_* syscalls
        // answer ENOSYS on their own); an empty tmpfs instance satisfies
        // dev-mqueue.mount so sysinit does not fail. mqueue(7) users that
        // actually open a queue surface the syscall gap unchanged.
        // securityfs: same empty-instance treatment -- sysinit's
        // sys-kernel-security.mount must not fail local-fs.
        // pstore (why-slow lane): OH init.virt.cfg mounts pstore on
        // /sys/fs/pstore every boot; Linux's pstore fs is built-in, so
        // the reference's mount succeeds and ours answered EINVAL — an
        // ours-only error row in the boot error-stream diff. Mount the
        // EXISTING sysfs pstore subtree (pstore-lite already publishes
        // the previous boot's record there, named like the reference's
        // ramoops backends) so content survives the mount-over.
        _ => return Err(errno::Errno::InvalidArgument.as_neg_i32()),
    }

    Ok(())
}
