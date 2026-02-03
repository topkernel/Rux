//! 虚拟文件系统 (VFS) 核心功能
//!
//! 完全遵循 Linux 内核的 VFS 设计 (fs/ namespace)
//!
//! 核心概念：
//! - `struct nameidata`: 路径查找
//! - `struct super_block`: 超级块
//! - `struct file_system_type`: 文件系统类型
//! - 文件系统注册表
//! - 路径解析和文件查找

use alloc::sync::Arc;
use alloc::borrow::ToOwned;
use spin::Mutex;
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use crate::fs::{Inode, InodeMode, make_reg_inode_with_data, make_dir_inode, REG_FILE_OPS, REG_RO_FILE_OPS};
use crate::fs::{Dentry, make_root_dentry};
use crate::fs::File;
use crate::fs::FileFlags;
use crate::println;

/// 文件系统最大文件数
const MAX_FILES: usize = 256;

/// 路径最大长度
const PATH_MAX: usize = 4096;

/// 文件系统类型
///
/// 对应 Linux 的 struct file_system_type (include/linux/fs.h)
#[repr(C)]
pub struct FileSystemType {
    /// 文件系统名称
    pub name: &'static str,
    /// 获取超级块
    pub get_sb: Option<fn() -> Arc<SuperBlock>>,
    /// 杀死超级块
    pub kill_sb: Option<fn(sb: Arc<SuperBlock>)>,
}

/// 超级块
///
/// 对应 Linux 的 struct super_block (include/linux/fs.h)
///
/// 超级块表示一个已挂载的文件系统
pub struct SuperBlock {
    /// 文件系统类型
    pub fs_type: &'static FileSystemType,
    /// 根目录项
    pub root: Mutex<Option<Arc<Dentry>>>,
    /// 文件系统标志
    pub flags: Mutex<u64>,
    /// 文件系统私有数据
    pub s_fs_info: Mutex<Option<*mut u8>>,
    /// 文件系统魔数（用于识别）
    pub s_magic: Mutex<u32>,
}

unsafe impl Send for SuperBlock {}
unsafe impl Sync for SuperBlock {}

impl SuperBlock {
    /// 创建新的超级块
    pub fn new(fs_type: &'static FileSystemType) -> Self {
        Self {
            fs_type,
            root: Mutex::new(None),
            flags: Mutex::new(0),
            s_fs_info: Mutex::new(None),
            s_magic: Mutex::new(0),
        }
    }

    /// 设置根目录
    pub fn set_root(&self, dentry: Arc<Dentry>) {
        *self.root.lock() = Some(dentry);
    }

    /// 获取根目录
    pub fn get_root(&self) -> Option<Arc<Dentry>> {
        self.root.lock().clone()
    }
}

/// 路径查找上下文 (nameidata)
///
/// 对应 Linux 的 struct nameidata (include/linux/namei.h)
///
/// 用于路径解析和文件查找
pub struct NameiData {
    /// 当前路径深度
    pub depth: usize,
    /// 最后访问的 dentry
    pub dentry: Option<Arc<Dentry>>,
    /// 起始目录
    pub base: Option<Arc<Dentry>>,
    /// 路径标志
    pub flags: u32,
}

/// 路径查找标志
pub mod namei_flags {
    /// 符号链接跟随
    pub const LOOKUP_FOLLOW: u32 = 0x0001;
    /// 目录查找
    pub const LOOKUP_DIRECTORY: u32 = 0x0002;
    /// 继续查找
    pub const LOOKUP_CONTINUE: u32 = 0x0004;
    /// 不创建
    pub const LOOKUP_NO_SYMLINKS: u32 = 0x0008;
    /// 自动挂载
    pub const LOOKUP_AUTOMOUNT: u32 = 0x0010;
    /// 父目录
    pub const LOOKUP_PARENT: u32 = 0x0020;
    /// 返回重命名
    pub const LOOKUP_RENAME: u32 = 0x0040;
    /// 父目录重命名
    pub const LOOKUP_REVAL: u32 = 0x0080;
    /// 创建
    pub const LOOKUP_CREATE: u32 = 0x0100;
    /// 排他性
    pub const LOOKUP_EXCL: u32 = 0x0200;
}

impl NameiData {
    /// 创建新的路径查找上下文
    pub fn new() -> Self {
        Self {
            depth: 0,
            dentry: None,
            base: None,
            flags: 0,
        }
    }

    /// 设置起始目录
    pub fn set_base(&mut self, dentry: Arc<Dentry>) {
        self.base = Some(dentry);
    }
}

/// 全局文件系统注册表
///
/// 管理所有已挂载的文件系统
pub struct FsRegistry {
    /// 已挂载的文件系统
    mounted: Mutex<BTreeMap<String, Arc<SuperBlock>>>,
    /// 全局根文件系统
    root_fs: Mutex<Option<Arc<SuperBlock>>>,
}

unsafe impl Send for FsRegistry {}
unsafe impl Sync for FsRegistry {}

/// 全局文件系统注册表
pub static FS_REGISTRY: FsRegistry = FsRegistry {
    mounted: Mutex::new(BTreeMap::new()),
    root_fs: Mutex::new(None),
};

impl FsRegistry {
    /// 注册文件系统
    pub fn register_fs(&self, mount_point: String, sb: Arc<SuperBlock>) {
        let mut mounted = self.mounted.lock();
        mounted.insert(mount_point, sb);
        println!("VFS: registered filesystem");
    }

    /// 设置根文件系统
    pub fn set_root_fs(&self, sb: Arc<SuperBlock>) {
        *self.root_fs.lock() = Some(sb);
    }

    /// 获取根文件系统
    pub fn get_root_fs(&self) -> Option<Arc<SuperBlock>> {
        self.root_fs.lock().clone()
    }

    /// 路径解析 - 查找路径对应的 dentry
    ///
    /// 对应 Linux 的 path_lookup (fs/namei.c)
    pub fn path_lookup(&self, path: &str) -> Option<Arc<Dentry>> {
        let root_fs = self.get_root_fs()?;
        let root_dentry = root_fs.get_root()?;

        // 分割路径
        let components: Vec<&str> = path.split('/')
            .filter(|s| !s.is_empty())
            .collect();

        if components.is_empty() {
            return Some(root_dentry.clone());
        }

        // 简化实现：从根目录开始逐级查找
        let mut current_dentry = root_dentry.clone();
        let mut current_path = String::new();

        for component in &components {
            current_path.push('/');
            current_path.push_str(component);

            // 简化实现：在内存树中查找
            // 实际实现需要访问父 inode 的 lookup 操作
            if let Some(child) = self.lookup_child(&current_dentry, component) {
                current_dentry = child;
            } else {
                return None;
            }
        }

        Some(current_dentry)
    }

    /// 查找子目录项
    fn lookup_child(&self, parent: &Dentry, name: &str) -> Option<Arc<Dentry>> {
        // 简化实现：这里需要实际的目录结构
        // 当前只是占位符，实际需要从 inode 的目录数据中读取
        None
    }
}

/// 全局文件系统注册表（简化版，用于向后兼容）
///
/// 管理所有已注册的文件
/// 简化实现：使用线性表存储（未来可以扩展为真正的文件系统树）
pub struct LegacyFsRegistry {
    /// 文件名到 inode 的映射
    files: Mutex<BTreeMap<String, Arc<Inode>>>,
}

unsafe impl Send for LegacyFsRegistry {}
unsafe impl Sync for LegacyFsRegistry {}

/// 全局文件系统注册表（遗留）
pub static LEGACY_FS_REGISTRY: LegacyFsRegistry = LegacyFsRegistry {
    files: Mutex::new(BTreeMap::new()),
};

impl LegacyFsRegistry {
    /// 注册新文件
    pub fn register_file(&self, name: String, data: &[u8]) {
        let inode = Arc::new(make_reg_inode_with_data(name.len() as u64 + 1000, data));
        let mut files = self.files.lock();
        files.insert(name, inode);
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
    pub fn list_files(&self) -> Vec<String> {
        let files = self.files.lock();
        files.keys().map(|k| k.clone()).collect()
    }
}

/// 目录项结构
///
/// 对应 Linux 的 struct linux_dirent64 (include/linux/dirent.h)
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct LinuxDirent64 {
    /// inode 编号
    pub d_ino: u64,
    /// 文件位置偏移
    pub d_off: u64,
    /// 记录长度
    pub d_reclen: u16,
    /// 文件类型
    pub d_type: u8,
    /// 文件名（可变长度）
    pub d_name: [u8; 0],
}

/// 目录项类型
pub mod d_type {
    /// 未知类型
    pub const DT_UNKNOWN: u8 = 0;
    /// 常规文件
    pub const DT_REG: u8 = 1;
    /// 目录
    pub const DT_DIR: u8 = 2;
    /// 字符设备
    pub const DT_CHR: u8 = 3;
    /// 块设备
    pub const DT_BLK: u8 = 4;
    /// FIFO
    pub const DT_FIFO: u8 = 5;
    /// Socket
    pub const DT_SOCK: u8 = 6;
    /// 符号链接
    pub const DT_LNK: u8 = 7;
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
    let inode = match LEGACY_FS_REGISTRY.lookup_file(filename) {
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

/// 创建目录
///
/// 对应 Linux 的 sys_mkdir (fs/namei.c)
///
/// 参数：
/// - pathname: 目录路径
/// - mode: 目录权限
///
/// 返回：0 表示成功，负数表示错误码
pub fn sys_mkdir(pathname: &str, mode: u32) -> i32 {
    println!("sys_mkdir: pathname='{}', mode={:#o}", pathname, mode);

    // 简化实现：总是返回 ENOSYS
    // TODO: 实现真正的目录创建
    // mkdir 需要：
    // 1. 路径解析
    // 2. 检查父目录是否存在
    // 3. 检查目录名是否已存在
    // 4. 创建新的目录 inode
    // 5. 创建新的 dentry
    // 6. 添加到父目录

    -38_i32  // ENOSYS
}

/// 删除目录
///
/// 对应 Linux 的 sys_rmdir (fs/namei.c)
///
/// 参数：
/// - pathname: 目录路径
///
/// 返回：0 表示成功，负数表示错误码
pub fn sys_rmdir(pathname: &str) -> i32 {
    println!("sys_rmdir: pathname='{}'", pathname);

    // 简化实现：总是返回 ENOSYS
    // TODO: 实现真正的目录删除
    // rmdir 需要：
    // 1. 路径解析
    // 2. 检查目录是否为空
    // 3. 从父目录中删除
    // 4. 释放 dentry 和 inode

    -38_i32  // ENOSYS
}

/// 读取目录项
///
/// 对应 Linux 的 sys_getdents64 (fs/readdir.c)
///
/// 参数：
/// - fd: 文件描述符
/// - dirent: 目录项缓冲区
/// - count: 缓冲区大小
///
/// 返回：读取的字节数，负数表示错误码
pub fn sys_getdents64(fd: usize, dirent: *mut LinuxDirent64, count: usize) -> isize {
    println!("sys_getdents64: fd={}, dirent={:#x}, count={}", fd, dirent as usize, count);

    // 简化实现：总是返回 0（目录为空）
    // TODO: 实现真正的目录读取
    // getdents64 需要：
    // 1. 获取文件对象
    // 2. 检查是否为目录
    // 3. 读取目录内容
    // 4. 填充 dirent 结构
    // 5. 返回读取的字节数

    0
}

/// 初始化 VFS
///
/// 注册一些默认文件用于测试
pub fn init() {
    println!("VFS: initializing...");

    // 简化实现：只注册测试文件
    // 注册一些测试文件（遗留方式）
    LEGACY_FS_REGISTRY.register_file("test.txt".to_owned(), b"Hello, World!\nThis is a test file.\n");
    LEGACY_FS_REGISTRY.register_file("config.txt".to_owned(), b"debug=true\nverbose=false\n");
    LEGACY_FS_REGISTRY.register_file("empty.txt".to_owned(), b"");

    println!("VFS: initialized with {} files", LEGACY_FS_REGISTRY.list_files().len());
}
