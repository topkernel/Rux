//! Minimal selinuxfs — OpenHarmony Phase 1 insurance (S2).
//!
//! OH's first-stage init mounts selinuxfs at /sys/fs/selinux and libselinux
//! loads `policy.31` through it even in permissive mode. OH init tolerates
//! the mount failing, but a working mount keeps its failure-path retries
//! (and the boot-graph timeouts they feed) quiet. This module serves the
//! handful of nodes libselinux touches, permissively by construction:
//!
//! - `enforce`      reads "0\n"   (permissive; writes accepted, never enforced)
//! - `disable`      reads "0\n"   (SELinux is never disabled at runtime)
//! - `checkreqprot` reads "0\n"
//! - `policyvers`   reads 31 (the version OH 6.1 builds its policy with)
//! - `load`         accepts writes (policy bytes discarded); reads EOF
//! - `booleans`/`class`/`status` are present as empty directories
//!
//! Anything else is ENOENT — libselinux falls back gracefully.

use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;

use crate::errno;
use crate::fs::file::File;
use crate::fs::inode::{Inode, InodeMode, Ino, INodeOps, VfsDirEntry};
use crate::fs::FileOps;

const ROOT_INO: u64 = 1;

/// selinux_status_t (libselinux sestatus.c): the page mmap'd read-only by
/// `selinux_status_open()`. avc_open() → avc_init_internal() calls it with
/// fallback=0, so a failed mmap fails the whole avc_open and leaves
/// selinux_enabled=1 with avc_running=0 — a half-open AVC that makes every
/// samgr SA gate fail from a different direction. Static page: sequence
/// stays 0 (read_sequence always consistent), enforcing=0 (permissive),
/// deny_unknown=0 (allow unknown classes/perms — same answer as the node).
const STATUS_PAGE: [u8; 20] = {
    let mut p = [0u8; 20];
    p[0] = 1; // version = SELINUX_KERNEL_STATUS_VERSION
    p
};

enum Node {
    Const(&'static [u8]),
    /// Write sink (`load`): writes accepted and discarded, reads EOF.
    Sink,
    /// Write sink with a custom mode — `context` is opened O_RDWR by
    /// every libselinux `security_check_context` caller (samgr's SA
    /// permission gate runs as non-root), so it must be world-rw.
    SinkMode(u32),
    /// Present-but-empty directory.
    Dir,
}

const NODES: &[(&str, Node)] = &[
    ("booleans", Node::Dir),
    ("checkreqprot", Node::Const(b"0\n")),
    ("class", Node::Dir),
    // `context` — THE node samgr's SA-permission gate needs:
    // CheckPerm() calls security_check_context(callerSid), which opens
    // /sys/fs/selinux/context O_RDWR and writes the context string; the
    // write succeeding IS the check (b50: every GetServiceCheck failed
    // because selinux_mnt was NULL — statfs did not report the
    // SELINUX_MAGIC f_type). Accept and discard, like `load`.
    ("context", Node::SinkMode(0o666)),
    // `deny_unknown` — b51: with the context node fixed, samgr's
    // selinux_check_access() reached string_to_security_class
    // ("samgr_class"), which fails (no class/ discovery tree), and the
    // fallback asks security_deny_unknown(); ENOENT there returns -1,
    // which checkAccess.c treats as "deny" → the SA gate stayed shut.
    // "0" = allow unknown classes/permissions (permissive answer).
    ("deny_unknown", Node::Const(b"0\n")),
    ("disable", Node::Const(b"0\n")),
    ("enforce", Node::Const(b"0\n")),
    ("load", Node::Sink),
    // `mls` — is_selinux_mls_enabled(); "0" for a non-MLS policy.
    ("mls", Node::Const(b"0\n")),
    // `policy` — first-stage init chmods it to 0444 right after
    // mounting (a warning-only path, but a present node keeps it quiet).
    ("policy", Node::SinkMode(0o644)),
    ("policyvers", Node::Const(b"31\n")),
    // `status` — MUST be a regular file carrying the selinux_status_t
    // page: libselinux's selinux_status_open() mmaps it PROT_READ
    // MAP_SHARED; an empty directory node makes the mmap fail and takes
    // down avc_open() (see STATUS_PAGE above).
    ("status", Node::Const(&STATUS_PAGE)),
];

/// Child ino = 100 + index into NODES (stable within a boot).
fn child_ino(idx: usize) -> Ino {
    100 + idx as u64
}

fn node_index(name: &[u8]) -> Option<usize> {
    let s = core::str::from_utf8(name).ok()?;
    NODES.iter().position(|(n, _)| *n == s)
}

fn build_child_inode(idx: usize) -> Arc<Inode> {
    let (name, node) = &NODES[idx];
    let _ = name;
    let mode_bits = match node {
        Node::Dir => InodeMode::S_IFDIR | 0o555,
        Node::SinkMode(m) => InodeMode::S_IFREG | *m,
        // Const nodes must be WORLD-READABLE: file_open enforces DAC
        // (inode_permission, Linux may_open), and samgr's libselinux runs
        // as uid 5555 — a 0600 root-owned `enforce`/`deny_unknown`/
        // `status` open fails EACCES, which makes security_getenforce()
        // abort avc_open() and security_deny_unknown() return -1
        // (checkAccess.c reads -1 as "deny" → every SA gate fails; b60).
        // `load` stays root-writable (first-stage init writes the policy).
        Node::Const(_) => InodeMode::S_IFREG | 0o444,
        Node::Sink => InodeMode::S_IFREG | 0o644,
    };
    let mut inode = Inode::new(child_ino(idx), InodeMode::new(mode_bits));
    inode.ops = Some(&SELINUXFS_INODE_OPS);
    // Report the true byte size: file-backed mmap() faults SIGBUS at or
    // past inode size, so `status` must claim 20 bytes for libselinux's
    // selinux_status_open() mmap to be serviceable.
    if let Node::Const(d) = node {
        inode.set_size(d.len() as u64);
    }
    // private_data = index into NODES (as a small integer tag) — the read
    // path resolves the constant from it; no allocation to free.
    inode.private_data = Some(idx as *mut u8);
    Arc::new(inode)
}

/// First-touch-per-node trace mask: one bit per NODES entry, so a boot
/// log shows WHICH selinuxfs nodes each component reaches (and by which
/// pid) without spamming a line per poll. `context`/`access`/`status`
/// touches are the diagnostic for "does libselinux consult the kernel
/// selinuxfs at all" (samgr SA-gate forensics, boots b50+).
static NODE_TOUCHED: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

unsafe fn selinuxfs_lookup(dir: &Inode, name: &[u8]) -> Result<Ino, errno::Errno> {
    if dir.ino != ROOT_INO {
        // Class discovery probes (`class/<name>/index`) land here — log
        // the first few: they show which security classes libselinux
        // fails to discover (string_to_security_class → discover_class).
        static CLASS_PROBES: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
        use core::sync::atomic::Ordering::Relaxed;
        if CLASS_PROBES.fetch_add(1, Relaxed) < 20 {
            if let Ok(s) = core::str::from_utf8(name) {
                crate::pr_info!(
                    "selinuxfs: class lookup '{}' by pid {} -> ENOTDIR",
                    s,
                    crate::process::current_pid()
                );
            }
        }
        return Err(errno::Errno::NotADirectory); // child dirs are empty
    }
    match node_index(name) {
        Some(idx) => {
            use core::sync::atomic::Ordering::Relaxed;
            let mask = 1u64 << idx;
            if NODE_TOUCHED.fetch_or(mask, Relaxed) & mask == 0 {
                crate::pr_info!(
                    "selinuxfs: first lookup '{}' by pid {}",
                    NODES[idx].0,
                    crate::process::current_pid()
                );
            }
            Ok(child_ino(idx))
        }
        None => Err(errno::Errno::NoSuchFileOrDirectory), // ENOENT
    }
}

unsafe fn selinuxfs_iget(parent: &Inode, _name: &[u8], ino: Ino) -> Result<Arc<Inode>, errno::Errno> {
    if parent.ino != ROOT_INO {
        return Err(errno::Errno::NoSuchFileOrDirectory);
    }
    match ino.checked_sub(100) {
        Some(i) if (i as usize) < NODES.len() => Ok(build_child_inode(i as usize)),
        _ => Err(errno::Errno::NoSuchFileOrDirectory),
    }
}

unsafe fn selinuxfs_readdir(dir: &Inode) -> Option<Vec<VfsDirEntry>> {
    if dir.ino != ROOT_INO {
        return Some(Vec::new());
    }
    let mut out = vec![
        VfsDirEntry { ino: ROOT_INO, name: Vec::from(&b"."[..]), file_type: 4 },
        VfsDirEntry { ino: ROOT_INO, name: Vec::from(&b".."[..]), file_type: 4 },
    ];
    for (idx, (name, node)) in NODES.iter().enumerate() {
        out.push(VfsDirEntry {
            ino: child_ino(idx),
            name: Vec::from(name.as_bytes()),
            file_type: match node {
                Node::Dir => 4,   // DT_DIR
                _ => 8,           // DT_REG
            },
        });
    }
    Some(out)
}

unsafe fn selinuxfs_get_file_ops(inode: &Inode) -> Option<&'static FileOps> {
    if inode.mode.is_directory() {
        Some(&crate::fs::file::DIR_FILE_OPS)
    } else {
        Some(&SELINUXFS_FILE_OPS)
    }
}

/// No-op setattr: init chmods `policy` to 0444 right after mounting.
/// The default (no setattr op) answers EROFS, which init logs as a
/// boot-graph warning; a silent success keeps the graph clean. Mode
/// bits on synthetic nodes are cosmetic — nothing enforces them.
unsafe fn selinuxfs_setattr(_inode: &Inode, _attr: u32, _arg1: u64, _arg2: u64) -> i32 {
    0
}

/// pub(crate) for the statfs dispatch: statfs("/sys/fs/selinux") must
/// report f_type = SELINUX_MAGIC (0xf97cff8c) or libselinux's
/// verify_selinuxmnt() refuses to set selinux_mnt and every
/// security_check_context() fails with ENOENT (boot b50's SA-denial
/// loop: "GetServiceCheck callingSid:,SA:5100,ret:fail").
pub(crate) static SELINUXFS_INODE_OPS: INodeOps = INodeOps {
    lookup: Some(selinuxfs_lookup),
    iget: Some(selinuxfs_iget),
    create: None,
    link: None,
    unlink: None,
    symlink: None,
    mkdir: None,
    rmdir: None,
    mknod: None,
    rename: None,
    readlink: None,
    get_file_ops: Some(selinuxfs_get_file_ops),
    readdir: Some(selinuxfs_readdir),
    readdir_cacheable: false,
    open: None,
    permission: None,
    getattr: None,
    setattr: Some(selinuxfs_setattr),
    destroy_inode: None,
};

fn selinuxfs_read(file: &File, buf: &mut [u8]) -> isize {
    // private_data on the FILE is cloned from the inode at open time by
    // the generic path; when absent, fall back to the inode's tag.
    let tag = match unsafe { *file.private_data.get() } {
        Some(p) => p as usize,
        None => {
            let inode = match unsafe { (*file.inode.get()).clone() } {
                Some(i) => i,
                None => return -9, // EBADF
            };
            match inode.private_data {
                Some(p) => p as usize,
                None => return -9,
            }
        }
    };
    let data: &[u8] = match NODES.get(tag).map(|(_, n)| n) {
        Some(Node::Const(d)) => d,
        _ => &[],
    };
    let off = file.get_pos() as usize;
    let avail = data.len().saturating_sub(off);
    let n = buf.len().min(avail);
    buf[..n].copy_from_slice(&data[off..off + n]);
    file.set_pos((off + n) as u64);
    n as isize
}

fn selinuxfs_write(_file: &File, buf: &[u8]) -> isize {
    buf.len() as isize
}

/// Positional read (pread(2) + the mmap demand-fault fill). No shared
/// file-position swing (R71): libselinux's selinux_status_open() mmaps
/// `status` MAP_SHARED, and the fault path fills the page through
/// read_at — without this entry the fill served a zero page (version
/// byte 0 instead of STATUS_PAGE's 1).
fn selinuxfs_read_at(file: &File, offset: u64, buf: &mut [u8]) -> isize {
    let tag = match unsafe { *file.private_data.get() } {
        Some(p) => p as usize,
        None => match unsafe { (*file.inode.get()).clone() } {
            Some(i) => match i.private_data {
                Some(p) => p as usize,
                None => return -9,
            },
            None => return -9,
        },
    };
    let data: &[u8] = match NODES.get(tag).map(|(_, n)| n) {
        Some(Node::Const(d)) => d,
        _ => &[],
    };
    let off = offset as usize;
    let avail = data.len().saturating_sub(off);
    let n = buf.len().min(avail);
    buf[..n].copy_from_slice(&data[off..off + n]);
    n as isize
}

static SELINUXFS_FILE_OPS: FileOps = FileOps::builder()
    .read(selinuxfs_read)
    .write(selinuxfs_write)
    .read_at(selinuxfs_read_at)
    .build();

/// D-6 shape-test accessor: expose the private table to the unit-test
/// group `fileops-caps` without widening the runtime API.
#[cfg(any(test, feature = "unit-test"))]
pub mod caps_shape {
    pub fn selinuxfs_ops() -> &'static crate::fs::file::FileOps {
        &super::SELINUXFS_FILE_OPS
    }
}

/// Build the root inode for a selinuxfs mount.
pub fn create_root_inode() -> Arc<Inode> {
    let mut inode = Inode::new(ROOT_INO, InodeMode::new(InodeMode::S_IFDIR | 0o555));
    inode.ops = Some(&SELINUXFS_INODE_OPS);
    Arc::new(inode)
}
