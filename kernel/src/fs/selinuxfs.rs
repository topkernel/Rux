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

use crate::fs::file::File;
use crate::fs::inode::{Inode, InodeMode, Ino, INodeOps, VfsDirEntry};
use crate::fs::FileOps;

const ROOT_INO: u64 = 1;

enum Node {
    Const(&'static [u8]),
    /// Write sink (`load`): writes accepted and discarded, reads EOF.
    Sink,
    /// Present-but-empty directory.
    Dir,
}

const NODES: &[(&str, Node)] = &[
    ("booleans", Node::Dir),
    ("checkreqprot", Node::Const(b"0\n")),
    ("class", Node::Dir),
    ("disable", Node::Const(b"0\n")),
    ("enforce", Node::Const(b"0\n")),
    ("load", Node::Sink),
    ("policyvers", Node::Const(b"31\n")),
    ("status", Node::Dir),
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
    let mut inode = match node {
        Node::Dir => Inode::new(child_ino(idx), InodeMode::new(InodeMode::S_IFDIR | 0o555)),
        Node::Const(_) | Node::Sink => {
            Inode::new(child_ino(idx), InodeMode::new(InodeMode::S_IFREG | 0o600))
        }
    };
    inode.ops = Some(&SELINUXFS_INODE_OPS);
    // private_data = index into NODES (as a small integer tag) — the read
    // path resolves the constant from it; no allocation to free.
    inode.private_data = Some(idx as *mut u8);
    Arc::new(inode)
}

unsafe fn selinuxfs_lookup(dir: &Inode, name: &[u8]) -> Result<Ino, i32> {
    if dir.ino != ROOT_INO {
        return Err(-20); // ENOTDIR: child dirs are empty
    }
    match node_index(name) {
        Some(idx) => Ok(child_ino(idx)),
        None => Err(-2), // ENOENT
    }
}

unsafe fn selinuxfs_iget(parent: &Inode, _name: &[u8], ino: Ino) -> Result<Arc<Inode>, i32> {
    if parent.ino != ROOT_INO {
        return Err(-2);
    }
    match ino.checked_sub(100) {
        Some(i) if (i as usize) < NODES.len() => Ok(build_child_inode(i as usize)),
        _ => Err(-2),
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

static SELINUXFS_INODE_OPS: INodeOps = INodeOps {
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
    open: None,
    permission: None,
    getattr: None,
    setattr: None,
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

static SELINUXFS_FILE_OPS: FileOps = FileOps {
    read: Some(selinuxfs_read),
    write: Some(selinuxfs_write),
    lseek: None,
    close: None,
    poll: None,
    read_at: None,
};

/// Build the root inode for a selinuxfs mount.
pub fn create_root_inode() -> Arc<Inode> {
    let mut inode = Inode::new(ROOT_INO, InodeMode::new(InodeMode::S_IFDIR | 0o555));
    inode.ops = Some(&SELINUXFS_INODE_OPS);
    Arc::new(inode)
}
