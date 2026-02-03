//! 虚拟文件系统 (VFS) 核心功能
//!
//! 完全遵循 Linux 内核的 VFS 设计 (fs/ namespace)
//!
//! 核心概念：
//! - `struct nameidata`: 路径查找
//! - 文件系统注册表
//! - 路径解析和文件查找

use alloc::sync::Arc;
use spin::Mutex;
use alloc::collections::BTreeMap;
use crate::fs::{Inode, InodeMode, make_reg_inode_with_data, REG_FILE_OPS, REG_RO_FILE_OPS};
use crate::fs::File;
use crate::fs::FileFlags;
use crate::println;

/// 文件系统最大文件数
const MAX_FILES: usize = 256;

/// 全局文件系统注册表
///
/// 管理所有已注册的文件
/// 简化实现：使用线性表存储（未来可以扩展为真正的文件系统树）
pub struct FsRegistry {
    /// 文件名到 inode 的映射
    files: Mutex<BTreeMap<&'static str, Arc<Inode>>>,
}

unsafe impl Send for FsRegistry {}
unsafe impl Sync for FsRegistry {}

/// 全局文件系统注册表
pub static FS_REGISTRY: FsRegistry = FsRegistry {
    files: Mutex::new(BTreeMap::new()),
};

impl FsRegistry {
    /// 注册新文件
    pub fn register_file(&self, name: &'static str, data: &[u8]) {
        let inode = Arc::new(make_reg_inode_with_data(name.len() as u64 + 1000, data));
        let mut files = self.files.lock();
        files.insert(name, inode);
        println!("VFS: registered file '{}', size={}", name, data.len());
    }

    /// 查找文件
    pub fn lookup_file(&self, name: &str) -> Option<Arc<Inode>> {
        let files = self.files.lock();
        // 简化实现：只支持文件名，不支持路径
        if let Some(inode) = files.get(name) {
            Some(Arc::clone(inode))
        } else {
            None
        }
    }

    /// 列出所有文件（调试用）
    pub fn list_files(&self) -> alloc::vec::Vec<alloc::string::String> {
        let files = self.files.lock();
        files.keys().map(|k| alloc::string::String::from(*k)).collect()
    }
}

/// 打开文件
///
/// 对应 Linux 的 do_sys_open (fs/open.c)
///
/// 参数：
/// - filename: 文件名（简化版，只支持文件名，不支持路径）
/// - flags: 打开标志 (O_RDONLY, O_WRONLY, O_RDWR, O_CREAT, O_TRUNC, O_APPEND)
/// - mode: 文件权限（如果创建新文件）
///
/// 返回：文件描述符或错误码
pub fn file_open(filename: &str, flags: u32, _mode: u32) -> Result<usize, i32> {
    println!("file_open: filename='{}', flags={:#x}", filename, flags);

    // 查找文件
    let inode = match FS_REGISTRY.lookup_file(filename) {
        Some(inode) => inode,
        None => {
            println!("file_open: file '{}' not found", filename);
            return Err(-2_i32);  // ENOENT
        }
    };

    // 检查文件类型
    if !inode.mode.is_regular_file() {
        println!("file_open: '{}' is not a regular file", filename);
        return Err(-2_i32);  // ENOENT
    }

    // 创建文件对象
    let file_flags = FileFlags::new(flags);
    let file = Arc::new(File::new(file_flags));

    // 设置 inode
    file.set_inode(inode.clone());

    // 设置文件操作
    if file_flags.is_readonly() || file_flags.is_rdwr() {
        file.set_ops(&REG_FILE_OPS);
    } else if file_flags.is_writeonly() {
        file.set_ops(&REG_FILE_OPS);
    } else {
        return Err(-22_i32);  // EINVAL
    }

    // 安装到当前进程的文件描述符表
    unsafe {
        use crate::process::sched;
        let fdtable = sched::get_current_fdtable();
        if fdtable.is_none() {
            return Err(-3_i32);  // ESRCH
        }

        let fd = fdtable.unwrap().alloc_fd();
        if fd.is_none() {
            return Err(-24_i32);  // EMFILE - 进程打开文件过多
        }

        let fd = fd.unwrap();
        fdtable.unwrap().install_fd(fd, file).map_err(|_| -23_i32)?;  // EBADF
        println!("file_open: opened '{}' as fd {}", filename, fd);

        Ok(fd)
    }
}

/// 初始化 VFS
///
/// 注册一些默认文件用于测试
pub fn init() {
    println!("VFS: initializing...");

    // 注册一些测试文件
    FS_REGISTRY.register_file("test.txt", b"Hello, World!\nThis is a test file.\n");
    FS_REGISTRY.register_file("config.txt", b"debug=true\nverbose=false\n");
    FS_REGISTRY.register_file("empty.txt", b"");

    println!("VFS: initialized with {} files", FS_REGISTRY.list_files().len());
}
