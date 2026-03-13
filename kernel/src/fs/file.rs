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
use spin::Mutex;
use core::cell::UnsafeCell;

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
}

#[repr(C, align(16))]
pub struct File {
    /// File flags
    pub flags: FileFlags,
    /// File position
    pub pos: Mutex<u64>,
    /// Associated inode
    pub inode: UnsafeCell<Option<Arc<Inode>>>,
    /// Associated dentry
    pub dentry: UnsafeCell<Option<Arc<Dentry>>>,
    /// File operation functions
    pub ops: UnsafeCell<Option<&'static FileOps>>,
    /// Private data (for device-specific data)
    pub private_data: UnsafeCell<Option<*mut u8>>,
    /// close-on-exec flag (FD_CLOEXEC)
    pub cloexec: Mutex<bool>,
}

unsafe impl Sync for File {}

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
            flags,
            pos: Mutex::new(0),
            inode: UnsafeCell::new(None),
            dentry: UnsafeCell::new(None),
            ops: UnsafeCell::new(None),
            private_data: UnsafeCell::new(None),
            cloexec: Mutex::new(false),  // Default: don't set close-on-exec
        }
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
        -9  // EBADF
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
}

// ============================================================================
// FdTable - Using Box allocation
// ============================================================================

/// FdTable entry stored on the heap
struct FdTableEntry {
    fds: [Option<Arc<File>>; 1024],
    next_fd: usize,
    count: usize,
}

pub struct FdTable {
    /// Heap-allocated entry with interior mutability
    entry: UnsafeCell<Box<FdTableEntry>>,
}

unsafe impl Sync for FdTable {}

impl FdTable {
    /// Create new file descriptor table
    pub fn new() -> Self {
        let entry = Box::new(FdTableEntry {
            fds: [const { None }; 1024],
            next_fd: 0,
            count: 0,
        });

        Self { entry: UnsafeCell::new(entry) }
    }

    /// Get entry reference
    fn entry(&self) -> &FdTableEntry {
        unsafe { &*(*self.entry.get()) }
    }

    /// Get entry mutable reference
    fn entry_mut(&self) -> &mut FdTableEntry {
        unsafe { &mut *(*self.entry.get()) }
    }

    /// Allocate file descriptor
    pub fn alloc_fd(&self) -> Option<usize> {
        let entry = self.entry();
        let mut next = entry.next_fd;

        for i in 0..1024 {
            let fd = (next + i) % 1024;
            if entry.fds[fd].is_none() {
                self.entry_mut().next_fd = (fd + 1) % 1024;
                return Some(fd);
            }
        }

        None
    }

    /// Install file to file descriptor table
    pub fn install_fd(&self, fd: usize, file: Arc<File>) -> Result<(), ()> {
        if fd >= 1024 {
            return Err(());
        }

        let entry = self.entry_mut();
        if entry.fds[fd].is_some() {
            return Err(());
        }
        entry.fds[fd] = Some(file);
        entry.count += 1;
        Ok(())
    }

    /// Get file object for file descriptor
    pub fn get_file(&self, fd: usize) -> Option<Arc<File>> {
        if fd >= 1024 {
            return None;
        }
        self.entry().fds[fd].clone()
    }

    /// Close file descriptor
    pub fn close_fd(&self, fd: usize) -> Result<(), ()> {
        if fd >= 1024 {
            return Err(());
        }

        let entry = self.entry_mut();
        if entry.fds[fd].is_none() {
            return Err(());
        }

        let file_opt = core::mem::replace(&mut entry.fds[fd], None);
        entry.count -= 1;

        // Call close operation if exists
        if let Some(file) = file_opt {
            unsafe {
                let file_ptr = Arc::as_ptr(&file) as *mut File;
                let ops_ptr = (*file_ptr).ops.get();
                if !ops_ptr.is_null() && !(*ops_ptr).is_none() {
                    (*file_ptr).close();
                }
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
    pub fn dup2_fd(&self, oldfd: usize, newfd: usize) -> Option<usize> {
        if oldfd >= 1024 || newfd >= 1024 {
            return None;
        }

        if oldfd == newfd {
            self.get_file(oldfd)?;
            return Some(newfd);
        }

        let file = self.get_file(oldfd)?;
        let _ = self.close_fd(newfd);
        self.install_fd(newfd, file).ok()?;
        Some(newfd)
    }
}

impl Drop for FdTable {
    fn drop(&mut self) {
        // Close all open files
        let entry = self.entry_mut();
        for fd in 0..1024 {
            if entry.fds[fd].is_some() {
                let _ = self.close_fd(fd);
            }
        }
        // Box will be automatically deallocated
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
        // Get current file position
        let offset = file.get_pos() as usize;

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
    let current_pos = file.get_pos() as isize;

    // Get file size
    let file_size = if let Some(ref inode) = unsafe { &*file.inode.get() } {
        inode.get_size() as isize
    } else {
        return -9  // EBADF
    };

    let new_pos = match whence {
        0 => offset,              // SEEK_SET
        1 => current_pos + offset, // SEEK_CUR
        2 => file_size + offset,   // SEEK_END
        _ => return -22,           // EINVAL - invalid whence
    };

    if new_pos < 0 {
        return -22;  // EINVAL - negative position invalid
    }

    file.set_pos(new_pos as u64);
    new_pos
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
};

pub static REG_RO_FILE_OPS: FileOps = FileOps {
    read: Some(reg_file_read),
    write: None,
    lseek: Some(reg_file_lseek),
    close: Some(reg_file_close),
};
