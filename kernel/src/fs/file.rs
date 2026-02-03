//! 文件对象和文件描述符管理
//!
//! 完全遵循 Linux 内核的文件对象设计 (fs/file.c, include/linux/fs.h)
//!
//! 核心概念：
//! - `struct file`: 打开的文件对象
//! - `fdtable`: 文件描述符表
//! - `struct file_operations`: 文件操作函数指针

use crate::fs::inode::{Inode, INodeOps};
use crate::fs::dentry::Dentry;
use crate::collection::SimpleArc;
use spin::Mutex;
use core::cell::UnsafeCell;
use core::mem::MaybeUninit;

/// 文件标志位
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
}

/// 文件操作函数指针表
///
/// 对应 Linux 的 struct file_operations (include/linux/fs.h)
#[repr(C)]
pub struct FileOps {
    /// 读取文件
    pub read: Option<unsafe fn(*mut File, *mut u8, usize) -> isize>,
    /// 写入文件
    pub write: Option<unsafe fn(*mut File, *const u8, usize) -> isize>,
    /// 定位文件位置
    pub lseek: Option<unsafe fn(*mut File, isize, i32) -> isize>,
    /// 关闭文件
    pub close: Option<unsafe fn(*mut File) -> i32>,
}

/// 文件对象
///
/// 对应 Linux 的 struct file (include/linux/fs.h)
#[repr(C)]
pub struct File {
    /// 文件标志
    pub flags: FileFlags,
    /// 文件位置
    pub pos: Mutex<u64>,
    /// 关联的 inode
    pub inode: UnsafeCell<Option<SimpleArc<Inode>>>,
    /// 关联的 dentry
    pub dentry: UnsafeCell<Option<SimpleArc<Dentry>>>,
    /// 文件操作函数
    pub ops: UnsafeCell<Option<&'static FileOps>>,
    /// 私有数据（用于设备特定数据）
    pub private_data: UnsafeCell<Option<*mut u8>>,
}

unsafe impl Sync for File {}

impl File {
    /// 创建新文件对象
    pub fn new(flags: FileFlags) -> Self {
        Self {
            flags,
            pos: Mutex::new(0),
            inode: UnsafeCell::new(None),
            dentry: UnsafeCell::new(None),
            ops: UnsafeCell::new(None),
            private_data: UnsafeCell::new(None),
        }
    }

    /// 设置 inode
    pub fn set_inode(&self, inode: SimpleArc<Inode>) {
        unsafe { *self.inode.get() = Some(inode); }
    }

    /// 设置 dentry
    pub fn set_dentry(&self, dentry: SimpleArc<Dentry>) {
        unsafe { *self.dentry.get() = Some(dentry); }
    }

    /// 设置文件操作
    pub fn set_ops(&self, ops: &'static FileOps) {
        unsafe { *self.ops.get() = Some(ops); }
    }

    /// 设置私有数据
    pub fn set_private_data(&self, data: *mut u8) {
        unsafe { *self.private_data.get() = Some(data); }
    }

    /// 读取文件
    pub unsafe fn read(&self, buf: *mut u8, count: usize) -> isize {
        if let Some(ops) = *self.ops.get() {
            if let Some(read_fn) = ops.read {
                return read_fn(self as *const _ as *mut _, buf, count);
            }
        }
        -9  // EBADF
    }

    /// 写入文件
    pub unsafe fn write(&self, buf: *const u8, count: usize) -> isize {
        if let Some(ops) = *self.ops.get() {
            if let Some(write_fn) = ops.write {
                return write_fn(self as *const _ as *mut _, buf, count);
            }
        }
        -9  // EBADF
    }

    /// 定位文件位置
    pub unsafe fn lseek(&self, offset: isize, whence: i32) -> isize {
        if let Some(ops) = *self.ops.get() {
            if let Some(lseek_fn) = ops.lseek {
                return lseek_fn(self as *const _ as *mut _, offset, whence);
            }
        }
        -9  // EBADF
    }

    /// 关闭文件
    pub unsafe fn close(&mut self) -> i32 {
        if let Some(ops) = *self.ops.get() {
            if let Some(close_fn) = ops.close {
                return close_fn(self);
            }
        }
        0
    }

    /// 获取当前位置
    pub fn get_pos(&self) -> u64 {
        *self.pos.lock()
    }

    /// 设置文件位置
    pub fn set_pos(&self, new_pos: u64) {
        *self.pos.lock() = new_pos;
    }
}

/// 文件描述符表
///
/// 对应 Linux 的 struct fdtable (include/linux/fdtable.h)
pub struct FdTable {
    /// 文件描述符数组 (每个进程最多 1024 个打开文件)
    fds: UnsafeCell<[Option<SimpleArc<File>>; 1024]>,
    /// 下一个可用的文件描述符
    next_fd: Mutex<usize>,
    /// 文件描述符数量
    count: Mutex<usize>,
}

unsafe impl Sync for FdTable {}

impl FdTable {
    /// 创建新的文件描述符表
    pub fn new() -> Self {
        // 使用 from_fn 初始化数组，避免 MaybeUninit 未定义行为
        let fds: [Option<SimpleArc<File>>; 1024] = core::array::from_fn(|_| None);

        Self {
            fds: UnsafeCell::new(fds),
            next_fd: Mutex::new(0),
            count: Mutex::new(0),
        }
    }

    /// 分配文件描述符
    pub fn alloc_fd(&self) -> Option<usize> {
        let mut next = self.next_fd.lock();
        let fds = unsafe { &mut *self.fds.get() };

        // 从 next_fd 开始搜索可用的文件描述符
        for i in 0..1024 {
            let fd = (*next + i) % 1024;
            if fds[fd].is_none() {
                *next = (fd + 1) % 1024;
                *self.count.lock() += 1;
                return Some(fd);
            }
        }

        None // 没有可用的文件描述符
    }

    /// 安装文件到文件描述符表
    pub fn install_fd(&self, fd: usize, file: SimpleArc<File>) -> Result<(), ()> {
        if fd >= 1024 {
            return Err(());
        }

        let fds = unsafe { &mut *self.fds.get() };

        if fds[fd].is_some() {
            return Err(()); // 文件描述符已被占用
        }

        fds[fd] = Some(file);
        Ok(())
    }

    /// 获取文件描述符对应的文件对象
    pub fn get_file(&self, fd: usize) -> Option<SimpleArc<File>> {
        if fd >= 1024 {
            return None;
        }
        let fds = unsafe { &*self.fds.get() };
        // TODO: SimpleArc 需要实现 clone
        None
    }

    /// 关闭文件描述符
    pub fn close_fd(&self, fd: usize) -> Result<(), ()> {
        if fd >= 1024 {
            return Err(());
        }

        let fds = unsafe { &mut *self.fds.get() };

        if fds[fd].is_none() {
            return Err(());
        }

        unsafe {
            if let Some(ref file) = fds[fd] {
                // 调用文件关闭操作
                // SimpleArc::as_ref 返回 &T，我们需要裸指针
                let file_ptr = file.as_ref() as *const File as *mut File;
                (*file_ptr).close();
            }
        }

        fds[fd] = None;
        *self.count.lock() -= 1;
        Ok(())
    }

    /// 复制文件描述符
    pub fn dup_fd(&self, oldfd: usize) -> Option<usize> {
        if oldfd >= 1024 {
            return None;
        }

        let file = self.get_file(oldfd)?;
        let newfd = self.alloc_fd()?;

        // TODO: SimpleArc 需要实现 clone 才能安装
        // self.install_fd(newfd, file).ok()?;
        None
    }
}

/// 获取当前进程的文件对象（通过文件描述符）
///
/// 对应 Linux 的 fget (fs/file.c)
pub unsafe fn get_file_fd(fd: usize) -> Option<SimpleArc<File>> {
    use crate::process::sched;
    sched::get_current_fdtable()?.get_file(fd)
}

/// 安装文件到当前进程的文件描述符表
///
/// 对应 Linux 的 fd_install (fs/file.c)
pub unsafe fn get_file_fd_install(file: SimpleArc<File>) -> Option<usize> {
    use crate::process::sched;
    let fdtable = sched::get_current_fdtable()?;
    let fd = fdtable.alloc_fd()?;
    fdtable.install_fd(fd, file).ok()?;
    Some(fd)
}

/// 关闭文件描述符
///
/// 对应 Linux 的 close 系统调用
pub unsafe fn close_file_fd(fd: usize) -> Result<(), i32> {
    use crate::process::sched;
    match sched::get_current_fdtable() {
        Some(fdtable) => fdtable.close_fd(fd).map_err(|_| -9_i32),  // EBADF
        None => Err(-9_i32),  // EBADF
    }
}

// ============================================================================
// 内核线程的标准输入输出
// ============================================================================

/// 获取 stdin (fd=0)
pub unsafe fn get_stdin() -> Option<SimpleArc<File>> {
    get_file_fd(0)
}

/// 获取 stdout (fd=1)
pub unsafe fn get_stdout() -> Option<SimpleArc<File>> {
    get_file_fd(1)
}

/// 获取 stderr (fd=2)
pub unsafe fn get_stderr() -> Option<SimpleArc<File>> {
    get_file_fd(2)
}

// ============================================================================
// 常规文件的默认操作
// ============================================================================

/// 常规文件读取操作
///
/// 对应 Linux 的 generic_file_read (mm/filemap.c)
unsafe fn reg_file_read(file: *mut File, buf: *mut u8, count: usize) -> isize {
    if let Some(ref inode) = *(*file).inode.get() {
        // 获取当前文件位置
        let offset = (*file).get_pos() as usize;

        // 检查用户缓冲区
        if buf.is_null() {
            return -14_isize;  // EFAULT
        }

        // 从 inode 读取数据
        let read_slice = core::slice::from_raw_parts_mut(buf, count);
        let bytes_read = inode.read_data(offset, read_slice);

        // 更新文件位置
        (*file).set_pos((offset + bytes_read) as u64);

        bytes_read as isize
    } else {
        -9_isize  // EBADF
    }
}

/// 常规文件写入操作
///
/// 对应 Linux 的 generic_file_write (mm/filemap.c)
unsafe fn reg_file_write(file: *mut File, buf: *const u8, count: usize) -> isize {
    if let Some(ref inode) = *(*file).inode.get() {
        // 获取当前文件位置
        let offset = (*file).get_pos() as usize;

        // 检查用户缓冲区
        if buf.is_null() {
            return -14_isize;  // EFAULT
        }

        // 写入数据到 inode
        let write_slice = core::slice::from_raw_parts(buf, count);
        let bytes_written = inode.write_data(offset, write_slice);

        // 更新文件位置
        (*file).set_pos((offset + bytes_written) as u64);

        bytes_written as isize
    } else {
        -9_isize  // EBADF
    }
}

/// 常规文件定位操作
///
/// 对应 Linux 的 generic_file_llseek (fs/read_write.c)
unsafe fn reg_file_lseek(file: *mut File, offset: isize, whence: i32) -> isize {
    // SEEK_SET = 0, SEEK_CUR = 1, SEEK_END = 2
    let current_pos = (*file).get_pos() as isize;

    // 获取文件大小
    let file_size = if let Some(ref inode) = *(*file).inode.get() {
        inode.get_size() as isize
    } else {
        return -9_isize;  // EBADF
    };

    let new_pos = match whence {
        0 => offset,                    // SEEK_SET
        1 => current_pos + offset,       // SEEK_CUR
        2 => file_size + offset,         // SEEK_END
        _ => return -22_isize,           // EINVAL - 无效的 whence
    };

    if new_pos < 0 {
        return -22_isize;  // EINVAL - 负的位置无效
    }

    (*file).set_pos(new_pos as u64);
    new_pos
}

/// 常规文件关闭操作
unsafe fn reg_file_close(_file: *mut File) -> i32 {
    // 目前不需要做特殊处理
    // File 的析构函数会自动处理资源清理
    0
}

/// 常规文件操作表
///
/// 对应 Linux 的 generic_ro_fops 和 generic_file_ops
pub static REG_FILE_OPS: FileOps = FileOps {
    read: Some(reg_file_read),
    write: Some(reg_file_write),
    lseek: Some(reg_file_lseek),
    close: Some(reg_file_close),
};

/// 只读文件操作表
pub static REG_RO_FILE_OPS: FileOps = FileOps {
    read: Some(reg_file_read),
    write: None,
    lseek: Some(reg_file_lseek),
    close: Some(reg_file_close),
};
