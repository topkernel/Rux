#![forbid(unsafe_code)]
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
    if crate::process::ns::in_init_mnt_ns() {
        let mut table = MOUNT_TABLE.lock();
        table.retain(|(_, mp, _, _)| mp != mount_point);
        table.push((
            String::from(device),
            String::from(mount_point),
            String::from(fs_type),
            String::from(flags),
        ));
    }
    crate::process::ns::ns_register_mount(device, mount_point, fs_type, flags);
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
    crate::process::ns::ns_get_mounts()
        .into_iter()
        .map(|e| (e.device, e.mount_point, e.fs_type, e.flags))
        .collect()
}

/// Same view as get_mounts(), keeping the rows' PERMANENT mount ids —
/// /proc/self/mountinfo numbers rows by struct mount.mnt_id, never by
/// output position (registry churn re-pairing positional ids with
/// different mounts made systemd's monitor see phantom changes).
pub fn get_mount_entries() -> Vec<crate::process::ns::MountEntry> {
    crate::process::ns::ns_get_mounts()
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

    // ext2/ext4 from a LOOP device: a second ext4 instance on the loop's
    // GenDisk (LTP mount/umount tests: losetup an mkfs.ext2 image, mount,
    // verify, umount). The boot root keeps its own instance.
    // "ext3" is the ext4 driver's alias (reference >= 4.3: ext3 mounts
    // through ext4). LTP's device tests pick their scratch fstype from a
    // probe that lands on ext3 whenever mkfs.ext3 exists — rejecting it
    // broke every loop-mounted LTP mount test that chose that name.
    // (Transplanted from 6a4d6733, lost in a later force-push.)
    if (fs_type == "ext2" || fs_type == "ext3" || fs_type == "ext4")
        && source.starts_with("/dev/loop")
    {
        let minor: Option<u32> = source["/dev/loop".len()..]
            .parse()
            .ok();
        if let Some(idx) = minor {
            let disk = crate::drivers::loop_dev::loop_disk(idx as usize)
                .ok_or(errno::Errno::NoSuchDevice.as_neg_i32())?;
            let (root, effective_ro) =
                crate::fs::ext4::mount_loop_instance(disk, mnt_flags.is_readonly())?;
            // Bring-up policy: the ext4 layer downgraded an rw request
            // (metadata_csum) — mirror it in the VFS mount flags so all
            // write paths fail EROFS at the VFS, not inside ext4.
            let mnt_flags = if effective_ro && !mnt_flags.is_readonly() {
                MntFlags::new(mnt_flags.bits() | MntFlags::MNT_READONLY)
            } else {
                mnt_flags
            };
            crate::fs::vfs::vfs_mount(target, root, mnt_flags);
            register_mount(
                source,
                target,
                fs_type,
                if mnt_flags.is_readonly() { "ro" } else { "rw" },
            );
            return Ok(());
        }
        return Err(errno::Errno::InvalidArgument.as_neg_i32());
    }

    match fs_type {
        "ext4" | "ext3" => {
            // ext3: ext4-driver alias (reference >= 4.3). (6a4d6733.)
            // SOURCE RESOLUTION (OH Phase 1b): the source names the disk —
            // "/dev/vdb", "/dev/block/vdc" (ueventd's node dir), or bare
            // "vdb" all resolve through the vdX letter. The named disk is
            // mounted as a SEPARATE instance (the loop-device discipline)
            // when a root ext4 is already live, so `mount /dev/block/vdb
            // /usr` on an initrd root gives /usr the SECOND disk, not a
            // second view of the first (the old code ignored the source
            // and always mounted the boot disk — silently wrong for every
            // multi-disk layout).
            let disk_of_source = |src: &str| -> Option<&'static crate::drivers::blkdev::GenDisk> {
                let trimmed = src
                    .trim_start_matches("/dev/block/")
                    .trim_start_matches("/dev/")
                    .trim_matches('/');
                crate::drivers::virtio::get_pci_gen_disk_by_name(trimmed)
            };

            if let Some(disk) = disk_of_source(source) {
                if !crate::fs::ext4::is_mounted() {
                    // First ext4 becomes THE root instance (full mount
                    // path, journal replay — the OH init mounting
                    // /dev/block/vdb before any other ext4).
                    crate::fs::ext4::mount_ext4_flags(disk, mnt_flags.is_readonly())
                        .map_err(|e| e as i32)?;
                    crate::fs::vfs::vfs_mount(
                        target,
                        crate::fs::ext4::create_root_inode(),
                        mnt_flags,
                    );
                } else {
                    // Additional disks: separate instance (mount_loop_
                    // instance discipline — no journal replay, like a
                    // fresh scratch mount; the OH images are clean).
                    let (root, effective_ro) = crate::fs::ext4::mount_loop_instance(
                        disk,
                        mnt_flags.is_readonly(),
                    )
                    .map_err(|e| e as i32)?;
                    let mnt_flags = if effective_ro && !mnt_flags.is_readonly() {
                        MntFlags::new(mnt_flags.bits() | MntFlags::MNT_READONLY)
                    } else {
                        mnt_flags
                    };
                    crate::fs::vfs::vfs_mount(target, root, mnt_flags);
                }
                register_mount(
                    source,
                    target,
                    fs_type,
                    if mnt_flags.is_readonly() { "ro" } else { "rw" },
                );
                return Ok(());
            }

            // No source disk identity (anonymous "ext4" mount, LTP-style
            // mount /dev/vda on the boot disk): the legacy boot-disk path.
            // R14-12 (F22): refuse to re-run mount_ext4 on a live
            // filesystem — a second `mount -t ext4` rebuilt the Ext4,
            // re-ran journal recovery on a live journal, swapped
            // GLOBAL_EXT4_FS under open files and leaked the old instance.
            if crate::fs::ext4::is_mounted() {
                return Err(errno::Errno::DeviceOrResourceBusy.as_neg_i32());
            }
            let fs = crate::fs::ext4::get_ext4_fs()
                .ok_or(errno::Errno::NoSuchDevice.as_neg_i32())?;
            // Mount ext4 if not already mounted
            if let Some(disk) = crate::drivers::virtio::get_pci_gen_disk() {
                crate::fs::ext4::mount_ext4(disk)
                    .map_err(|e| e as i32)?;
            } else if let Some(virtio_dev) = crate::drivers::virtio::get_device() {
                // get_device() hands out a &'static VirtIOBlkDevice
                // (VIRTIO_BLK static), so its disk field borrows for the
                // machine's lifetime.
                let disk: &'static crate::drivers::blkdev::GenDisk =
                    &virtio_dev.disk;
                crate::fs::ext4::mount_ext4(disk)
                    .map_err(|e| e as i32)?;
            } else {
                return Err(errno::Errno::NoSuchDevice.as_neg_i32());
            }
            let _ = fs; // already mounted
            crate::fs::vfs::vfs_mount(target, crate::fs::ext4::create_root_inode(), mnt_flags);
            register_mount("/dev/vda", target, fs_type, if mnt_flags.is_readonly() { "ro" } else { "rw" });
        }
        // devpts (OpenHarmony Phase 1): init mounts devpts on /dev/pts.
        // The kernel's pty devices live in devfs under pts/; an independent
        // tmpfs-style instance keeps each mount isolated while the devfs
        // /dev/pts directory (populated by pty allocation) stays the node
        // source when the target is /dev/pts. Tolerated options
        // (gid=,mode=,ptmxmode=,newinstance) are accepted and ignored.
        "devpts" => {
            crate::fs::tmpfs::ensure_registered();
            // devpts mount data (gid=/mode=/ptmxmode=/newinstance) carries
            // no page budget; pass it through so a hypothetical future
            // size= is honored too, same as tmpfs.
            let mut root_inode = crate::fs::tmpfs::create_mount_instance(data);
            crate::fs::tmpfs::tag_mount_instance_type(
                &mut root_inode,
                &crate::fs::tmpfs::DEVPTS_INODE_OPS,
            );
            crate::fs::vfs::vfs_mount(target, root_inode, mnt_flags);
            register_mount(fs_type, target, "devpts", if mnt_flags.is_readonly() { "ro" } else { "rw" });
        }
        // selinuxfs (OpenHarmony Phase 1): a minimal read-only pseudo fs so
        // mount("selinuxfs", "/sys/fs/selinux") succeeds and load_policy's
        // open(".../load"/".../enforce") paths find nodes. Permissive by
        // construction: "enforce" reads 0, "disable" reads 0; policy bytes
        // written to "load" are accepted and discarded. This is S2
        // insurance — OH init tolerates the mount failing, but a working
        // mount keeps its failure-path retries quiet.
        "selinuxfs" => {
            let root_inode = crate::fs::selinuxfs::create_root_inode();
            crate::fs::vfs::vfs_mount(target, root_inode, mnt_flags);
            register_mount(fs_type, target, "selinuxfs", if mnt_flags.is_readonly() { "ro" } else { "rw" });
        }
        "proc" | "procfs" => {
            crate::fs::procfs::mount_procfs()
                .map_err(|e| e as i32)?;
            crate::fs::vfs::vfs_mount(target, crate::fs::procfs::create_root_inode(), mnt_flags);
            register_mount(fs_type, target, "proc", if mnt_flags.is_readonly() { "ro" } else { "rw" });
        }
        "devfs" | "devtmpfs" => {
            crate::fs::devfs::init();
            if let Some(root_entry) = crate::fs::devfs::get_root_entry() {
                crate::fs::vfs::vfs_mount(target,
                    crate::fs::devfs::create_root_inode(&root_entry), mnt_flags);
                register_mount(fs_type, target, "devtmpfs", if mnt_flags.is_readonly() { "ro" } else { "rw" });
            } else {
                return Err(errno::Errno::NoSuchDevice.as_neg_i32());
            }
        }
        "sysfs" => {
            // U1a: sysfs device model + uevent. Build the KObject tree
            // (idempotent), prepare the /sys mountpoint, then link the
            // dentry tree.
            crate::fs::sysfs::init_sysfs()
                .map_err(|e| e as i32)?;
            crate::fs::sysfs::mount_sysfs()
                .map_err(|e| e as i32)?;
            crate::fs::vfs::vfs_mount(target, crate::fs::sysfs::create_root_inode(), mnt_flags);
            register_mount("sysfs", target, "sysfs", if mnt_flags.is_readonly() { "ro" } else { "rw" });
        }
        "cgroup2" | "cgroup" => {
            // U1b: cgroup v2 unified hierarchy. init is idempotent;
            // mount_cgroupfs links the dentry tree at the target.
            // "cgroup" (v1) mounts share the same tree — systemd's named
            // hierarchy "name=systemd" is the only v1 consumer on this
            // rootfs, and it just needs the mount to succeed so the
            // cgroup.procs interface works (noble-desktop, 2026-11-13).
            //
            // cgroup v1 controller parity (launcher-errno lane, OH 6.6
            // reference boot error stream): controller-bound v1 mounts
            // FAIL with EINVAL on the reference — its kernel exposes
            // the MEMORY controller only. OH init mounts /dev/cpuctl
            // cpu, /dev/cpuset cpuset, /dev/pids pids and /dev/frz
            // freezer (all EINVAL, logged and tolerated) plus
            // /dev/memcg memory (succeeds). Our old behavior accepted
            // every controller, so every follow-up chmod/chown/open row
            // diverged (real cgroupfs nodes answering EPERM vs plain
            // ENOENT on the reference's tmpfs dirs) — an ours-only
            // error family. Mirror the reference: only the memory
            // controller and generic mount tokens mount; any other
            // controller token answers EINVAL.
            if fs_type == "cgroup" {
                if let Some(opts) = data {
                    let disallowed = opts.split(',').any(|tok| {
                        let tok = tok.trim();
                        !tok.is_empty()
                            && tok != "memory"
                            && !tok.starts_with("name=")
                            && !matches!(
                                tok,
                                "nodev" | "noexec" | "nosuid" | "rw" | "ro" | "remount"
                            )
                    });
                    if disallowed {
                        return Err(errno::Errno::InvalidArgument.as_neg_i32());
                    }
                }
            }
            crate::fs::cgroup::init_cgroupfs()
                .map_err(|e| e as i32)?;
            crate::fs::cgroup::mount_cgroupfs(target)
                .map_err(|e| e as i32)?;
        }
        "tmpfs" | "shm" => {
            // P0-6: each mount creates an INDEPENDENT tmpfs instance
            // (own inode number space + icache fs_id). Register the fs type
            // so the registry-based mount path also accepts "tmpfs".
            crate::fs::tmpfs::ensure_registered();
            let root_inode = crate::fs::tmpfs::create_mount_instance(data);
            crate::fs::vfs::vfs_mount(target, root_inode, mnt_flags);
            register_mount("tmpfs", target, "tmpfs", if mnt_flags.is_readonly() { "ro" } else { "rw" });
        }
        // ramfs/rootfs: tmpfs-instance alias. systemd mounts ramfs on
        // /run/credentials/<unit> (target given as /proc/self/fd/N) at
        // every unit start; refusing it made every service log
        // "Failed to set up mount namespacing" and dropped credentials
        // dirs (gdm's first failure family).
        "ramfs" | "rootfs" => {
            crate::fs::tmpfs::ensure_registered();
            let root_inode = crate::fs::tmpfs::create_mount_instance(data);
            crate::fs::vfs::vfs_mount(target, root_inode, mnt_flags);
            register_mount(fs_type, target, fs_type, if mnt_flags.is_readonly() { "ro" } else { "rw" });
        }
        // POSIX mqueue: no queue semantics in Rux yet (mq_* syscalls
        // answer ENOSYS on their own); an empty tmpfs instance satisfies
        // dev-mqueue.mount so sysinit does not fail. mqueue(7) users that
        // actually open a queue surface the syscall gap unchanged.
        // securityfs: same empty-instance treatment -- sysinit's
        // sys-kernel-security.mount must not fail local-fs.
        "securityfs" => {
            crate::fs::tmpfs::ensure_registered();
            let mut root_inode = crate::fs::tmpfs::create_mount_instance(data);
            crate::fs::tmpfs::tag_mount_instance_type(
                &mut root_inode,
                &crate::fs::tmpfs::SECURITYFS_INODE_OPS,
            );
            crate::fs::vfs::vfs_mount(target, root_inode, mnt_flags);
            register_mount("securityfs", target, "securityfs", if mnt_flags.is_readonly() { "ro" } else { "rw" });
        }
        // configfs (launcher-errno lane): OH's storage_daemon.cfg mounts
        // configfs at /config and chowns /config/{hmdfs,sharefs} — on
        // the reference the mount succeeds (a second mount answers
        // EBUSY) and the two SUBSYSTEM dirs exist because the hmdfs /
        // sharefs drivers register configfs subsystems. Rux has neither
        // driver; the mount previously failed EINVAL, an ours-only
        // error family (mount + both chowns + every later row). Mount
        // a tmpfs instance tagged CONFIGFS_MAGIC, pre-populated with
        // the two subsystem dirs, so the chown/chmod path matches.
        // Divergence: userspace mkdir/rmdir inside the subsystem dirs
        // behaves like tmpfs, not configfs (no subsystem item
        // semantics) — no boot-path component on the OH image creates
        // configfs items (audited oh-robot).
        "configfs" => {
            crate::fs::tmpfs::ensure_registered();
            let mut root_inode = crate::fs::tmpfs::create_mount_instance(data);
            crate::fs::tmpfs::tag_mount_instance_type(
                &mut root_inode,
                &crate::fs::tmpfs::CONFIGFS_INODE_OPS,
            );
            // Pre-create the hmdfs/sharefs subsystem dirs (the
            // reference's driver-registered configfs groups) INSIDE the
            // instance, before it is published by the mount.
            if let Some(ops) = root_inode.ops {
                if ops.mkdir.is_some() {
                    // INV-VFSOPS-1 dispatcher (root_inode is the freshly
                    // created instance root; mkdir is the tmpfs mkdir
                    // twin).
                    let _ = ops.call_mkdir(
                        &root_inode,
                        b"hmdfs",
                        crate::fs::inode::InodeMode::new(0o755),
                        Err(crate::errno::Errno::ReadOnlyFileSystem),
                    );
                    let _ = ops.call_mkdir(
                        &root_inode,
                        b"sharefs",
                        crate::fs::inode::InodeMode::new(0o755),
                        Err(crate::errno::Errno::ReadOnlyFileSystem),
                    );
                }
            }
            crate::fs::vfs::vfs_mount(target, root_inode, mnt_flags);
            register_mount("configfs", target, "configfs", if mnt_flags.is_readonly() { "ro" } else { "rw" });
        }
        "mqueue" => {
            crate::fs::tmpfs::ensure_registered();
            let mut root_inode = crate::fs::tmpfs::create_mount_instance(data);
            // Type tag: statfs on the mount must report MQUEUE_MAGIC —
            // systemd verifies dev-mqueue.mount by statfs f_type AFTER
            // mount(2) and failed the unit even with mount(2) == 0
            // (ub2404 sysinit, the st_dev follow-on).
            crate::fs::tmpfs::tag_mount_instance_type(
                &mut root_inode,
                &crate::fs::tmpfs::MQUEUE_INODE_OPS,
            );
            crate::fs::vfs::vfs_mount(target, root_inode, mnt_flags);
            register_mount("mqueue", target, "mqueue", if mnt_flags.is_readonly() { "ro" } else { "rw" });
        }
        // debugfs/tracefs from userspace (sys-kernel-debug.mount /
        // sys-kernel-tracing.mount in the sysinit chain). The boot path
        // mounts the canonical instances; a userspace mount request
        // either mounts the boot instance at the requested point or, if
        // boot did not create it (feature-gated), creates it now.
        "debugfs" => {
            // Single-instance semantics (launcher-errno lane): the
            // kernel automounts debugfs at /sys/kernel/debug; on the OH
            // reference init's later `mount debugfs ... /sys/kernel/
            // debug` answers EBUSY (err 16 in the reference boot error
            // stream) and the kernel's own submounts under it — the
            // tracefs instance at /sys/kernel/debug/tracing — stay
            // visible. Our old arm stacked a second debugfs mount that
            // SHADOWED the tracefs alias (every /sys/kernel/debug/
            // tracing chmod then ENOENT — an ours-only error family).
            if mounted_as("debugfs", target) {
                return Err(errno::Errno::DeviceOrResourceBusy.as_neg_i32());
            }
            if crate::fs::debugfs::debugfs_root().is_none() {
                crate::fs::debugfs::init_debugfs()?;
            }
            let _ = crate::fs::vfs::path_lookup(target, 0);
            crate::fs::vfs::vfs_mount(
                target,
                crate::fs::debugfs::create_root_inode(),
                mnt_flags,
            );
            register_mount("debugfs", target, "debugfs", if mnt_flags.is_readonly() { "ro" } else { "rw" });
        }
        // pstore (why-slow lane): OH init.virt.cfg mounts pstore on
        // /sys/fs/pstore every boot; Linux's pstore fs is built-in, so
        // the reference's mount succeeds and ours answered EINVAL — an
        // ours-only error row in the boot error-stream diff. Mount the
        // EXISTING sysfs pstore subtree (pstore-lite already publishes
        // the previous boot's record there, named like the reference's
        // ramoops backends) so content survives the mount-over.
        "pstore" => {
            // Prefer the sysfs pstore subtree (pstore-lite record files);
            // fall back to an empty sysfs root when the lookup misses.
            let inode = match crate::fs::vfs::path_lookup("/sys/fs/pstore", 0) {
                Ok(p) => match p.inode {
                    Some(i) => i,
                    None => crate::fs::sysfs::create_root_inode(),
                },
                Err(_) => crate::fs::sysfs::create_root_inode(),
            };
            crate::fs::vfs::vfs_mount(target, inode, mnt_flags);
            register_mount(fs_type, target, "pstore", if mnt_flags.is_readonly() { "ro" } else { "rw" });
        }
        "tracefs" => {
            if crate::fs::tracefs::tracefs_root().is_none() {
                crate::fs::tracefs::init_tracefs()?;
            }
            let _ = crate::fs::vfs::path_lookup(target, 0);
            crate::fs::vfs::vfs_mount(
                target,
                crate::fs::tracefs::create_root_inode(),
                mnt_flags,
            );
            register_mount("tracefs", target, "tracefs", if mnt_flags.is_readonly() { "ro" } else { "rw" });
        }
        _ => return Err(errno::Errno::InvalidArgument.as_neg_i32()),
    }

    Ok(())
}
