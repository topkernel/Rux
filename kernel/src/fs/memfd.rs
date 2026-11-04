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
//! while a writable MAP_SHARED mapping exists). A memfd created without
//! MFD_ALLOW_SEALING starts pre-sealed F_SEAL_SEAL, like the reference
//! kernel.

use alloc::sync::Arc;

use crate::errno;
use crate::fs::file::{File, FileFlags, REG_FILE_OPS};
use crate::fs::inode::{setattr_attr, Inode, InodeMode, INodeOps};

/// memfd_create flags (UAPI). Values follow include/uapi/linux/memfd.h
/// of the reference kernel exactly (MFD_HUGETLB is 0x0400 there — bit
/// 0x0004 is NOT a flag; a caller passing it gets EINVAL as an unknown
/// bit either way, since we have no hugetlbfs backing).
pub const MFD_CLOEXEC: u32 = 0x0001;
pub const MFD_ALLOW_SEALING: u32 = 0x0002;
pub const MFD_HUGETLB: u32 = 0x0400;
pub const MFD_NOEXEC_SEAL: u32 = 0x0008;
pub const MFD_EXEC: u32 = 0x0010;
pub const MFD_NONBLOCK: u32 = 0x0080;

/// File seal bits (UAPI, memfd_create(2)).
pub const F_SEAL_SEAL: u32 = 0x0001;
pub const F_SEAL_SHRINK: u32 = 0x0002;
pub const F_SEAL_GROW: u32 = 0x0004;
pub const F_SEAL_WRITE: u32 = 0x0008;
pub const F_SEAL_FUTURE_WRITE: u32 = 0x0010;
pub const F_SEAL_EXEC: u32 = 0x0020;

/// The longest name memfd_create accepts, in bytes (excluding the NUL).
///
/// The reference kernel's limit is NAME_MAX (255) minus the length of
/// the "memfd:" prefix it prepends to the file name
/// (kernel/mm/memfd.c: MFD_NAME_MAX_LEN = NAME_MAX - MFD_NAME_PREFIX_LEN).
pub const MFD_NAME_MAX_LEN: usize = 249;

/// All seal bits accepted by F_ADD_SEALS (F_ALL_SEALS in the reference
/// mm/memfd.c — includes F_SEAL_EXEC).
const F_SEAL_KNOWN: u32 = F_SEAL_SEAL
    | F_SEAL_SHRINK
    | F_SEAL_GROW
    | F_SEAL_WRITE
    | F_SEAL_FUTURE_WRITE
    | F_SEAL_EXEC;

/// Per-memfd seal bookkeeping, owned by the inode.
struct MemfdState {
    /// Current seal set. "Can seals be added" is expressed by the set
    /// itself, exactly like the reference: a memfd created without
    /// MFD_ALLOW_SEALING starts F_SEAL_SEAL (the add path EPERMs on it),
    /// while MFD_NOEXEC_SEAL starts F_SEAL_EXEC with SEAL_SEAL cleared —
    /// reference memfd_add_seals has no allow-sealing flag check.
    seals: core::sync::atomic::AtomicU32,
}

/// icache identity prefix for memfd inodes ("MEMF"). memfd inodes are never
/// inserted into the icache (no path lookup can reach them), but a unique
/// fs_id keeps them consistent with the VFS-H8 isolation discipline.
pub const FS_ID_MEMFD: u64 = 0x4D45_4D46_0000_0004;

/// Monotonic memfd inode number source.
static MEMFD_INO: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0x1000);

/// sc138 (Session Q): memfd NAME registry. The parked app children map
/// their engine libraries through memfd fds (APPMAP fd=14 fileoff=0xcf95a
/// style), and `File::path()` prints only "anon_inode:[<ino>]" for them —
/// the park chain's caller lib was unnameable. memfd_create receives the
/// name (OH's loader passes the .so name); record ino -> name here so the
/// APPFD/APPMAP printers can resolve it. Fixed lock-free table, first 512
/// memfd slots of the boot, hash-collision overwrite (probe-only aid).
const MEMFD_NAME_SLOTS: usize = 512;
const MEMFD_NAME_MAX: usize = 48;

struct MemfdNameSlot {
    ino: core::sync::atomic::AtomicU64,
    /// Byte-wise atomic so the static table needs no unsafe mutation.
    name: [core::sync::atomic::AtomicU8; MEMFD_NAME_MAX],
}

const MEMFD_SLOT_ZERO: MemfdNameSlot = MemfdNameSlot {
    ino: core::sync::atomic::AtomicU64::new(0),
    name: [const { core::sync::atomic::AtomicU8::new(0) }; MEMFD_NAME_MAX],
};

static MEMFD_NAMES: [MemfdNameSlot; MEMFD_NAME_SLOTS] = [MEMFD_SLOT_ZERO; MEMFD_NAME_SLOTS];

fn memfd_name_record(ino: u64, name: &[u8]) {
    use core::sync::atomic::Ordering::Relaxed;
    let n = name.len().min(MEMFD_NAME_MAX - 1);
    let slot = &MEMFD_NAMES[(ino as usize) % MEMFD_NAME_SLOTS];
    for i in 0..n {
        slot.name[i].store(name[i], Relaxed);
    }
    for i in n..MEMFD_NAME_MAX {
        slot.name[i].store(0, Relaxed);
    }
    // ino stored LAST: a reader sees either the old entry or a fully
    // written new one; a torn name is possible only under a same-slot
    // race, acceptable for a forensic printer.
    slot.ino.store(ino, core::sync::atomic::Ordering::Release);
}

/// Resolve a memfd name by inode number (the APPFD/APPMAP printers call
/// this after `File::path()` returned "anon_inode:[<ino>]").
pub fn memfd_name_by_ino(ino: u64) -> Option<&'static str> {
    use core::sync::atomic::Ordering::Relaxed;
    let slot = &MEMFD_NAMES[(ino as usize) % MEMFD_NAME_SLOTS];
    if slot.ino.load(core::sync::atomic::Ordering::Acquire) != ino {
        return None;
    }
    // Leak-free static read: build the name in a shared scratch buffer is
    // racy; instead return a reference into a per-call leaked String via
    // Box::leak — probe printers run a handful of times per boot.
    let mut end = 0usize;
    for i in 0..MEMFD_NAME_MAX {
        if slot.name[i].load(Relaxed) == 0 {
            end = i;
            break;
        }
    }
    if end == 0 {
        end = MEMFD_NAME_MAX;
    }
    let bytes: alloc::vec::Vec<u8> = (0..end).map(|i| slot.name[i].load(Relaxed)).collect();
    let s = alloc::string::String::from_utf8(bytes).ok()?;
    Some(alloc::boxed::Box::leak(s.into_boxed_str()))
}

/// setattr for memfd inodes: the only attribute we implement is ATTR_SIZE
/// (ftruncate). Resize is grow-with-zeros / shrink — the FileBuffer content
//  is the single copy of the data.
unsafe fn memfd_setattr(inode: &Inode, attr: u32, arg1: u64, _arg2: u64) -> i32 {
    if attr == setattr_attr::ATTR_SIZE {
        let new_size = arg1 as usize;

        // Seal enforcement (reference shmem_setattr): F_SEAL_GROW blocks
        // growing, F_SEAL_SHRINK blocks shrinking — and that is ALL.
        // F_SEAL_WRITE does NOT block ftruncate (memfd_test.c
        // test_seal_write shrinks and grows under a plain F_SEAL_WRITE;
        // the reference checks only SHRINK/GROW here).
        let seals = inode_seals(inode);
        let old_size = inode.get_size() as usize;
        if seals & F_SEAL_GROW != 0 && new_size > old_size {
            return -(errno::constants::EPERM as i32);
        }
        if seals & F_SEAL_SHRINK != 0 && new_size < old_size {
            return -(errno::constants::EPERM as i32);
        }

        let mut guard = inode.data.lock();
        if guard.is_none() {
            *guard = Some(crate::fs::buffer::FileBuffer::new());
        }
        if let Some(ref mut data) = *guard {
            // R76: FileBuffer is page-backed chunk storage; a large
            // ftruncate (observed 64 MiB from gnome-shell init) draws
            // order-4 pages from the zone allocator instead of exhausting
            // the kernel heap.
            if data.resize(new_size).is_err() {
                return -(errno::constants::ENOMEM as i32);
            }
            inode.set_size(new_size as u64);
            return 0;
        }
        return -(errno::constants::ENOMEM as i32);
    }
    if attr == setattr_attr::ATTR_MODE {
        // F_SEAL_EXEC freezes the exec bits (reference shmem_setattr:
        // "(info->seals & F_SEAL_EXEC) && ATTR_MODE: if ((i_mode ^
        // ia_mode) & 0111) return -EPERM" — a chmod that would CHANGE any
        // x bit, in either direction, is refused).
        let new_mode = arg1 as u32;
        if inode_seals(inode) & F_SEAL_EXEC != 0
            && ((inode.mode.bits() ^ new_mode) & 0o111) != 0
        {
            return -(errno::constants::EPERM as i32);
        }
        // fchmod: store the permission bits, keep S_IFREG, and refresh
        // the cached mode word so stat/open/DAC all observe it.
        inode.update_cached_mode(InodeMode::new(InodeMode::S_IFREG | (new_mode & 0o7777)));
        return 0;
    }
    -(errno::constants::EPERM as i32)
}

/// Read the seal set of a memfd inode (F_SEAL_SEAL for non-sealable
/// memfds — set at creation; 0 for a fresh sealable memfd).
fn inode_seals(inode: &Inode) -> u32 {
    // SAFETY: private_data for a memfd inode is a Box<MemfdState> installed
    // at creation (before the inode was published through the fd table) and
    // only freed by destroy_inode when the last Arc<Inode> drops — while we hold
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

/// fallocate(2) for memfd files — reference shmem_fallocate semantics
/// (mm/shmem.c): only FALLOC_FL_KEEP_SIZE and FALLOC_FL_PUNCH_HOLE are
/// supported, anything else is EOPNOTSUPP (the syscall layer already
/// EINVAL'd unknown bits and PUNCH_HOLE without KEEP_SIZE).
///   PUNCH_HOLE: F_SEAL_WRITE/F_SEAL_FUTURE_WRITE make it EPERM; the
///   range is zeroed, the size stays.
///   alloc: F_SEAL_GROW makes a range past EOF EPERM; without KEEP_SIZE
///   the size grows to offset+len (zeros come from the sparse buffer).
pub fn fallocate(inode: &Inode, mode: i32, offset: u64, len: u64) -> Result<(), i32> {
    const FALLOC_FL_KEEP_SIZE: i32 = 1;
    const FALLOC_FL_PUNCH_HOLE: i32 = 2;

    if mode & !(FALLOC_FL_KEEP_SIZE | FALLOC_FL_PUNCH_HOLE) != 0 {
        return Err(-(errno::constants::EOPNOTSUPP as i32));
    }
    // inode_newsize_ok's overflow answer (EFBIG) — loff_t end overflow.
    let end = offset
        .checked_add(len)
        .ok_or(-(errno::constants::EINVAL as i32))?;

    let seals = inode_seals(inode);
    if mode & FALLOC_FL_PUNCH_HOLE != 0 {
        if seals & (F_SEAL_WRITE | F_SEAL_FUTURE_WRITE) != 0 {
            return Err(-(errno::constants::EPERM as i32));
        }
        // Zero [offset, offset+len) clipped to the current size
        // (shmem_truncate_range on a page-rounded window; the FileBuffer
        // keeps the observable contract: reads in the range return 0).
        let size = inode.get_size() as usize;
        let start = (offset as usize).min(size);
        let stop = (end as usize).min(size);
        if stop > start {
            let mut guard = inode.data.lock();
            if let Some(ref mut data) = *guard {
                let zeros = alloc::vec![0u8; stop - start];
                data.write(start, &zeros);
            }
        }
        return Ok(());
    }

    if seals & F_SEAL_GROW != 0 && end > inode.get_size() {
        return Err(-(errno::constants::EPERM as i32));
    }
    if mode & FALLOC_FL_KEEP_SIZE == 0 && end > inode.get_size() {
        let mut guard = inode.data.lock();
        if guard.is_none() {
            *guard = Some(crate::fs::buffer::FileBuffer::new());
        }
        if let Some(ref mut data) = *guard {
            data.resize(end as usize).map_err(|_| -(errno::constants::ENOMEM as i32))?;
        }
        inode.set_size(end);
    }
    Ok(())
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
    readdir_cacheable: false,
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
    // The reference kernel reads the name with strnlen_user() bounded by
    // MFD_NAME_MAX_LEN + 1 and rejects a longer name with EINVAL.
    if name.len() > MFD_NAME_MAX_LEN {
        return Err(-(errno::constants::EINVAL as i32));
    }

    // Flag validation. MFD_HUGETLB needs a hugetlbfs pool we do not have;
    // MFD_ALLOW_SEALING enables the F_ADD_SEALS/F_GET_SEALS interface (see
    // module docs). Unknown bits are EINVAL like Linux.
    //
    // MFD_NONBLOCK is an extension: the flag does not exist in the
    // reference UAPI (nor upstream at all), so no conformant caller can
    // pass it; it is accepted and mapped onto O_NONBLOCK rather than
    // rejected, so a caller that hardcodes the bit still gets a working
    // file. Every other accepted bit is in the reference's MFD_ALL_FLAGS.
    const KNOWN: u32 = MFD_CLOEXEC
        | MFD_ALLOW_SEALING
        | MFD_HUGETLB
        | MFD_NOEXEC_SEAL
        | MFD_EXEC
        | MFD_NONBLOCK;
    if flags & !KNOWN != 0 {
        return Err(-(errno::constants::EINVAL as i32));
    }
    // MFD_EXEC and MFD_NOEXEC_SEAL are mutually exclusive (the reference
    // kernel rejects the combination with EINVAL, kernel/mm/memfd.c:
    // "Invalid if both EXEC and NOEXEC_SEAL are set").
    if flags & MFD_EXEC != 0 && flags & MFD_NOEXEC_SEAL != 0 {
        return Err(-(errno::constants::EINVAL as i32));
    }
    if flags & MFD_HUGETLB != 0 {
        return Err(-(errno::constants::EINVAL as i32)); // no hugetlbfs backing
    }

    let ino = MEMFD_INO.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    // sc138: remember the caller-supplied name for the probe printers.
    memfd_name_record(ino, name);

    // Flag resolution mirrors the reference with vm.memfd_noexec scope 0
    // (its default): a caller passing neither MFD_EXEC nor
    // MFD_NOEXEC_SEAL gets MFD_EXEC implied (check_sysctl_memfd_noexec,
    // kernel/mm/memfd.c). The reference's pr_warn_once on such calls is
    // dmesg-only and intentionally not reproduced (serial spam for every
    // legacy caller); scope 1/2 enforcement is not implemented (no sysctl
    // surface yet).
    let flags = if flags & (MFD_EXEC | MFD_NOEXEC_SEAL) == 0 {
        flags | MFD_EXEC
    } else {
        flags
    };

    // Mode matrix, straight from the reference: shmem_file_setup creates
    // the inode S_IFREG|0777 with NO umask (kernel-internal inode, not a
    // VFS mknod), and MFD_NOEXEC_SEAL strips the exec bits (i_mode &=
    // ~0111). kselftest memfd_test.c asserts 0777 for MFD_EXEC and 0666
    // for MFD_NOEXEC_SEAL via fstat.
    let mut mode = InodeMode::S_IFREG | 0o777;
    if flags & MFD_NOEXEC_SEAL != 0 {
        mode &= !0o111;
    }
    let mut inode = Inode::new(ino, InodeMode::new(mode));
    inode.fs_id = FS_ID_MEMFD;
    inode.ops = Some(&MEMFD_INODE_OPS);
    // Initial seal matrix (reference mm/memfd.c memfd_create over the
    // shmem default info->seals = F_SEAL_SEAL, shmem_get_inode):
    //   MFD_NOEXEC_SEAL          -> F_SEAL_SEAL cleared, F_SEAL_EXEC set
    //                                (regardless of MFD_ALLOW_SEALING)
    //   MFD_ALLOW_SEALING alone  -> F_SEAL_SEAL cleared (empty set)
    //   neither                  -> F_SEAL_SEAL stays (sealing disabled;
    //                                F_GET_SEALS reports it, and it is
    //                                what makes F_ADD_SEALS EPERM)
    // Seal bookkeeping hangs off the inode (shared by dups/forks of the
    // fd); freed by memfd_destroy_inode when the last reference drops.
    let initial_seals = if flags & MFD_NOEXEC_SEAL != 0 {
        F_SEAL_EXEC
    } else if flags & MFD_ALLOW_SEALING != 0 {
        0
    } else {
        F_SEAL_SEAL
    };
    inode.private_data = Some(
        alloc::boxed::Box::into_raw(alloc::boxed::Box::new(MemfdState {
            seals: core::sync::atomic::AtomicU32::new(initial_seals),
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
    // Reference shmem write path seal rules:
    // - F_SEAL_WRITE | F_SEAL_FUTURE_WRITE: write(2) is EPERM outright
    //   (FUTURE_WRITE's "existing mappings keep working" carve-out is
    //   about MAP_SHARED pages, not write(2));
    // - F_SEAL_GROW: a write whose pos+len exceeds i_size is EPERM
    //   (in-size writes stay allowed).
    // SAFETY: inode cell written once at open; read-only here.
    let (seals, size) = unsafe {
        (*file.inode.get())
            .as_ref()
            .map(|ino| (inode_seals(ino), ino.get_size() as usize))
            .unwrap_or((0, 0))
    };
    if seals & (F_SEAL_WRITE | F_SEAL_FUTURE_WRITE) != 0 {
        return -(errno::constants::EPERM as isize);
    }
    if seals & F_SEAL_GROW != 0 {
        let pos = file.get_pos() as usize;
        if pos.saturating_add(buf.len()) > size {
            return -(errno::constants::EPERM as isize);
        }
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
pub static MEMFD_FILE_OPS: crate::fs::file::FileOps = crate::fs::file::FileOps::builder()
    .read(memfd_file_read)
    .write(memfd_file_write)
    .lseek(memfd_file_lseek)
    .close(memfd_file_close)
    .build();

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

/// Seal set of `file` when it is a memfd, None otherwise (mmap gate).
pub fn file_seals(file: &File) -> Option<u32> {
    let inode = file_memfd_inode(file)?;
    Some(inode_seals(&inode))
}

/// True when `file` is a memfd file (ops-table identity) — the magic-link
/// reopen router.
pub fn is_memfd_file(file: &File) -> bool {
    file.get_ops()
        .is_some_and(|o| core::ptr::eq(o as *const _, &MEMFD_FILE_OPS as *const _))
}

/// Reopen a memfd through its /proc/[pid]/fd/N magic link (the reference
/// nd_jump_link lands on the shmem dentry and opens it: same inode, NEW
/// file description carrying the caller's flags). O_TRUNC goes through
/// the same seal-checked resize as ftruncate (reference handle_truncate
/// -> shmem_setattr: F_SEAL_SHRINK blocks it, F_SEAL_WRITE does not).
pub fn reopen(file: &File, flags: u32) -> Result<alloc::sync::Arc<File>, i32> {
    use crate::fs::file::FileFlags;
    let inode =
        file_memfd_inode(file).ok_or(-(errno::constants::EINVAL as i32))?;

    // DAC on the memfd mode like any reopen (0777 / 0666 from creation).
    let accmode = flags & FileFlags::O_ACCMODE;
    let mut mask = 0u32;
    if accmode != FileFlags::O_WRONLY {
        mask |= crate::fs::permission::MAY_READ;
    }
    if accmode != 0 {
        mask |= crate::fs::permission::MAY_WRITE;
    }
    if !crate::fs::permission::inode_permission(&inode, mask) {
        return Err(-(errno::constants::EACCES as i32));
    }

    let new_file = alloc::sync::Arc::new(File::new(FileFlags::new(flags)));
    new_file.set_inode(alloc::sync::Arc::clone(&inode));
    new_file.set_ops(&MEMFD_FILE_OPS);
    if flags & FileFlags::O_TRUNC != 0 {
        // SAFETY: memfd_setattr only touches this memfd inode's own state
        // (seals, size, FileBuffer); we hold an Arc to it.
        let r = unsafe { memfd_setattr(&inode, setattr_attr::ATTR_SIZE, 0, 0) };
        if r != 0 {
            return Err(r);
        }
    }
    Ok(new_file)
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

    let cur = state.seals.load(core::sync::atomic::Ordering::Acquire);
    if cur & F_SEAL_SEAL != 0 {
        // Already sealed against further seals — this is also how the
        // reference refuses F_ADD_SEALS on a memfd created without
        // MFD_ALLOW_SEALING (it starts F_SEAL_SEAL).
        return Err(-(errno::constants::EPERM as i32));
    }
    if seals & F_SEAL_WRITE != 0 && cur & F_SEAL_WRITE == 0 && has_writable_shared_mapping(file)
    {
        // Adding F_SEAL_WRITE while a writable shared mapping exists.
        return Err(-(errno::constants::EBUSY as i32));
    }

    // "SEAL_EXEC implies SEAL_WRITE, making W^X from the start" (reference
    // memfd_add_seals): adding F_SEAL_EXEC while the mode still has any
    // exec bit expands the request to the full write-block set.
    let mut seals = seals;
    if seals & F_SEAL_EXEC != 0 && inode.mode.bits() & 0o111 != 0 {
        seals |= F_SEAL_SHRINK | F_SEAL_GROW | F_SEAL_WRITE | F_SEAL_FUTURE_WRITE;
    }

    state
        .seals
        .fetch_or(seals, core::sync::atomic::Ordering::AcqRel);
    Ok(())
}
