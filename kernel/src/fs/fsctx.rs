//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! fs_context + detached mounts — the "new mount API" family
//! (fsopen/fsconfig/fsmount/move_mount/open_tree/mount_setattr).
//!
//! Reference semantics (Linux fs/fsopen.c + fs/namespace.c), scoped to
//! the create→configure→attach lifecycle for the filesystem types this
//! system actually mounts (the keyctl model: reference-faithful where
//! callers look, no stub errno lies):
//!
//! - `fsopen(fstype, flags)` allocates an fs_context fd (FSOPEN_CLOEXEC
//!   is the only flag; unknown types are ENODEV, like get_fs_type).
//! - `fsconfig()` records parameters. Types converted to the new API in
//!   the reference (tmpfs/ramfs/proc/devpts/ext4) validate keys and fail
//!   EINVAL on unknown ones — systemd's `mount_option_supported()`
//!   probes exactly that shape (FSCONFIG_SET_FD with a bogus key must
//!   answer EINVAL, never succeed). Legacy types accept every key and
//!   accumulate a monolithic data string, like legacy_parse_param.
//!   FSCONFIG_CMD_CREATE builds the root instance (vfs_get_tree); a
//!   second CREATE or post-CREATE parameter is EBUSY.
//! - `fsmount(fsfd, flags, attr)` turns a created context into a
//!   DETACHED mount fd (struct mount, not in any namespace). attr flags
//!   map onto MntFlags; an uncreated context is EINVAL.
//! - `move_mount(from, to)` attaches a detached mount (or moves an
//!   attached one) at the target path. AT_EMPTY_PATH forms resolve
//!   through the fd's dentry (systemd/util-linux always pass "" +
//!   MOVE_MOUNT_F_EMPTY_PATH).
//! - `open_tree(path, OPEN_TREE_CLONE)` clones the mount covering the
//!   path into a detached mount fd (AT_RECURSIVE also clones submounts,
//!   reusing the MS_REC bind-clone machinery).
//! - `mount_setattr()` applies MOUNT_ATTR_* flag changes to the covering
//!   mount of a path or to a detached mount fd. size 0 (the systemd
//!   `mount_new_api_supported()` probe shape) is EINVAL, never success.
//!
//! Divergence note: mounts created here attach through the same
//! dentry/mount registry as mount(2) mounts (VfsMountInternal), so
//! /proc/mounts, umount2 and MS_MOVE see them identically.

use crate::fs::dentry::{Dentry, VfsMountInternal};
use crate::fs::file::{FileFlags, FileOps};
use crate::fs::inode::Inode;
use crate::fs::mount::MntFlags;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// UAPI constants (include/uapi/linux/mount.h, linux/fs.h)
// ---------------------------------------------------------------------------

pub const FSOPEN_CLOEXEC: u64 = 0x1;
pub const FSMOUNT_CLOEXEC: u64 = 0x1;
pub const OPEN_TREE_CLONE: u64 = 0x1;
pub const OPEN_TREE_CLOEXEC: u64 = 0x80000; // O_CLOEXEC

pub const FSCONFIG_SET_FLAG: u64 = 0;
pub const FSCONFIG_SET_STRING: u64 = 1;
pub const FSCONFIG_SET_BINARY: u64 = 2;
pub const FSCONFIG_SET_PATH: u64 = 3;
pub const FSCONFIG_SET_PATH_EMPTY: u64 = 4;
pub const FSCONFIG_SET_FD: u64 = 5;
pub const FSCONFIG_CMD_CREATE: u64 = 6;
pub const FSCONFIG_CMD_RECONFIGURE: u64 = 7;
/// uapi/linux/mount.h numbering (verified against mainline): 6 CREATE,
/// 7 RECONFIGURE, 8 CREATE_EXCL. There is no command 9 — unknown
/// commands answer EOPNOTSUPP like the reference default case.
pub const FSCONFIG_CMD_CREATE_EXCL: u64 = 8;

pub const MOUNT_ATTR_RDONLY: u64 = 0x0000_0001;
pub const MOUNT_ATTR_NOSUID: u64 = 0x0000_0002;
pub const MOUNT_ATTR_NODEV: u64 = 0x0000_0004;
pub const MOUNT_ATTR_NOEXEC: u64 = 0x0000_0008;
pub const MOUNT_ATTR__ATIME: u64 = 0x0000_0070;
pub const MOUNT_ATTR_NOATIME: u64 = 0x0000_0010;
pub const MOUNT_ATTR_STRICTATIME: u64 = 0x0000_0020;
pub const MOUNT_ATTR_NODIRATIME: u64 = 0x0000_0080;
pub const MOUNT_ATTR_IDMAP: u64 = 0x0010_0000;
pub const MOUNT_ATTR_NOSYMFOLLOW: u64 = 0x0020_0000;
/// Bits we can map onto internal mount state.
pub const MOUNT_ATTR_KNOWN_SET: u64 = MOUNT_ATTR_RDONLY
    | MOUNT_ATTR_NOSUID
    | MOUNT_ATTR_NODEV
    | MOUNT_ATTR_NOEXEC
    | MOUNT_ATTR__ATIME
    | MOUNT_ATTR_NODIRATIME
    | MOUNT_ATTR_NOSYMFOLLOW;

pub const MOVE_MOUNT_F_SYMLINKS: u64 = 0x1;
pub const MOVE_MOUNT_F_AUTOMOUNTS: u64 = 0x2;
pub const MOVE_MOUNT_F_EMPTY_PATH: u64 = 0x4;
pub const MOVE_MOUNT_T_SYMLINKS: u64 = 0x10;
pub const MOVE_MOUNT_T_AUTOMOUNTS: u64 = 0x20;
pub const MOVE_MOUNT_T_EMPTY_PATH: u64 = 0x40;
pub const MOVE_MOUNT_BENEATH: u64 = 0x80;
pub const MOVE_MOUNT_KNOWN: u64 = 0xF7;

// ---------------------------------------------------------------------------
// fs_context
// ---------------------------------------------------------------------------

/// Is `fs_type` accepted by fsopen()? (fsopen's get_fs_type; unknown
/// names answer ENODEV.) ext2/ext3 are the ext4 driver's aliases (the
/// classic-mount discipline).
pub fn fs_type_known(fs_type: &str) -> bool {
    matches!(
        fs_type,
        "tmpfs"
            | "shm"
            | "ramfs"
            | "rootfs"
            | "proc"
            | "procfs"
            | "sysfs"
            | "devpts"
            | "devfs"
            | "devtmpfs"
            | "mqueue"
            | "securityfs"
            | "selinuxfs"
            | "debugfs"
            | "tracefs"
            | "cgroup2"
            | "cgroup"
            | "ext2"
            | "ext3"
            | "ext4"
    )
}

/// Parameter key sets for the types converted to the new mount API in
/// the reference kernel (fs_parameter_spec tables). fsconfig fails
/// EINVAL for keys outside these + the universal set, which is exactly
/// what systemd's mount_option_supported() probe expects. Legacy types
/// accept everything (legacy_parse_param never rejects).
pub fn param_key_known(fs_type: &str, key: &str) -> bool {
    // Universal keys: "source" is vfs_parse_fs_param_source; the rest
    // are the common sb-flag keys of vfs_parse_sb_flag (common_set/
    // clear_sb_flag: sync/dirsync/lazytime/mand/ro/rw) — they apply to
    // every filesystem type (LTP fsconfig01 SET_PATH "sync",
    // fspick01 SET_STRING "sync").
    if matches!(key, "source" | "ro" | "rw" | "sync" | "dirsync" | "lazytime" | "mand") {
        return true;
    }
    match fs_type {
        // mm/shmem.c shmem_fs_parameters (+ramfs rootfs parameters).
        "tmpfs" | "shm" | "ramfs" | "rootfs" => matches!(
            key,
            "mode"
                | "uid"
                | "gid"
                | "size"
                | "nr_inodes"
                | "nr_blocks"
                | "huge"
                | "mpol"
                | "mpol_mode"
                | "mpol_nodelist"
                | "noswap"
                | "inode32"
                | "inode64"
                | "full_inodes"
        ),
        // fs/proc/root.c proc_fs_parameters.
        "proc" | "procfs" => matches!(key, "hidepid" | "subset" | "gid" | "only"),
        // fs/devpts/inode.c devpts_fs_parameters.
        "devpts" => matches!(
            key,
            "gid"
                | "uid"
                | "mode"
                | "ptmxmode"
                | "newinstance"
                | "max"
                | "reserve"
                | "nr_inodes"
                | "setgid"
        ),
        // fs/ext4/super.c ext4_fs_parameters (generous subset).
        "ext2" | "ext3" | "ext4" => true,
        // Legacy-parameter filesystems: accept everything.
        _ => true,
    }
}

/// The fs_context state machine (fs/fs_context.c). Only the states the
/// create pipeline needs are modelled: parameters before CREATE, a root
/// instance after it. `reconfigure` marks an fspick() context (purpose
/// FS_CONTEXT_FOR_RECONFIGURE): parameters are accepted until
/// FSCONFIG_CMD_RECONFIGURE (a no-op — the live mount IS the state),
/// CREATE/fsmount are refused like the reference's phase checks.
pub struct FsContext {
    pub fs_type: String,
    /// The "source" parameter (fsconfig SET_STRING "source") — the
    /// device/name shown in /proc/mounts.
    pub source: Option<String>,
    /// Recorded key[=value] parameters, in fsconfig order.
    pub params: Vec<(String, Option<String>)>,
    /// Monolithic option string for legacy types (joined like
    /// legacy_parse_param's buffer).
    pub legacy_data: String,
    /// sb_flags spellings ("ro" flag) recorded before CREATE.
    pub sb_readonly: bool,
    /// Root instance after FSCONFIG_CMD_CREATE (fc->root).
    pub root: Option<Arc<Inode>>,
    /// Effective read-only after CREATE (ext4 metadata_csum downgrades).
    pub effective_ro: bool,
    /// fspick() product: reconfigure-purpose context.
    pub reconfigure: bool,
    /// fspick() product: the live mount CMD_RECONFIGURE commits onto.
    pub target_mount: Option<Arc<VfsMountInternal>>,
}

impl FsContext {
    pub fn new(fs_type: &str) -> Self {
        Self {
            fs_type: String::from(fs_type),
            source: None,
            params: Vec::new(),
            legacy_data: String::new(),
            sb_readonly: false,
            root: None,
            effective_ro: false,
            reconfigure: false,
            target_mount: None,
        }
    }

    /// fspick() product: context over an existing mount.
    pub fn new_reconfigure(fs_type: &str, source: &str, root: Arc<Inode>) -> Self {
        Self {
            fs_type: String::from(fs_type),
            source: Some(String::from(source)),
            params: Vec::new(),
            legacy_data: String::new(),
            sb_readonly: false,
            root: Some(root),
            effective_ro: false,
            reconfigure: true,
            target_mount: None,
        }
    }

    /// The monolithic data string a classic mount(2) would have carried
    /// ("k=v,k=v" from the recorded params). tmpfs consumes size=/mode=;
    /// every other constructor accepts-and-ignores like do_mount.
    pub fn data_string(&self) -> Option<String> {
        if self.legacy_data.is_empty() {
            None
        } else {
            Some(self.legacy_data.clone())
        }
    }
}

// ---------------------------------------------------------------------------
// Detached mount object (fsmount / open_tree clone)
// ---------------------------------------------------------------------------

/// A mount fd's payload: the (possibly detached) VfsMountInternal plus
/// the bookkeeping /proc/mounts needs when it gets attached.
pub struct MountFd {
    pub mnt: Arc<VfsMountInternal>,
    /// Attached into the dentry tree already (move_mount happened)?
    pub attached: bool,
    /// For the /proc/mounts row at attach time.
    pub fs_type: String,
    pub source: String,
    /// open_tree(AT_RECURSIVE|OPEN_TREE_CLONE) submount clones, recorded
    /// at clone time and materialized INSIDE the mount root at
    /// move_mount-attach time (the MS_REC bind clone discipline: the
    /// deferred roots share the source mounts' inodes, same superblock).
    pub submounts: Vec<(Vec<String>, Arc<Dentry>, MntFlags)>,
    /// The ORIGINAL subtree root the submount rel-paths are relative to
    /// (open_tree's resolved source `anc`). The materialization walk's
    /// source-side cursor starts here — kernel-created mountpoints
    /// (/proc, /dev, /sys: dcache-only, no on-disk entries) resolve
    /// through it when the fresh root's disk tree has no placeholders
    /// (same fallback as the MS_REC bind clone, flip6).
    pub submount_src_base: Option<Arc<Dentry>>,
}

/// Build the root instance for a created fs_context (the per-type
/// constructors do_mount uses — same inodes, no attach). Returns
/// (root inode, effective read-only).
pub fn create_fs_root(
    fs_type: &str,
    source: Option<&str>,
    data: Option<&str>,
    ro: bool,
) -> Result<(Arc<Inode>, bool), i32> {
    let src = source.unwrap_or("");
    match fs_type {
        "tmpfs" | "shm" => {
            crate::fs::tmpfs::ensure_registered();
            Ok((crate::fs::tmpfs::create_mount_instance(data), false))
        }
        "ramfs" | "rootfs" => {
            crate::fs::tmpfs::ensure_registered();
            Ok((crate::fs::tmpfs::create_mount_instance(data), false))
        }
        "devpts" => {
            crate::fs::tmpfs::ensure_registered();
            let mut root = crate::fs::tmpfs::create_mount_instance(data);
            crate::fs::tmpfs::tag_mount_instance_type(
                &mut root,
                &crate::fs::tmpfs::DEVPTS_INODE_OPS,
            );
            Ok((root, false))
        }
        "securityfs" => {
            crate::fs::tmpfs::ensure_registered();
            let mut root = crate::fs::tmpfs::create_mount_instance(data);
            crate::fs::tmpfs::tag_mount_instance_type(
                &mut root,
                &crate::fs::tmpfs::SECURITYFS_INODE_OPS,
            );
            Ok((root, false))
        }
        "mqueue" => {
            crate::fs::tmpfs::ensure_registered();
            let mut root = crate::fs::tmpfs::create_mount_instance(data);
            crate::fs::tmpfs::tag_mount_instance_type(
                &mut root,
                &crate::fs::tmpfs::MQUEUE_INODE_OPS,
            );
            Ok((root, false))
        }
        "selinuxfs" => Ok((crate::fs::selinuxfs::create_root_inode(), false)),
        "proc" | "procfs" => {
            crate::fs::procfs::mount_procfs().map_err(|e| e as i32)?;
            Ok((crate::fs::procfs::create_root_inode(), false))
        }
        "sysfs" => {
            crate::fs::sysfs::init_sysfs().map_err(|e| e as i32)?;
            crate::fs::sysfs::mount_sysfs().map_err(|e| e as i32)?;
            Ok((crate::fs::sysfs::create_root_inode(), false))
        }
        "devfs" | "devtmpfs" => {
            crate::fs::devfs::init();
            match crate::fs::devfs::get_root_entry() {
                Some(e) => Ok((crate::fs::devfs::create_root_inode(&e), false)),
                None => Err(crate::errno::Errno::NoSuchDevice.as_neg_i32()),
            }
        }
        "debugfs" => {
            if crate::fs::debugfs::debugfs_root().is_none() {
                crate::fs::debugfs::init_debugfs()?;
            }
            Ok((crate::fs::debugfs::create_root_inode(), false))
        }
        "tracefs" => {
            if crate::fs::tracefs::tracefs_root().is_none() {
                crate::fs::tracefs::init_tracefs()?;
            }
            Ok((crate::fs::tracefs::create_root_inode(), false))
        }
        "cgroup2" | "cgroup" => {
            crate::fs::cgroup::init_cgroupfs().map_err(|e| e as i32)?;
            Ok((crate::fs::cgroup::create_root_inode(), false))
        }
        // ext2/ext3/ext4 from a loop device or a named vdX disk: the
        // separate-instance discipline (mount_loop_instance — no journal
        // replay, like a fresh scratch mount).
        "ext2" | "ext3" | "ext4" => {
            // "/dev/loopN"
            if src.starts_with("/dev/loop") {
                let idx: Option<u32> = src["/dev/loop".len()..].parse().ok();
                if let Some(idx) = idx {
                    let disk = crate::drivers::loop_dev::loop_disk(idx as usize)
                        .ok_or(crate::errno::Errno::NoSuchDevice.as_neg_i32())?;
                    let (root, effective_ro) =
                        crate::fs::ext4::mount_loop_instance(disk, ro)?;
                    return Ok((root, effective_ro));
                }
                return Err(crate::errno::Errno::InvalidArgument.as_neg_i32());
            }
            // Named disk ("/dev/vdb", "/dev/block/vdc", bare "vdb").
            let trimmed = src
                .trim_start_matches("/dev/block/")
                .trim_start_matches("/dev/")
                .trim_matches('/');
            let disk =
                crate::drivers::virtio::get_pci_gen_disk_by_name(trimmed).map(|d| d as *const _);
            if let Some(disk) = disk {
                let (root, effective_ro) =
                    crate::fs::ext4::mount_loop_instance(disk, ro)?;
                return Ok((root, effective_ro));
            }
            Err(crate::errno::Errno::NoSuchDevice.as_neg_i32())
        }
        _ => Err(crate::errno::Errno::InvalidArgument.as_neg_i32()),
    }
}

// ---------------------------------------------------------------------------
// File-descriptor plumbing
// ---------------------------------------------------------------------------

/// fs_context fd close: free the context.
fn fsctx_close(file: &crate::fs::File) -> i32 {
    // SAFETY: private_data holds a Box<FsContext> installed by sys_fsopen
    // (single owner, identity-checked via FSCTX_OPS).
    if let Some(p) = unsafe { *file.private_data.get() } {
        drop(unsafe { Box::from_raw(p as *mut FsContext) });
        unsafe { *file.private_data.get() = None };
    }
    0
}

/// File operations for fsopen() descriptors (I/O is EINVAL via the
/// missing-op table, like the reference anon inode).
pub static FSCTX_OPS: FileOps = FileOps::builder().close(fsctx_close).build();

/// Resolve a file's fs_context, if it is an fsopen() descriptor.
pub fn fsctx_of(file: &crate::fs::File) -> Option<&'static FsContext> {
    let ops = file.get_ops()?;
    if !core::ptr::eq(ops as *const FileOps, &FSCTX_OPS as *const FileOps) {
        return None;
    }
    // SAFETY: identity-checked above — private_data came from
    // Box::into_raw(FsContext) in sys_fsopen and lives until close.
    let p = (unsafe { *file.private_data.get() })?;
    Some(unsafe { &*(p as *const FsContext) })
}

/// Resolve a file's fs_context mutably (fsconfig/fsmount).
pub fn fsctx_of_mut(file: &crate::fs::File) -> Option<&'static mut FsContext> {
    let ops = file.get_ops()?;
    if !core::ptr::eq(ops as *const FileOps, &FSCTX_OPS as *const FileOps) {
        return None;
    }
    // SAFETY: same identity check; &mut is sound because syscall context
    // serializes per-fd operations (no concurrent fsconfig on one fd —
    // the reference holds fc->uapi_mutex).
    let p = (unsafe { *file.private_data.get() })?;
    Some(unsafe { &mut *(p as *mut FsContext) })
}

/// mount fd close: free the MountFd bookkeeping. An ATTACHED mount is
/// owned by the mount registry after move_mount — only the box goes; a
/// never-attached (detached) mount's VfsMountInternal drops with it
/// (nothing else references it; its root dentry shares the fs inode,
/// which stays alive through the fs instance).
fn mountfd_close(file: &crate::fs::File) -> i32 {
    // SAFETY: private_data holds a Box<MountFd> installed by
    // fsmount/open_tree (single owner, identity-checked via MOUNTFD_OPS).
    if let Some(p) = unsafe { *file.private_data.get() } {
        drop(unsafe { Box::from_raw(p as *mut MountFd) });
        unsafe { *file.private_data.get() = None };
    }
    0
}

/// File operations for fsmount()/open_tree(CLONE) descriptors.
pub static MOUNTFD_OPS: FileOps = FileOps::builder().close(mountfd_close).build();

/// Resolve a file's mount-fd payload, if it is an fsmount/open_tree
/// descriptor.
pub fn mountfd_of(file: &crate::fs::File) -> Option<&'static mut MountFd> {
    let ops = file.get_ops()?;
    if !core::ptr::eq(ops as *const FileOps, &MOUNTFD_OPS as *const FileOps) {
        return None;
    }
    // SAFETY: identity-checked above; syscall context per-fd.
    let p = (unsafe { *file.private_data.get() })?;
    Some(unsafe { &mut *(p as *mut MountFd) })
}

/// File operations for open_tree() WITHOUT OPEN_TREE_CLONE: an O_PATH
/// descriptor referring to the tree at the path (read/write/lseek fail
/// EBADF/EINVAL like O_PATH files; move_mount reads the dentry).
pub static TREEPATH_OPS: FileOps = FileOps::builder().build();

/// Is `file` an open_tree(no-clone) O_PATH descriptor? (move_mount's
/// from-fd arm and mount_setattr accept these.)
pub fn treepath_dentry(file: &crate::fs::File) -> Option<Arc<Dentry>> {
    let ops = file.get_ops()?;
    if !core::ptr::eq(ops as *const FileOps, &TREEPATH_OPS as *const FileOps) {
        return None;
    }
    // SAFETY: dentry written once at open_tree; read-only access.
    unsafe { &*file.dentry.get() }.clone()
}

/// Build a fresh detached mount whose root shares `root_inode`
/// (clone_mnt discipline: same superblock, new struct mount).
pub fn make_detached_mount(
    root_inode: Arc<Inode>,
    flags: MntFlags,
    fs_type: &str,
    source: &str,
    submounts: Vec<(Vec<String>, Arc<Dentry>, MntFlags)>,
    submount_src_base: Option<Arc<Dentry>>,
) -> Arc<crate::fs::File> {
    let mounted_root = Arc::new(Dentry::new(String::from("/")));
    mounted_root.set_inode(root_inode);
    mounted_root.set_hashed();
    let mnt = Arc::new(VfsMountInternal::new(mounted_root, flags));
    // O_PATH discipline: the descriptor names the mount, it carries no
    // read/write permission (read()/write() on it fail like the
    // reference's anon-inode O_PATH files).
    let file = Arc::new(crate::fs::File::new(FileFlags::new(FileFlags::O_PATH)));
    file.set_ops(&MOUNTFD_OPS);
    let boxed = Box::new(MountFd {
        mnt,
        attached: false,
        fs_type: String::from(fs_type),
        source: String::from(source),
        submounts,
        submount_src_base,
    });
    // SAFETY: single-owner handoff into the fd's private_data slot.
    unsafe {
        *file.private_data.get() = Some(Box::into_raw(boxed) as *mut u8);
    }
    file
}

/// Install a prepared file into the caller's fd table at the lowest
/// free slot; EMFILE when full. Sets FD_CLOEXEC when asked.
pub fn install_fd_with(file: Arc<crate::fs::File>, cloexec: bool) -> Result<usize, i32> {
    let fdtable = crate::sched::get_current_fdtable().ok_or(-crate::syscall::errno::EBADF)?;
    let fd = fdtable.alloc_fd().ok_or(-crate::syscall::errno::EMFILE)?;
    fdtable
        .install_fd(fd, file)
        .map_err(|_| -crate::syscall::errno::EMFILE)?;
    if cloexec {
        crate::fs::set_cloexec_fd(fd, true);
    }
    Ok(fd)
}

// ---------------------------------------------------------------------------
// MOUNT_ATTR ↔ MntFlags mapping
// ---------------------------------------------------------------------------

/// Apply MOUNT_ATTR_{attr_set,attr_clr} onto the current MntFlags
/// (mount_setattr's attribute computation). Bits in attr_set turn the
/// mapped internal flag ON, bits in attr_clr turn it OFF (attr_clr
/// wins on overlap, like do_mount_setattr's two-step apply). The atime
/// field is exclusive: NOATIME sets MNT_NOATIME, STRICTATIME/RELATIME
/// clear it; a set field value overrides a clear-field one.
/// NOSYMFOLLOW has no internal bit yet and is accepted-and-ignored
/// (module header; no caller-visible lie).
pub fn mount_attr_apply(cur: MntFlags, attr_set: u64, attr_clr: u64) -> MntFlags {
    let mut bits = cur.bits();
    let pair = |bits: &mut u64, s: u64, c: u64, internal: u64| {
        if attr_set & s != 0 {
            *bits |= internal;
        }
        if attr_clr & c != 0 {
            *bits &= !internal;
        }
    };
    pair(&mut bits, MOUNT_ATTR_RDONLY, MOUNT_ATTR_RDONLY, MntFlags::MNT_READONLY);
    pair(&mut bits, MOUNT_ATTR_NOSUID, MOUNT_ATTR_NOSUID, MntFlags::MNT_NOSUID);
    pair(&mut bits, MOUNT_ATTR_NODEV, MOUNT_ATTR_NODEV, MntFlags::MNT_NODEV);
    pair(&mut bits, MOUNT_ATTR_NOEXEC, MOUNT_ATTR_NOEXEC, MntFlags::MNT_NOEXEC);
    pair(
        &mut bits,
        MOUNT_ATTR_NODIRATIME,
        MOUNT_ATTR_NODIRATIME,
        MntFlags::MNT_NODIRATIME,
    );
    pair(
        &mut bits,
        MOUNT_ATTR_NOSYMFOLLOW,
        MOUNT_ATTR_NOSYMFOLLOW,
        MntFlags::MNT_NOSYMFOLLOW,
    );
    // The atime field: only a set-field value may RAISE noatime; both a
    // set-field relatime/strictatime value and any clear-field bits drop
    // it back to the default (relatime).
    let atime_set = attr_set & MOUNT_ATTR__ATIME;
    let atime_clr = attr_clr & MOUNT_ATTR__ATIME;
    if atime_set == MOUNT_ATTR_NOATIME && atime_clr & MOUNT_ATTR_NOATIME == 0 {
        bits |= MntFlags::MNT_NOATIME;
    } else if atime_set == MOUNT_ATTR_STRICTATIME || atime_set == 0 {
        if atime_set == MOUNT_ATTR_STRICTATIME || atime_clr != 0 {
            bits &= !MntFlags::MNT_NOATIME;
        }
    }
    MntFlags::new(bits)
}

/// fsmount()/mount_setattr() attr validation: unknown bits are EINVAL;
/// the atime field must carry exactly one selectable value; IDMAP is
/// rejected (no idmapped-mount support — userns_fd handling would lie).
pub fn validate_mount_attr(attr: u64) -> Result<(), i32> {
    if attr & !(MOUNT_ATTR_KNOWN_SET | MOUNT_ATTR_IDMAP) != 0 {
        return Err(-crate::syscall::errno::EINVAL);
    }
    if attr & MOUNT_ATTR_IDMAP != 0 {
        return Err(-crate::syscall::errno::EINVAL);
    }
    let atime = attr & MOUNT_ATTR__ATIME;
    if atime != 0 && atime != MOUNT_ATTR_NOATIME && atime != MOUNT_ATTR_STRICTATIME {
        return Err(-crate::syscall::errno::EINVAL);
    }
    Ok(())
}

/// mount_setattr() PAIR validation (fs/namespace.c mount_setattr's
/// build_mount_kattr): the MOUNT_ATTR_<atime> values are an enum, not a
/// bitmap — switching atime requires the FULL MOUNT_ATTR__ATIME field in
/// attr_clr, and attr_set's atime bits are only legal in that idiom;
/// attr_set atime bits without the clr field are EINVAL, as is a partial
/// clr field. This is the exact shape util-linux 2.39's libmount sends
/// for every classic-flag mount ("-o nosuid,nodev,noexec,relatime":
/// attr_set = NOSUID|NODEV|NOEXEC|RELATIME, attr_clr = MOUNT_ATTR__ATIME)
/// — validating set and clr independently made that idiom EINVAL and
/// each mount unit's mount(8) died on its post-mount flag pass
/// (sys-kernel-debug/tracing, dev-mqueue "Failed to mount" class).
/// MOUNT_ATTR_IDMAP stays rejected in attr_set (no idmapped mounts);
/// clearing it is a no-op here (nothing is ever idmapped).
pub fn validate_mount_attr_pair(attr_set: u64, attr_clr: u64) -> Result<(), i32> {
    if attr_set & !(MOUNT_ATTR_KNOWN_SET | MOUNT_ATTR_IDMAP) != 0
        || attr_clr & !(MOUNT_ATTR_KNOWN_SET | MOUNT_ATTR_IDMAP) != 0
    {
        return Err(-crate::syscall::errno::EINVAL);
    }
    if attr_set & MOUNT_ATTR_IDMAP != 0 {
        return Err(-crate::syscall::errno::EINVAL);
    }
    if attr_clr & MOUNT_ATTR__ATIME != 0 {
        if attr_clr & MOUNT_ATTR__ATIME != MOUNT_ATTR__ATIME {
            return Err(-crate::syscall::errno::EINVAL);
        }
        match attr_set & MOUNT_ATTR__ATIME {
            x if x == MOUNT_ATTR_RELATIME
                || x == MOUNT_ATTR_NOATIME
                || x == MOUNT_ATTR_STRICTATIME => {}
            _ => return Err(-crate::syscall::errno::EINVAL),
        }
    } else if attr_set & MOUNT_ATTR__ATIME != 0 {
        return Err(-crate::syscall::errno::EINVAL);
    }
    Ok(())
}

/// MOUNT_ATTR_RELATIME (the enum's zero value — "relatime" is the
/// default; a set-field value of 0 selects it explicitly).
pub const MOUNT_ATTR_RELATIME: u64 = 0x0000_0000;

/// MOUNT_ATTR bits → MntFlags for a fresh fsmount (attr_flags argument).
pub fn attr_flags_to_mnt(attr: u64) -> MntFlags {
    mount_attr_apply(MntFlags::new(0), attr, 0)
}
