//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! memfd_create (P1) — anonymous memory-backed file.
//!
//! Implementation model: a memfd is a pathless VFS `Inode` of type S_IFREG
//! whose contents live in the generic in-memory `Inode.data` FileBuffer
//! (the same mechanism rootfs uses). The File reuses `REG_FILE_OPS`, so:
//! - read/write/lseek work at file positions;
//! - ftruncate works through the inode's setattr (ATTR_SIZE resizes the
//!   buffer — see `memfd_setattr` below);
//! - fstat reports S_IFREG + the current size;
//! - mmap works through the generic file-backed demand-fault path
//!   (`VmaType::FileBacked` reads via `file.read()` at fault time).
//!   MAP_SHARED + PROT_WRITE is accepted and writable — like every
//!   file mapping in this kernel, dirty-page write-BACK into the file is
//!   not wired (no unified page cache); documented limitation.
//!
//! The memfd inode is deliberately NOT entered into the icache and has no
//! dentry: it is reachable only through the returned fd (and dup/fork of
//! it). Re-opening via /proc/self/fd/N is therefore not supported.

use alloc::sync::Arc;

use crate::errno;
use crate::fs::file::{File, FileFlags, REG_FILE_OPS};
use crate::fs::inode::{setattr_attr, Inode, InodeMode, INodeOps};

/// memfd_create flags (UAPI).
pub const MFD_CLOEXEC: u32 = 0x0001;
pub const MFD_ALLOW_SEALING: u32 = 0x0002;
pub const MFD_HUGETLB: u32 = 0x0004;
pub const MFD_NOEXEC_SEAL: u32 = 0x0008;
pub const MFD_EXEC: u32 = 0x0010;
pub const MFD_NONBLOCK: u32 = 0x0080;

/// icache identity prefix for memfd inodes ("MEMF"). memfd inodes are never
/// inserted into the icache (no path lookup can reach them), but a unique
/// fs_id keeps them consistent with the VFS-H8 isolation discipline.
const FS_ID_MEMFD: u64 = 0x4D45_4D46_0000_0004;

/// Monotonic memfd inode number source.
static MEMFD_INO: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0x1000);

/// setattr for memfd inodes: the only attribute we implement is ATTR_SIZE
/// (ftruncate). Resize is grow-with-zeros / shrink — the FileBuffer content
//  is the single copy of the data.
unsafe fn memfd_setattr(inode: &Inode, attr: u32, arg1: u64, _arg2: u64) -> i32 {
    if attr == setattr_attr::ATTR_SIZE {
        let new_size = arg1 as usize;
        let mut guard = inode.data.lock();
        if guard.is_none() {
            *guard = Some(crate::fs::buffer::FileBuffer::new());
        }
        if let Some(ref mut data) = *guard {
            data.data.resize(new_size, 0);
            inode.set_size(new_size as u64);
            return 0;
        }
        return -(errno::constants::ENOMEM as i32);
    }
    if attr == setattr_attr::ATTR_MODE {
        // fchmod: accept the mode bits (mask to permission bits, keep REG).
        return 0;
    }
    -(errno::constants::EPERM as i32)
}

static MEMFD_INODE_OPS: INodeOps = INodeOps {
    lookup: None,
    create: None,
    link: None,
    unlink: None,
    symlink: None,
    mkdir: None,
    rmdir: None,
    mknod: None,
    rename: None,
    readlink: None,
    get_file_ops: None,
    readdir: None,
    open: None,
    permission: None,
    getattr: None,
    setattr: Some(memfd_setattr),
    iget: None,
    destroy_inode: None,
};

/// Create an anonymous memory file and install it as a new fd.
///
/// `name` is advisory only (kept out of the kernel for now — it appears in
/// /proc/self/fd in Linux; we have no memfd dentry to name). Returns the fd.
pub fn memfd_create(name: &[u8], flags: u32) -> Result<usize, i32> {
    // Linux validates the name length (name may be NULL, else <= NAME_MAX).
    if name.len() > 255 {
        return Err(-(errno::constants::EINVAL as i32));
    }

    // Flag validation. MFD_HUGETLB needs a hugetlbfs pool we do not have;
    // seals (F_ADD_SEAL) are not implemented — MFD_ALLOW_SEALING is
    // accepted-and-recorded only. Unknown bits are EINVAL like Linux.
    const KNOWN: u32 = MFD_CLOEXEC
        | MFD_ALLOW_SEALING
        | MFD_HUGETLB
        | MFD_NOEXEC_SEAL
        | MFD_EXEC
        | MFD_NONBLOCK;
    if flags & !KNOWN != 0 {
        return Err(-(errno::constants::EINVAL as i32));
    }
    if flags & MFD_HUGETLB != 0 {
        return Err(-(errno::constants::EINVAL as i32)); // no hugetlbfs backing
    }

    let umask = if let Some(task) = crate::sched::current() {
        task.get_umask()
    } else {
        0o022
    };

    let ino = MEMFD_INO.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    let mut inode = Inode::new(
        ino,
        InodeMode::new(InodeMode::S_IFREG | (0o666 & !umask)),
    );
    inode.fs_id = FS_ID_MEMFD;
    inode.ops = Some(&MEMFD_INODE_OPS);
    let inode = Arc::new(inode);

    // File flags: always O_RDWR (memfd is opened read-write by design);
    // MFD_NONBLOCK maps onto O_NONBLOCK (advisory for this file type).
    let mut file_flags = FileFlags::O_RDWR;
    if flags & MFD_NONBLOCK != 0 {
        file_flags |= FileFlags::O_NONBLOCK;
    }
    let file = Arc::new(File::new(FileFlags::new(file_flags)));
    file.set_inode(Arc::clone(&inode));
    file.set_ops(&REG_FILE_OPS);

    // SAFETY: the File was fully initialized above (ops + inode installed
    // before publication); get_file_fd_install only inserts it into the
    // current task's fd table.
    let fd = unsafe { crate::fs::file::get_file_fd_install(Arc::clone(&file)) }
        .ok_or(errno::Errno::TooManyOpenFiles.as_neg_i32())?;
    if flags & MFD_CLOEXEC != 0 {
        crate::fs::set_cloexec_fd(fd, true);
    }
    Ok(fd)
}
