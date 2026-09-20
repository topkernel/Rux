//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! File Object and File Descriptor Management
//!
//!
//! Core concepts:
//! - `struct file`: Opened file object
//! - `fdtable`: File descriptor table
//! - `struct file_operations`: File operation function pointers

use crate::errno;
use crate::fs::inode::Inode;
use crate::fs::dentry::Dentry;
use alloc::sync::Arc;
use alloc::boxed::Box;
use crate::sync::spinlock::Spinlock;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU32, Ordering};

#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct FileFlags(u32);

impl FileFlags {
    pub const O_RDONLY: u32 = 0o00000000;
    pub const O_WRONLY: u32 = 0o00000001;
    pub const O_RDWR: u32 = 0o00000002;
    pub const O_ACCMODE: u32 = 0o00000003;
    pub const O_CREAT: u32 = 0o00000100;
    pub const O_EXCL: u32 = 0o00000200;
    pub const O_NOCTTY: u32 = 0o00000400;
    pub const O_TRUNC: u32 = 0o00001000;
    pub const O_APPEND: u32 = 0o00002000;
    pub const O_NONBLOCK: u32 = 0o00004000;
    pub const O_DSYNC: u32 = 0o00010000;
    pub const O_DIRECT: u32 = 0o00040000;
    pub const O_LARGEFILE: u32 = 0o00100000;
    pub const O_DIRECTORY: u32 = 0o00200000;
    pub const O_NOFOLLOW: u32 = 0o00400000;
    pub const O_NOATIME: u32 = 0o01000000;
    pub const O_CLOEXEC: u32 = 0o02000000;
    pub const O_SYNC: u32 = 0o04000000;
    pub const O_PATH: u32 = 0o10000000;

    pub fn new(flags: u32) -> Self {
        Self(flags)
    }

    pub fn is_readonly(&self) -> bool {
        (self.0 & Self::O_ACCMODE) == Self::O_RDONLY
    }

    pub fn is_writeonly(&self) -> bool {
        (self.0 & Self::O_ACCMODE) == Self::O_WRONLY
    }

    pub fn is_rdwr(&self) -> bool {
        (self.0 & Self::O_ACCMODE) == Self::O_RDWR
    }

    pub fn bits(&self) -> u32 {
        self.0
    }

    /// Set flags (for F_SETFL)
    pub fn set_bits(&mut self, flags: u32) {
        self.0 = flags;
    }

    /// Add flags (bitwise OR)
    pub fn add_flags(&mut self, flags: u32) {
        self.0 |= flags;
    }
}

#[repr(C)]
pub struct FileOps {
    /// Read file
    pub read: Option<fn(&File, &mut [u8]) -> isize>,
    /// Write file
    pub write: Option<fn(&File, &[u8]) -> isize>,
    /// Seek file position
    pub lseek: Option<fn(&File, isize, i32) -> isize>,
    /// Close file
    pub close: Option<fn(&File) -> i32>,
    /// Poll for readiness (takes requested events mask, returns ready events mask)
    pub poll: Option<fn(&File, u16) -> u16>,
}

/// R36-B1: monotonically increasing open-file-description generation.
/// Epoll registrations record this instead of the Arc's heap address: the
/// slab allocator hands a freed File's address back to the NEXT same-size
/// allocation with near certainty, and fd numbers recycle the same way, so
/// the old address-as-identity scheme let a stale entry silently match a
/// NEW file after close(fd)+reopen (and made the legitimate re-ADD return
/// EEXIST). A never-repeating generation keeps the identity stable for the
/// description's lifetime without pinning the Arc.
static FILE_ID_GENERATION: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(1);

#[repr(C, align(16))]
pub struct File {
    /// R31-B3: set by close_fd when the slot was removed but an in-flight
    /// syscall still holds a clone — the final dropper runs the close op.
    pub close_pending: core::sync::atomic::AtomicBool,
    /// File flags stored as AtomicU32 for lock-free concurrent access
    /// (fcntl F_SETFL vs read/write data race fix).
    pub flags: AtomicU32,
    /// File position
    pub pos: Spinlock<u64>,
    /// Associated inode
    pub inode: UnsafeCell<Option<Arc<Inode>>>,
    /// Associated dentry
    pub dentry: UnsafeCell<Option<Arc<Dentry>>>,
    /// File operation functions
    pub ops: UnsafeCell<Option<&'static FileOps>>,
    /// Private data (for device-specific data)
    pub private_data: UnsafeCell<Option<*mut u8>>,
    /// close-on-exec flag (FD_CLOEXEC)
    pub cloexec: Spinlock<bool>,
    /// R36-B1: unique open-file-description id (see FILE_ID_GENERATION).
    /// Written once in File::new before the value is shared; readers only
    /// need a plain u64 load (the Arc publication orders it).
    pub file_id: u64,
}

// SAFETY: File is only shared across threads when referenced through Arc,
// and all mutable access goes through Spinlock or UnsafeCell fields that
// are synchronized with IRQ-safe locking.
unsafe impl Sync for File {}
unsafe impl Send for File {}

// Compile-time checks for File structure alignment
const _: () = assert!(core::mem::align_of::<File>() >= 16);
const _: () = {
    let offset = core::mem::offset_of!(File, inode);
    assert!(offset % 8 == 0, "inode field is not 8-byte aligned!");
};


impl File {
    /// Create new file object
    pub fn new(flags: FileFlags) -> Self {
        Self {
            flags: AtomicU32::new(flags.bits()),
            pos: Spinlock::new(0),
            inode: UnsafeCell::new(None),
            dentry: UnsafeCell::new(None),
            ops: UnsafeCell::new(None),
            private_data: UnsafeCell::new(None),
            close_pending: core::sync::atomic::AtomicBool::new(false),
            cloexec: Spinlock::new(false),  // Default: don't set close-on-exec
            file_id: FILE_ID_GENERATION.fetch_add(1, Ordering::Relaxed),
        }
    }

    /// Read file flags (returns a copy, lock-free via AtomicU32).
    pub fn flags(&self) -> FileFlags {
        FileFlags::new(self.flags.load(Ordering::Acquire))
    }

    /// Set file flags atomically (for F_SETFL).
    pub fn set_flags(&self, flags: FileFlags) {
        self.flags.store(flags.bits(), Ordering::Release);
    }

    /// Load flags bits atomically (convenience for callers that only need the u32).
    pub fn flags_bits(&self) -> u32 {
        self.flags.load(Ordering::Acquire)
    }

    /// Set inode
    pub fn set_inode(&self, inode: Arc<Inode>) {
        unsafe { *self.inode.get() = Some(inode); }
    }

    /// Set dentry
    pub fn set_dentry(&self, dentry: Arc<Dentry>) {
        unsafe { *self.dentry.get() = Some(dentry); }
    }

    /// Set file operations
    pub fn set_ops(&self, ops: &'static FileOps) {
        unsafe { *self.ops.get() = Some(ops); }
    }

    /// Get file operations
    pub fn get_ops(&self) -> Option<&'static FileOps> {
        unsafe { *self.ops.get() }
    }

    /// Set private data
    pub fn set_private_data(&self, data: *mut u8) {
        unsafe { *self.private_data.get() = Some(data); }
    }

    /// Get close-on-exec flag
    pub fn get_cloexec(&self) -> bool {
        *self.cloexec.lock()
    }

    /// Set close-on-exec flag
    pub fn set_cloexec(&self, cloexec: bool) {
        *self.cloexec.lock() = cloexec;
    }

    /// Read file
    pub unsafe fn read(&self, buf: *mut u8, count: usize) -> isize {
        // f_mode enforcement: read on an O_WRONLY fd is EBADF (Linux)
        if self.flags().is_writeonly() {
            return -9; // EBADF
        }
        if let Some(ops) = *self.ops.get() {
            if let Some(read_fn) = ops.read {
                let slice = core::slice::from_raw_parts_mut(buf, count);
                return read_fn(self, slice);
            }
        }
        -9  // EBADF
    }

    /// Write file
    pub unsafe fn write(&self, buf: *const u8, count: usize) -> isize {
        // f_mode enforcement: write on an O_RDONLY fd is EBADF (Linux)
        if self.flags().is_readonly() {
            return -9; // EBADF
        }
        if let Some(ops) = *self.ops.get() {
            if let Some(write_fn) = ops.write {
                let slice = core::slice::from_raw_parts(buf, count);
                return write_fn(self, slice);
            }
        }
        -9  // EBADF
    }

    /// Seek file position
    pub unsafe fn lseek(&self, offset: isize, whence: i32) -> isize {
        if let Some(ops) = *self.ops.get() {
            if let Some(lseek_fn) = ops.lseek {
                return lseek_fn(self, offset, whence);
            }
        }
        // No seek op = unseekable object (pipe, socket, fifo): POSIX says
        // ESPIPE, not EBADF (review SYSA-M11).
        -29 // ESPIPE
    }

    /// Close file
    pub unsafe fn close(&mut self) -> i32 {
        if let Some(ops) = *self.ops.get() {
            if let Some(close_fn) = ops.close {
                return close_fn(self);
            }
        }
        0
    }

    /// Get current position
    pub fn get_pos(&self) -> u64 {
        *self.pos.lock()
    }

    /// Set file position
    pub fn set_pos(&self, new_pos: u64) {
        *self.pos.lock() = new_pos;
    }

    /// Recover the filesystem path of this file from its dentry.
    ///
    /// Returns "anon_inode:[<id>]" style names for dentry-less files
    /// (pipes, sockets, epoll instances, ...).
    pub fn path(&self) -> alloc::string::String {
        // SAFETY: dentry is written once at open time and never mutated
        // afterwards; read-only access here.
        let dentry_opt = unsafe { (*self.dentry.get()).clone() };
        match dentry_opt {
            Some(dentry) => dentry.build_path(),
            None => {
                // SAFETY: inode is written once at open; read-only access.
                let inode_opt = unsafe { (*self.inode.get()).clone() };
                match inode_opt {
                    Some(inode) => alloc::format!("anon_inode:[{}]", inode.ino),
                    None => alloc::string::String::from("anon_inode"),
                }
            }
        }
    }
}

// R31-B3 (v2 completion): the close op deferred by close_fd's close_pending
// flag runs HERE, in the final dropper's context.  This is NOT the reverted
// v1 behavior ("close on every Drop"): the flag is only set when close_fd
// already removed every fd-table slot for this file while a clone was still
// in flight (a syscall-local Arc or an io_uring pinned file), so ordinary
// drops — failed opens, intermediate clones, table teardown that ran the op
// itself — all see `false` and execute nothing.  Those in-flight clones are
// only ever released in task context (syscall exit / register / ring close),
// never in IRQ context, so ext4/jbd2 side effects of the deferred close stay
// out of arbitrary contexts.  Without this hook the flag was set but never
// consumed and the close op leaked (pipe EOF never delivered, epoll Box and
// mem-file Box never freed).
impl Drop for File {
    fn drop(&mut self) {
        if self.close_pending.load(Ordering::Acquire) {
            // SAFETY: we own the File exclusively (refcount reached 0) and
            // close_fd already committed this close; ops.close was installed
            // at open time and never mutated afterwards.
            unsafe { self.close(); }
        }
    }
}

// ============================================================================
// FdTable - Using Box allocation
// ============================================================================

/// FdTable entry stored on the heap
struct FdTableEntry {
    fds: [Option<Arc<File>>; 1024],
    /// Per-descriptor close-on-exec bits. FD_CLOEXEC belongs to the
    /// DESCRIPTOR in POSIX, not the underlying description: a file opened
    /// O_CLOEXEC at fd N and dup2'd to fd M must leave M WITHOUT the flag.
    /// The old per-File flag leaked O_CLOEXEC through dup2 — musl-based
    /// shells open redirections O_CLOEXEC, dup2 to stdio, exec, and lost
    /// their stdio fds (found via musl __init_libc's deliberate NULL-crash
    /// on POLLNVAL + missing /dev/null).
    cloexec_bits: [u64; 16], // 1024 bits
    next_fd: usize,
    count: usize,
}

pub struct FdTable {
    /// Heap-allocated entry protected by a spinlock for concurrent access.
    /// Replaces the old UnsafeCell which relied on the BKL for safety.
    entry: crate::sync::spinlock::Spinlock<Box<FdTableEntry>>,
}

impl FdTable {
    /// Create new file descriptor table
    pub fn new() -> Self {
        let entry = Box::new(FdTableEntry {
            fds: [const { None }; 1024],
            cloexec_bits: [0; 16],
            next_fd: 0,
            count: 0,
        });

        Self { entry: crate::sync::spinlock::Spinlock::new(entry) }
    }

    /// Allocate file descriptor
    pub fn alloc_fd(&self) -> Option<usize> {
        self.alloc_fd_from(0)
    }

    /// Allocate file descriptor >= min_fd
    pub fn alloc_fd_from(&self, min_fd: usize) -> Option<usize> {
        let mut entry = self.entry.lock_irqsave();
        let start = if min_fd > entry.next_fd { min_fd } else { entry.next_fd };

        // Search from start to 1024
        for fd in start..1024 {
            if entry.fds[fd].is_none() {
                entry.next_fd = (fd + 1) % 1024;
                return Some(fd);
            }
        }
        // Wrap around: search from min_fd to start (if start > min_fd due to next_fd)
        if start > min_fd {
            for fd in min_fd..start {
                if entry.fds[fd].is_none() {
                    entry.next_fd = (fd + 1) % 1024;
                    return Some(fd);
                }
            }
        }

        None
    }

    /// Install file to file descriptor table
    pub fn install_fd(&self, fd: usize, file: Arc<File>) -> Result<(), ()> {
        if fd >= 1024 {
            return Err(());
        }

        let mut entry = self.entry.lock_irqsave();
        if entry.fds[fd].is_some() {
            return Err(());
        }
        entry.fds[fd] = Some(file);
        // Clear any stale CLOEXEC bit from a previous occupant of this fd
        // number (regression round 5, MED: closed CLOEXEC fds leaked the
        // bit to the next file that reused the number).
        entry.cloexec_bits[fd / 64] &= !(1u64 << (fd % 64));
        entry.count += 1;
        Ok(())
    }

    /// Get file object for file descriptor
    pub fn get_file(&self, fd: usize) -> Option<Arc<File>> {
        if fd >= 1024 {
            return None;
        }
        self.entry.lock_irqsave().fds[fd].clone()
    }

    /// Close file descriptor
    pub fn close_fd(&self, fd: usize) -> Result<(), ()> {
        if fd >= 1024 {
            return Err(());
        }

        // R13-4: capture the last-reference decision INSIDE the entry
        // lock. Reading strong_count after the release let a concurrent
        // get_file clone land between the slot clear and the check — the
        // count read 2, the close op was skipped forever, and the pipe
        // EOF was never delivered (hang face). Under the lock the slot is
        // gone, so any clone racing us is either already counted or will
        // see the empty slot; count==1 here is stable because the only
        // remaining holder is the one we are dropping.
        // R14-5: the last-reference DECISION stays inside the entry lock
        // (a concurrent get_file clone must not slip in between slot-clear
        // and count read), but the close op EXECUTES after the lock is
        // dropped — running it under entry.lock_irqsave held socket closes
        // into a multi-second virtio spin with IRQs off, wedging every fd
        // operation of every CLONE_FILES thread. Safe to run late: the
        // slot is gone so no NEW references can appear; our local Arc is
        // still the last one by construction.
        let run_close;
        let file_opt = {
            let mut entry = self.entry.lock_irqsave();
            if entry.fds[fd].is_none() {
                return Err(());
            }
            let file_opt = core::mem::replace(&mut entry.fds[fd], None);
            entry.count -= 1;
            // R10-2 (PIPE2 EBADF root cause): release only on the LAST
            // Arc reference — running the close op per EVENT let
            // pipe_file_close STEAL private_data from the File that fd 1
            // still pointed at after `dup3(w,1); close(w)`.
            run_close = match file_opt {
                Some(ref file) => Arc::strong_count(file) == 1,
                None => false,
            };
            file_opt
        };
        // R31-B3 (v2): if we hold the last reference, run the op now; if an
        // in-flight syscall holds a clone (count was 2), set close_pending
        // so the FINAL drop runs it — the flag lives on the File itself.
        if run_close {
            if let Some(file) = file_opt {
                unsafe {
                    let file_ptr = Arc::as_ptr(&file) as *mut File;
                    // Clear any close_pending left behind by an earlier
                    // close of a dup'd slot: count==2 back then meant "another
                    // fd still holds this file", not an in-flight clone, and
                    // we run the op right here — a stale flag would make the
                    // final Drop run the op a SECOND time (dup2 + close of
                    // both fds double-closed pipes/epoll).
                    (*file_ptr).close_pending.store(false, Ordering::Release);
                    let ops_ptr = (*file_ptr).ops.get();
                    if !ops_ptr.is_null() && !(*ops_ptr).is_none() {
                        (*file_ptr).close();
                    }
                }
            }
        } else if let Some(ref file) = file_opt {
            // Someone else still holds a reference and we removed the slot:
            // mark for the final dropper to run the op.
            unsafe {
                let file_ptr = Arc::as_ptr(file) as *mut File;
                (*file_ptr).close_pending.store(true, core::sync::atomic::Ordering::Release);
            }
        }

        Ok(())
    }

    /// Duplicate file descriptor
    pub fn dup_fd(&self, oldfd: usize) -> Option<usize> {
        if oldfd >= 1024 {
            return None;
        }

        let file = self.get_file(oldfd)?;
        let newfd = self.alloc_fd()?;
        self.install_fd(newfd, file).ok()?;
        Some(newfd)
    }

    /// Duplicate file descriptor to specific number (dup2)
    /// Validates oldfd is open before any operation.
    pub fn dup2_fd(&self, oldfd: usize, newfd: usize) -> Option<usize> {
        if oldfd >= 1024 || newfd >= 1024 {
            return None;
        }

        if oldfd == newfd {
            // dup2(f, f) must return f if f is valid open fd, else EBADF
            self.get_file(oldfd)?;
            return Some(newfd);
        }

        // Get the file to duplicate (validates oldfd is open)
        let file = self.get_file(oldfd)?;

        // Close newfd if open (ignore error — newfd may not be open)
        let _ = self.close_fd(newfd);

        // Install file at newfd
        self.install_fd(newfd, file).ok()?;

        // dup2 leaves FD_CLOEXEC CLEAR on the new descriptor (POSIX);
        // dup3(flags & O_CLOEXEC) sets it — callers apply that via
        // set_fd_cloexec.
        self.set_fd_cloexec(newfd, false);
        Some(newfd)
    }

    /// Per-descriptor close-on-exec flag (FD_CLOEXEC).
    pub fn set_fd_cloexec(&self, fd: usize, on: bool) {
        if fd >= 1024 { return; }
        let mut entry = self.entry.lock_irqsave();
        if on {
            entry.cloexec_bits[fd / 64] |= 1u64 << (fd % 64);
        } else {
            entry.cloexec_bits[fd / 64] &= !(1u64 << (fd % 64));
        }
    }

    pub fn get_fd_cloexec(&self, fd: usize) -> bool {
        if fd >= 1024 { return false; }
        let entry = self.entry.lock_irqsave();
        entry.cloexec_bits[fd / 64] & (1u64 << (fd % 64)) != 0
    }

    /// Close all file descriptors with close-on-exec flag set
    pub fn close_cloexec_fds(&self) {
        // Collect cloexec fds under lock, then close outside lock
        let cloexec_fds: alloc::vec::Vec<usize> = {
            let entry = self.entry.lock_irqsave();
            (0..1024).filter(|&fd| {
                entry.cloexec_bits[fd / 64] & (1u64 << (fd % 64)) != 0
            }).collect()
        };
        for fd in cloexec_fds {
            let _ = self.close_fd(fd);
        }
    }
}

impl Drop for FdTable {
    fn drop(&mut self) {
        // R20-FS3 (R14-5 for Drop): make the last-reference DECISION under the
        // entry lock but run the close ops AFTER releasing it — the old Drop
        // ran ops.close (socket close = multi-second virtio spin) with IRQs
        // off while holding the entry lock, wedging every fd operation of
        // every CLONE_FILES thread, exactly the hazard R14-5 fixed in
        // close_fd but never applied here.
        let mut to_close: alloc::vec::Vec<Arc<File>> = alloc::vec::Vec::new();
        {
            let mut entry = self.entry.lock_irqsave();
            for fd in 0..1024 {
                if entry.fds[fd].is_some() {
                    let file_opt = core::mem::replace(&mut entry.fds[fd], None);
                    entry.count -= 1;
                    if let Some(file) = file_opt {
                        // R10-2: last-reference-only release — see close_fd.
                        // R31-B3 (v2 completion): dup'd files live in several
                        // slots of the SAME table, so a count>1 here means a
                        // syscall-local clone is still outstanding — defer to
                        // the final dropper via close_pending exactly like
                        // close_fd does.  (The v1 "let _ = file;" erasure
                        // skipped the close op entirely: process exit never
                        // closed pipes, so readers never saw EOF.)
                        if Arc::strong_count(&file) == 1 {
                            to_close.push(file);
                        } else {
                            unsafe {
                                let file_ptr = Arc::as_ptr(&file) as *mut File;
                                (*file_ptr).close_pending.store(true, Ordering::Release);
                            }
                        }
                    }
                }
            }
        }
        for file in to_close {
            unsafe {
                let file_ptr = Arc::as_ptr(&file) as *mut File;
                // Clear a possibly-stale deferred flag before running the op
                // ourselves — same double-close hazard as close_fd.
                (*file_ptr).close_pending.store(false, Ordering::Release);
                let ops_ptr = (*file_ptr).ops.get();
                if !ops_ptr.is_null() && !(*ops_ptr).is_none() {
                    (*file_ptr).close();
                }
            }
        }
        // Box will be automatically deallocated
    }
}


/// Set the per-descriptor FD_CLOEXEC flag (POSIX: the flag belongs to the
/// descriptor, never the underlying file description).
pub fn set_cloexec_fd(fd: usize, on: bool) {
    if let Some(ft) = crate::sched::get_current_fdtable() {
        ft.set_fd_cloexec(fd, on);
    }
}

pub unsafe fn get_file_fd(fd: usize) -> Option<Arc<File>> {
    use crate::sched;
    sched::get_current_fdtable()?.get_file(fd)
}

pub unsafe fn get_file_fd_install(file: Arc<File>) -> Option<usize> {
    use crate::sched;
    let fdtable = sched::get_current_fdtable()?;
    let fd = fdtable.alloc_fd()?;
    fdtable.install_fd(fd, file).ok()?;
    Some(fd)
}

pub unsafe fn close_file_fd(fd: usize) -> Result<(), i32> {
    use crate::sched;
    match sched::get_current_fdtable() {
        Some(fdtable) => fdtable.close_fd(fd).map_err(|_| errno::Errno::BadFileNumber.as_neg_i32()),
        None => Err(errno::Errno::BadFileNumber.as_neg_i32()),
    }
}

// ============================================================================
// Kernel thread standard input/output
// ============================================================================

pub unsafe fn get_stdin() -> Option<Arc<File>> {
    get_file_fd(0)
}

pub unsafe fn get_stdout() -> Option<Arc<File>> {
    get_file_fd(1)
}

pub unsafe fn get_stderr() -> Option<Arc<File>> {
    get_file_fd(2)
}

// ============================================================================
// Default operations for regular files
// ============================================================================

fn reg_file_read(file: &File, buf: &mut [u8]) -> isize {
    if let Some(ref inode) = unsafe { &*file.inode.get() } {
        // Get current file position
        let offset = file.get_pos() as usize;

        // Read data from inode (buf.length handles automatically)
        let bytes_read = inode.read_data(offset, buf);

        // Update file position
        file.set_pos((offset + bytes_read) as u64);

        bytes_read as isize
    } else {
        -9  // EBADF
    }
}

fn reg_file_write(file: &File, buf: &[u8]) -> isize {
    if let Some(ref inode) = unsafe { &*file.inode.get() } {
        // O_APPEND: atomically move position to end before every write
        let offset = if file.flags.load(Ordering::Acquire) & FileFlags::O_APPEND != 0 {
            let end = inode.get_size();
            file.set_pos(end);
            end as usize
        } else {
            file.get_pos() as usize
        };

        // Write data to inode (buf.length handles automatically)
        let bytes_written = inode.write_data(offset, buf);

        // Update file position
        file.set_pos((offset + bytes_written) as u64);

        bytes_written as isize
    } else {
        -9  // EBADF
    }
}

fn reg_file_lseek(file: &File, offset: isize, whence: i32) -> isize {
    // SEEK_SET = 0, SEEK_CUR = 1, SEEK_END = 2
    let current_pos = file.get_pos() as i64;

    // Get file size
    let file_size = if let Some(ref inode) = unsafe { &*file.inode.get() } {
        inode.get_size() as i64
    } else {
        return -9  // EBADF
    };

    let new_pos = match whence {
        0 => offset as i64,                                           // SEEK_SET
        1 => match current_pos.checked_add(offset as i64) {           // SEEK_CUR
            Some(v) => v,
            None => return -22,  // EOVERFLOW
        },
        2 => match file_size.checked_add(offset as i64) {             // SEEK_END
            Some(v) => v,
            None => return -22,  // EOVERFLOW
        },
        _ => return -22,           // EINVAL - invalid whence
    };

    if new_pos < 0 {
        return -22;  // EINVAL - negative position invalid
    }

    file.set_pos(new_pos as u64);
    new_pos as isize
}

fn reg_file_close(_file: &File) -> i32 {
    // Currently no special handling needed
    // File destructor will handle resource cleanup automatically
    0
}

pub static REG_FILE_OPS: FileOps = FileOps {
    read: Some(reg_file_read),
    write: Some(reg_file_write),
    lseek: Some(reg_file_lseek),
    close: Some(reg_file_close),
    poll: None,
};

pub static REG_RO_FILE_OPS: FileOps = FileOps {
    read: Some(reg_file_read),
    write: None,
    lseek: Some(reg_file_lseek),
    close: Some(reg_file_close),
    poll: None,
};

/// Directory file operations (shared by all FS types).
/// Directories are read via getdents64, not via read().
fn dir_read_eisdir(_file: &File, _buf: &mut [u8]) -> isize {
    -21  // EISDIR
}

fn dir_close(_file: &File) -> i32 {
    0
}

pub static DIR_FILE_OPS: FileOps = FileOps {
    read: Some(dir_read_eisdir),
    write: None,
    lseek: None,
    close: Some(dir_close),
    poll: None,
};
