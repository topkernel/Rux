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
//!
//! Seals (F_ADD_SEALS / F_GET_SEALS, wired for OpenHarmony's
//! gralloc/buffer management): the seal set lives in a `MemfdState` Box
//! hung off the inode's `private_data` (freed by the `destroy_inode`
//! hook when the last Arc<Inode> drops). Enforced at the three mutation
//! points Linux guards: ftruncate (F_SEAL_GROW / F_SEAL_SHRINK /
//! F_SEAL_WRITE), write(2) (F_SEAL_WRITE), and F_ADD_SEALS itself
//! (F_SEAL_SEAL prevents further additions; adding F_SEAL_WRITE is EBUSY
//! while a writable MAP_SHARED mapping exists).

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

/// File seal bits (UAPI, memfd_create(2)).
pub const F_SEAL_SEAL: u32 = 0x0001;
pub const F_SEAL_SHRINK: u32 = 0x0002;
pub const F_SEAL_GROW: u32 = 0x0004;
pub const F_SEAL_WRITE: u32 = 0x0008;
pub const F_SEAL_FUTURE_WRITE: u32 = 0x0010;

/// All seal bits accepted by F_ADD_SEALS.
const F_SEAL_KNOWN: u32 =
    F_SEAL_SEAL | F_SEAL_SHRINK | F_SEAL_GROW | F_SEAL_WRITE | F_SEAL_FUTURE_WRITE;

/// Per-memfd seal bookkeeping, owned by the inode.
struct MemfdState {
    /// MFD_ALLOW_SEALING was passed at creation.
    allow_sealing: bool,
    /// Current seal set (0 = no seals).
    seals: core::sync::atomic::AtomicU32,
}

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

        // Seal enforcement (memfd_create(2)): F_SEAL_WRITE blocks ANY size
        // change; F_SEAL_GROW blocks growing; F_SEAL_SHRINK blocks
        // shrinking. EPERM like Linux memfd ftruncate.
        let seals = inode_seals(inode);
        let old_size = inode.get_size() as usize;
        if seals != 0 {
            if seals & F_SEAL_WRITE != 0 && new_size != old_size {
                return -(errno::constants::EPERM as i32);
            }
            if seals & F_SEAL_GROW != 0 && new_size > old_size {
                return -(errno::constants::EPERM as i32);
            }
            if seals & F_SEAL_SHRINK != 0 && new_size < old_size {
                return -(errno::constants::EPERM as i32);
            }
        }

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

/// Read the seal set of a memfd inode (0 for non-sealable memfds too).
fn inode_seals(inode: &Inode) -> u32 {
    // SAFETY: private_data for a memfd inode is a Box<MemfdState> installed
    // at creation (before the inode was published through the fd table) and
    // only freed by destroy_inode when the last Arc drops — while we hold
    // one, the pointer is valid.
    unsafe {
        match inode.private_data {
            Some(p) => (*(p as *const MemfdState))
                .seals
                .load(core::sync::atomic::Ordering::Acquire),
            None => 0,
        }
    }
}

/// destroy_inode hook: free the MemfdState Box.
unsafe fn memfd_destroy_inode(inode: &mut Inode) {
    if let Some(p) = inode.private_data.take() {
        // SAFETY: the Box was created by memfd_create and is exclusively
        // owned by this inode; the last Arc<Inode> is dropping now.
        drop(alloc::boxed::Box::from_raw(p as *mut MemfdState));
    }
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
    destroy_inode: Some(memfd_destroy_inode),
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
    // MFD_ALLOW_SEALING enables the F_ADD_SEALS/F_GET_SEALS interface (see
    // module docs). Unknown bits are EINVAL like Linux.
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
    // Seal bookkeeping hangs off the inode (shared by dups/forks of the fd);
    // freed by memfd_destroy_inode when the last reference drops.
    inode.private_data = Some(
        alloc::boxed::Box::into_raw(alloc::boxed::Box::new(MemfdState {
            allow_sealing: flags & MFD_ALLOW_SEALING != 0,
            seals: core::sync::atomic::AtomicU32::new(0),
        })) as *mut u8,
    );
    let inode = Arc::new(inode);

    // File flags: always O_RDWR (memfd is opened read-write by design);
    // MFD_NONBLOCK maps onto O_NONBLOCK (advisory for this file type).
    let mut file_flags = FileFlags::O_RDWR;
    if flags & MFD_NONBLOCK != 0 {
        file_flags |= FileFlags::O_NONBLOCK;
    }
    let file = Arc::new(File::new(FileFlags::new(file_flags)));
    file.set_inode(Arc::clone(&inode));
    file.set_ops(&MEMFD_FILE_OPS);

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

// ============================================================================
// File operations (seal-aware wrappers around the generic REG file ops)
// ============================================================================

fn memfd_file_read(file: &File, buf: &mut [u8]) -> isize {
    crate::fs::file::reg_file_read_pub(file, buf)
}

fn memfd_file_write(file: &File, buf: &[u8]) -> isize {
    // F_SEAL_WRITE blocks write(2) with EPERM (memfd_create(2)).
    // SAFETY: inode cell written once at open; read-only here.
    let seals = unsafe {
        (*file.inode.get())
            .as_ref()
            .map(|ino| inode_seals(ino))
            .unwrap_or(0)
    };
    if seals & F_SEAL_WRITE != 0 {
        return -(errno::constants::EPERM as isize);
    }
    crate::fs::file::reg_file_write_pub(file, buf)
}

fn memfd_file_lseek(file: &File, offset: isize, whence: i32) -> isize {
    crate::fs::file::reg_file_lseek_pub(file, offset, whence)
}

fn memfd_file_close(file: &File) -> i32 {
    crate::fs::file::reg_file_close_pub(file)
}

/// File ops for memfd files: identical to REG_FILE_OPS except the write
/// path enforces F_SEAL_WRITE.
static MEMFD_FILE_OPS: crate::fs::file::FileOps = crate::fs::file::FileOps {
    read: Some(memfd_file_read),
    write: Some(memfd_file_write),
    lseek: Some(memfd_file_lseek),
    close: Some(memfd_file_close),
    poll: None,
};

// ============================================================================
// fcntl F_ADD_SEALS / F_GET_SEALS
// ============================================================================

/// The inode of a memfd File, or None.
fn file_memfd_inode(file: &File) -> Option<Arc<Inode>> {
    // SAFETY: inode cell written once at open time; read-only here.
    let inode = unsafe { (*file.inode.get()).clone() }?;
    if inode.fs_id == FS_ID_MEMFD {
        Some(inode)
    } else {
        None
    }
}

/// F_GET_SEALS handler: the file's current seal set. EINVAL when the file
/// is not a memfd (Linux: seals are only defined for shmem/memfd files).
pub fn get_seals(file: &File) -> Result<u32, i32> {
    let inode = file_memfd_inode(file).ok_or(-(errno::constants::EINVAL as i32))?;
    Ok(inode_seals(&inode))
}

/// True when the current task has a writable MAP_SHARED VMA backed by
/// `file` (the F_ADD_SEALS(F_SEAL_WRITE) EBUSY condition).
///
/// Simplification vs Linux's i_mmap_writable count: only the CALLER's
/// address space is scanned. A dup'd fd mapped writable in another process
/// is not visible here (documented; the common seal-then-share pattern
/// seals before any mapping exists).
fn has_writable_shared_mapping(file: &File) -> bool {
    use crate::mm::vma::VmaFlags;
    let task = match crate::sched::current() {
        Some(t) => t,
        None => return false,
    };
    let aspace = match task.address_space() {
        Some(a) => a,
        None => return false,
    };
    let vma_mgr = aspace.vma_read();
    for vma in vma_mgr.iter() {
        let flags = vma.flags();
        if !flags.contains(VmaFlags::SHARED) || !flags.contains(VmaFlags::WRITE) {
            continue;
        }
        if let Some(pinned) = aspace.get_vma_file(vma.start().as_usize()) {
            if core::ptr::eq(Arc::as_ptr(&pinned) as *const File, file as *const File) {
                return true;
            }
        }
    }
    false
}

/// F_ADD_SEALS handler: add `seals` to the file's seal set.
pub fn add_seals(file: &File, seals: u32) -> Result<(), i32> {
    let inode = file_memfd_inode(file).ok_or(-(errno::constants::EINVAL as i32))?;

    // Unknown bits are EINVAL (Linux memfd_add_seals).
    if seals & !F_SEAL_KNOWN != 0 {
        return Err(-(errno::constants::EINVAL as i32));
    }

    // SAFETY: private_data is the MemfdState Box owned by this memfd inode
    // (see inode_seals).
    let state = unsafe {
        match inode.private_data {
            Some(p) => &*(p as *const MemfdState),
            None => return Err(-(errno::constants::EINVAL as i32)),
        }
    };

    if !state.allow_sealing {
        // File created without MFD_ALLOW_SEALING.
        return Err(-(errno::constants::EPERM as i32));
    }

    let cur = state.seals.load(core::sync::atomic::Ordering::Acquire);
    if cur & F_SEAL_SEAL != 0 {
        // Already sealed against further seals.
        return Err(-(errno::constants::EPERM as i32));
    }
    if seals & F_SEAL_WRITE != 0 && cur & F_SEAL_WRITE == 0 && has_writable_shared_mapping(file)
    {
        // Adding F_SEAL_WRITE while a writable shared mapping exists.
        return Err(-(errno::constants::EBUSY as i32));
    }

    state
        .seals
        .fetch_or(seals, core::sync::atomic::Ordering::AcqRel);
    Ok(())
}
