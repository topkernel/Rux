//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! 超级块和文件系统类型管理
//!
//! 完全遵循 Linux 内核的超级块设计 (fs/super.c, include/linux/fs.h)
//!
//! 核心概念：
//! - `struct super_block`: 超级块，表示一个已挂载的文件系统
//! - `struct file_system_type`: 文件系统类型，用于注册和挂载
//! - `struct vfsmount`: 挂载点，表示文件系统在命名空间中的位置

use crate::errno;
use alloc::sync::Arc;
use spin::Mutex;

#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct SuperBlockFlags(u64);

impl SuperBlockFlags {
    /// 只读挂载
    pub const SB_RDONLY: u64 = 1;
    /// 不更新 atime
    pub const SB_NOATIME: u64 = 1 << 5;
    /// 不更新 atime/mtime/ctime
    pub const SB_NODIRATIME: u64 = 1 << 6;
    /// 强制同步写入
    pub const SB_SYNCHRONOUS: u64 = 1 << 7;
    /// 禁止挂载
    pub const SB_MANDLOCK: u64 = 1 << 8;
    /// 不写入到设备
    pub const SB_DIRSYNC: u64 = 1 << 9;
    /// 不更新 atime
    pub const SB_NOSEC: u64 = 1 << 10;
    /// 活动挂载
    pub const SB_ACTIVE: u64 = 1 << 11;
    /// 正在写入
    pub const SB_WRITERS: u64 = 1 << 12;

    pub fn new(flags: u64) -> Self {
        Self(flags)
    }

    pub fn is_readonly(&self) -> bool {
        (self.0 & Self::SB_RDONLY) != 0
    }

    pub fn is_active(&self) -> bool {
        (self.0 & Self::SB_ACTIVE) != 0
    }

    pub fn bits(&self) -> u64 {
        self.0
    }
}

#[repr(C)]
pub struct SuperBlock {
    /// 文件系统标志
    pub s_flags: SuperBlockFlags,
    /// 块大小
    pub s_blocksize: usize,
    /// 块大小位数
    pub s_blocksize_bits: u8,
    /// 文件系统魔数
    pub s_magic: u32,
    /// 最大文件名长度
    pub s_max_links: u32,
    /// 根 inode
    pub s_root: Option<Arc<()>>,
    /// 文件系统类型
    pub s_type: Option<&'static FileSystemType>,
    /// 挂载选项
    pub s_options: Option<Arc<()>>,
    /// 私有数据（用于特定文件系统）
    pub s_fs_info: Option<*mut u8>,
}

unsafe impl Send for SuperBlock {}
unsafe impl Sync for SuperBlock {}

impl SuperBlock {
    /// 创建新超级块
    pub fn new(blocksize: usize, magic: u32) -> Self {
        // 计算块大小位数
        let mut bits = 0u8;
        let mut size = blocksize;
        while size > 1 {
            size >>= 1;
            bits += 1;
        }

        Self {
            s_flags: SuperBlockFlags::new(SuperBlockFlags::SB_RDONLY),
            s_blocksize: blocksize,
            s_blocksize_bits: bits,
            s_magic: magic,
            s_max_links: 0,
            s_root: None,
            s_type: None,
            s_options: None,
            s_fs_info: None,
        }
    }

    /// 设置文件系统类型
    pub fn set_type(&mut self, fs_type: &'static FileSystemType) {
        self.s_type = Some(fs_type);
    }

    /// 设置私有数据
    pub fn set_fs_info(&mut self, info: *mut u8) {
        self.s_fs_info = Some(info);
    }

    /// 设置标志
    pub fn set_flags(&mut self, flags: SuperBlockFlags) {
        self.s_flags = flags;
    }
}

pub struct FsContext<'a> {
    /// 源设备
    pub source: Option<&'a str>,
    /// 挂载目标
    pub target: Option<&'a str>,
    /// 挂载标志
    pub ms_flags: u64,
    /// 数据选项
    pub data: Option<&'a str>,
}

impl<'a> FsContext<'a> {
    /// 创建新的挂载上下文
    pub fn new(
        source: Option<&'a str>,
        target: Option<&'a str>,
        ms_flags: u64,
    ) -> Self {
        Self {
            source,
            target,
            ms_flags,
            data: None,
        }
    }
}

#[repr(C)]
pub struct FileSystemType {
    /// 文件系统名称
    pub name: &'static str,
    /// 获取超级块（挂载时调用）
    pub mount: Option<unsafe extern "C" fn(&FsContext<'_>) -> Result<*mut SuperBlock, i32>>,
    /// 杀死超级块（卸载时调用）
    pub kill_sb: Option<unsafe extern "C" fn(*mut SuperBlock)>,
    /// 文件系统标志
    pub fs_flags: u64,
}

impl FileSystemType {
    /// 创建新文件系统类型
    pub const fn new(
        name: &'static str,
        mount: Option<unsafe extern "C" fn(&FsContext<'_>) -> Result<*mut SuperBlock, i32>>,
        kill_sb: Option<unsafe extern "C" fn(*mut SuperBlock)>,
        fs_flags: u64,
    ) -> Self {
        Self {
            name,
            mount,
            kill_sb,
            fs_flags,
        }
    }

    /// 挂载文件系统
    ///
    /// 对应 Linux 的 vfs_kern_mount (fs/namespace.c)
    pub unsafe fn mount_fs(
        &self,
        source: Option<&str>,
        target: Option<&str>,
        flags: u64,
    ) -> Result<*mut SuperBlock, i32> {
        // 创建挂载上下文
        let fc = FsContext::new(source, target, flags);

        // 调用文件系统特定的挂载函数
        if let Some(mount_fn) = self.mount {
            mount_fn(&fc)
        } else {
            Err(errno::Errno::FunctionNotImplemented.as_neg_i32())
        }
    }

    /// 卸载文件系统
    ///
    /// 对应 Linux 的 deactivate_locked_super (fs/super.c)
    pub unsafe fn kill_super(&self, sb: *mut SuperBlock) {
        if let Some(kill_fn) = self.kill_sb {
            kill_fn(sb);
        }
    }
}

struct FsRegistry {
    /// 文件系统类型列表
    fs_types: Mutex<[Option<&'static FileSystemType>; 32]>,
}

unsafe impl Send for FsRegistry {}
unsafe impl Sync for FsRegistry {}

impl FsRegistry {
    pub const fn new() -> Self {
        Self {
            fs_types: Mutex::new([None; 32]),
        }
    }

    /// 注册文件系统类型
    ///
    /// 对应 Linux 的 register_filesystem (fs/filesystems.c)
    pub fn register(&self, fs_type: &'static FileSystemType) -> Result<(), i32> {
        crate::println!("fs: register: acquiring lock...");
        let mut registry = self.fs_types.lock();
        crate::println!("fs: register: lock acquired, searching for slot...");

        // 查找空闲槽位
        for i in 0..32 {
            if registry[i].is_none() {
                registry[i] = Some(fs_type);
                return Ok(());
            }
        }

        Err(errno::Errno::NoSpaceLeftOnDevice.as_neg_i32())
    }

    /// 注销文件系统类型
    ///
    /// 对应 Linux 的 unregister_filesystem (fs/filesystems.c)
    pub fn unregister(&self, fs_type: &'static FileSystemType) -> Result<(), i32> {
        let mut registry = self.fs_types.lock();

        // 查找并移除文件系统类型
        for i in 0..32 {
            if let Some(ft) = registry[i] {
                if core::ptr::eq(ft, fs_type) {
                    registry[i] = None;
                    return Ok(());
                }
            }
        }

        Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32())
    }

    /// 查找文件系统类型
    ///
    /// 对应 Linux 的 get_fs_type (fs/filesystems.c)
    pub fn get(&self, name: &str) -> Option<&'static FileSystemType> {
        let registry = self.fs_types.lock();

        for i in 0..32 {
            if let Some(fs_type) = registry[i] {
                if fs_type.name == name {
                    return Some(fs_type);
                }
            }
        }

        None
    }
}

static FS_REGISTRY: FsRegistry = FsRegistry::new();

pub fn register_filesystem(fs_type: &'static FileSystemType) -> Result<(), i32> {
    FS_REGISTRY.register(fs_type)
}

pub fn unregister_filesystem(fs_type: &'static FileSystemType) -> Result<(), i32> {
    FS_REGISTRY.unregister(fs_type)
}

pub fn get_fs_type(name: &str) -> Option<&'static FileSystemType> {
    FS_REGISTRY.get(name)
}

pub unsafe fn do_mount(
    dev_name: Option<&str>,
    dir_name: Option<&str>,
    type_name: &str,
    flags: u64,
    _data: Option<&str>,
) -> Result<(), i32> {
    // 查找文件系统类型
    let fs_type = get_fs_type(type_name).ok_or(-2_i32)?;  // ENOENT

    // 挂载文件系统
    let _sb = fs_type.mount_fs(dev_name, dir_name, flags)?;

    // TODO: 创建 vfsmount 结构
    // TODO: 将挂载点添加到命名空间

    Ok(())
}

pub unsafe fn do_umount(_target: &str, _flags: u64) -> Result<(), i32> {
    // TODO: 查找挂载点
    // TODO: 检查挂载点是否被使用
    // TODO: 调用文件系统的 kill_sb

    Err(errno::Errno::FunctionNotImplemented.as_neg_i32())
}

#[cfg(test)]
mod tests {
    use super::*;

    // 测试文件系统类型
    extern "C" fn test_mount(_fc: &FsContext) -> Result<*mut SuperBlock, i32> {
        // 简单地返回一个新的超级块
        let sb = Box::new(SuperBlock::new(4096, 0x1234));
        Ok(Box::into_raw(sb) as *mut SuperBlock)
    }

    extern "C" fn test_kill_sb(_sb: *mut SuperBlock) {
        // 简单地什么都不做
    }

    #[test]
    fn test_fs_registry() {
        // 创建测试文件系统类型
        let test_fs = FileSystemType::new(
            "testfs",
            Some(test_mount),
            Some(test_kill_sb),
            0,
        );

        // 注册文件系统
        assert!(register_filesystem(&test_fs).is_ok());

        // 查找文件系统
        assert!(get_fs_type("testfs").is_some());
        assert!(get_fs_type("nonexistent").is_none());

        // 注销文件系统
        assert!(unregister_filesystem(&test_fs).is_ok());
        assert!(get_fs_type("testfs").is_none());
    }

    #[test]
    fn test_superblock_flags() {
        let flags = SuperBlockFlags::new(SuperBlockFlags::SB_RDONLY | SuperBlockFlags::SB_ACTIVE);
        assert!(flags.is_readonly());
        assert!(flags.is_active());

        let flags2 = SuperBlockFlags::new(SuperBlockFlags::SB_RDONLY);
        assert!(flags2.is_readonly());
        assert!(!flags2.is_active());
    }
}
