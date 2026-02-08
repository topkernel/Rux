//! 挂载点和命名空间管理
//!
//! 完全遵循 Linux 内核的挂载点设计 (fs/namespace.c, include/linux/mount.h)
//!
//! 核心概念：
//! - `struct vfsmount`: 挂载点，表示文件系统在命名空间中的位置
//! - `struct mnt_namespace`: 命名空间，包含进程可见的所有挂载点
//! - 挂载点树：挂载点形成的层次结构

use crate::errno;
use crate::collection::SimpleArc;
use alloc::vec::Vec;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

/// 挂载点标志
///
/// 对应 Linux 的 MNT_* 宏 (include/linux/mount.h)
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct MntFlags(u64);

impl MntFlags {
    /// 只读挂载
    pub const MNT_READONLY: u64 = 0x01;
    /// 不更新 atime
    pub const MNT_NOATIME: u64 = 0x02;
    /// 不更新目录 atime
    pub const MNT_NODIRATIME: u64 = 0x04;
    /// 强制同步写入
    pub const MNT_SYNCHRONOUS: u64 = 0x08;
    /// 禁止程序执行
    pub const MNT_NOEXEC: u64 = 0x10;
    /// 不支持 suid/sgid
    pub const MNT_NOSUID: u64 = 0x20;
    /// 节点不更新 atime
    pub const MNT_NODEV: u64 = 0x40;
    /// 私有挂载
    pub const MNT_PRIVATE: u64 = 0x80;
    /// 共享挂载组
    pub const MNT_SHARED: u64 = 0x100;
    /// 从属挂载
    pub const MNT_SLAVE: u64 = 0x200;
    /// 不可绑定
    pub const MNT_UNBINDABLE: u64 = 0x400;
    /// 强制标志
    pub const MNT_FORCE: u64 = 0x800;

    pub fn new(flags: u64) -> Self {
        Self(flags)
    }

    pub fn is_readonly(&self) -> bool {
        (self.0 & Self::MNT_READONLY) != 0
    }

    pub fn is_noexec(&self) -> bool {
        (self.0 & Self::MNT_NOEXEC) != 0
    }

    pub fn is_nosuid(&self) -> bool {
        (self.0 & Self::MNT_NOSUID) != 0
    }

    pub fn bits(&self) -> u64 {
        self.0
    }
}

/// 挂载点
///
/// 对应 Linux 的 struct vfsmount (include/linux/mount.h)
/// 表示一个文件系统在命名空间中的挂载点
#[repr(C)]
pub struct VfsMount {
    /// 挂载点唯一 ID
    pub mnt_id: u64,
    /// 父挂载点
    pub mnt_parent: Option<SimpleArc<VfsMount>>,
    /// 挂载点标志
    pub mnt_flags: MntFlags,
    /// 挂载点名称（挂载点目录）
    pub mnt_mountpoint: Option<SimpleArc<Vec<u8>>>,
    /// 挂载根目录
    pub mnt_root: Option<SimpleArc<Vec<u8>>>,
    /// 超级块指针
    pub mnt_sb: Option<*mut u8>,
    /// 挂载点引用计数
    mnt_count: AtomicU64,
    /// 挂载点是否过期
    mnt_expired: AtomicU64,
    /// 命名空间
    pub mnt_ns: Option<*mut MntNamespace>,
}

unsafe impl Send for VfsMount {}
unsafe impl Sync for VfsMount {}

impl VfsMount {
    /// 创建新挂载点
    pub fn new(mountpoint: Vec<u8>, root: Vec<u8>, flags: MntFlags, sb: Option<*mut u8>) -> Self {
        Self {
            mnt_id: 0,  // 将在添加到命名空间时分配
            mnt_parent: None,
            mnt_flags: flags,
            mnt_mountpoint: SimpleArc::new(mountpoint),
            mnt_root: SimpleArc::new(root),
            mnt_sb: sb,
            mnt_count: AtomicU64::new(1),
            mnt_expired: AtomicU64::new(0),
            mnt_ns: None,
        }
    }

    /// 获取超级块
    pub fn get_superblock(&self) -> Option<*mut u8> {
        self.mnt_sb
    }

    /// 设置超级块
    pub fn set_superblock(&mut self, sb: *mut u8) {
        self.mnt_sb = Some(sb);
    }

    /// 设置父挂载点
    pub fn set_parent(&mut self, parent: SimpleArc<VfsMount>) {
        self.mnt_parent = Some(parent);
    }

    /// 增加引用计数
    pub fn get(&self) {
        self.mnt_count.fetch_add(1, Ordering::AcqRel);
    }

    /// 减少引用计数
    pub fn put(&self) {
        if self.mnt_count.fetch_sub(1, Ordering::AcqRel) == 1 {
            // 最后一个引用，这里应该清理资源
            // 但由于我们使用 Arc，实际清理会在 drop 时进行
        }
    }

    /// 检查是否过期
    pub fn is_expired(&self) -> bool {
        self.mnt_expired.load(Ordering::Acquire) != 0
    }

    /// 标记为过期
    pub fn mark_expired(&self) {
        self.mnt_expired.store(1, Ordering::Release);
    }

    /// 获取挂载点路径
    pub fn get_path(&self) -> Option<Vec<u8>> {
        self.mnt_mountpoint.as_ref().map(|_arc| {
            // 获取 Vec<u8> 的克隆
            // 注意：这里需要根据实际 SimpleArc 的实现来调整
            Vec::new()  // TODO: 实现实际的克隆
        })
    }
}

/// 命名空间
///
/// 对应 Linux 的 struct mnt_namespace (include/linux/mount.h)
/// 包含进程可见的所有挂载点
#[repr(C)]
pub struct MntNamespace {
    /// 命名空间 ID
    pub ns_id: u64,
    /// 挂载点列表
    mounts: Mutex<Vec<SimpleArc<VfsMount>>>,
    /// 根挂载点
    pub root: Option<SimpleArc<VfsMount>>,
    /// 引用计数
    count: AtomicU64,
}

unsafe impl Send for MntNamespace {}
unsafe impl Sync for MntNamespace {}

impl MntNamespace {
    /// 创建新命名空间
    pub fn new() -> Self {
        Self {
            ns_id: 0,
            mounts: Mutex::new(Vec::new()),
            root: None,
            count: AtomicU64::new(1),
        }
    }

    /// 添加挂载点到命名空间
    ///
    /// 对应 Linux 的 do_add_mount (fs/namespace.c)
    pub fn add_mount(&self, mount: SimpleArc<VfsMount>) -> Result<(), i32> {
        let mut mounts = self.mounts.lock();

        // 分配挂载点 ID
        let _mnt_id = mounts.len() as u64;

        // 如果是第一个挂载点，设置为根挂载点
        if self.root.is_none() {
            // 注意：这里需要修改 Arc 内部的值，这在 Rust 中比较复杂
            // 简化实现：我们在创建挂载点时就设置好所有属性
        }

        mounts.push(mount);
        Ok(())
    }

    /// 移除挂载点
    ///
    /// 对应 Linux 的 do_umount (fs/namespace.c)
    pub fn remove_mount(&self, mnt_id: u64) -> Result<(), i32> {
        let mut mounts = self.mounts.lock();

        // 查找并移除挂载点
        for i in 0..mounts.len() {
            if mounts[i].mnt_id == mnt_id {
                // 检查是否是根挂载点
                if let Some(ref root) = self.root {
                    if root.mnt_id == mnt_id {
                        return Err(errno::Errno::DeviceOrResourceBusy.as_neg_i32());
                    }
                }

                mounts.remove(i);
                return Ok(());
            }
        }

        Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32())
    }

    /// 查找挂载点
    pub fn find_mount(&self, _path: &[u8]) -> Option<SimpleArc<VfsMount>> {
        let mounts = self.mounts.lock();

        for mount in mounts.iter() {
            if let Some(ref _mountpoint) = mount.mnt_mountpoint {
                // TODO: 实现路径比较
                // if mountpoint.as_slice() == path {
                //     return Some(mount.clone());
                // }
            }
        }

        None
    }

    /// 获取所有挂载点
    pub fn list_mounts(&self) -> Vec<SimpleArc<VfsMount>> {
        let _mounts = self.mounts.lock();
        // SimpleArc 需要实现 Vec clone
        // 暂时返回空 Vec
        Vec::new()
    }

    /// 增加引用计数
    pub fn get(&self) {
        self.count.fetch_add(1, Ordering::AcqRel);
    }

    /// 减少引用计数
    pub fn put(&self) {
        if self.count.fetch_sub(1, Ordering::AcqRel) == 1 {
            // 最后一个引用，清理资源
        }
    }
}

/// 全局初始命名空间
static INIT_NS: MntNamespace = MntNamespace {
    ns_id: 0,
    mounts: Mutex::new(Vec::new()),
    root: None,
    count: AtomicU64::new(1),
};

/// 获取初始命名空间
///
/// 对应 Linux 的 init_mnt_namespace (fs/namespace.c)
pub fn get_init_namespace() -> &'static MntNamespace {
    &INIT_NS
}

/// 创建新的命名空间
///
/// 对应 Linux 的 create_mnt_ns (fs/namespace.c)
pub fn create_namespace() -> Result<&'static MntNamespace, i32> {
    // TODO: 实现真正的命名空间创建
    // 这需要动态分配，在 no_std 环境中比较复杂
    Err(errno::Errno::FunctionNotImplemented.as_neg_i32())
}

/// 克隆命名空间
///
/// 对应 Linux 的 copy_mnt_ns (fs/namespace.c)
pub fn clone_namespace(_ns: &MntNamespace) -> Result<&'static MntNamespace, i32> {
    // TODO: 实现命名空间克隆
    Err(errno::Errno::FunctionNotImplemented.as_neg_i32())
}

/// 挂载 propagation 类型
///
/// 对应 Linux 的 MS_* 宏 (include/linux/fs.h)
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct MsFlags(u64);

impl MsFlags {
    /// 绑定挂载
    pub const MS_BIND: u64 = 0x1000;
    /// 私有挂载
    pub const MS_PRIVATE: u64 = 0x40000;
    /// 共享挂载
    pub const MS_SHARED: u64 = 0x20000;
    /// 从属挂载
    pub const MS_SLAVE: u64 = 0x80000;
    /// 不可绑定
    pub const MS_UNBINDABLE: u64 = 0x200000;
    /// 移动挂载点
    pub const MS_MOVE: u64 = 0x8000;
    /// 递归绑定
    pub const MS_REC: u64 = 0x4000;

    pub fn new(flags: u64) -> Self {
        Self(flags)
    }

    pub fn is_bind(&self) -> bool {
        (self.0 & Self::MS_BIND) != 0
    }

    pub fn is_move(&self) -> bool {
        (self.0 & Self::MS_MOVE) != 0
    }

    pub fn bits(&self) -> u64 {
        self.0
    }
}

/// 挂载点树遍历器
///
/// 用于遍历挂载点层次结构
pub struct MountTreeIter<'a> {
    /// 当前命名空间
    ns: &'a MntNamespace,
    /// 当前位置
    current: Option<SimpleArc<VfsMount>>,
}

impl<'a> MountTreeIter<'a> {
    /// 创建新遍历器
    pub fn new(ns: &'a MntNamespace) -> Self {
        Self {
            ns,
            current: None,
        }
    }
}

impl<'a> Iterator for MountTreeIter<'a> {
    type Item = SimpleArc<VfsMount>;

    fn next(&mut self) -> Option<Self::Item> {
        // TODO: 实现深度优先遍历
        // 简化实现：只返回根挂载点
        let current = self.current.take();
        current
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mnt_flags() {
        let flags = MntFlags::new(MntFlags::MNT_READONLY | MntFlags::MNT_NOEXEC);
        assert!(flags.is_readonly());
        assert!(flags.is_noexec());
        assert!(!flags.is_nosuid());
    }

    #[test]
    fn test_vfsmount_create() {
        let mountpoint = b"/mnt".to_vec();
        let root = b"/".to_vec();
        let flags = MntFlags::new(MntFlags::MNT_READONLY);

        let mnt = VfsMount::new(mountpoint, root, flags, None);
        assert!(mnt.mnt_flags.is_readonly());
        assert_eq!(mnt.mnt_id, 0);
    }

    #[test]
    fn test_namespace() {
        let ns = MntNamespace::new();
        assert!(ns.root.is_none());
        assert_eq!(ns.list_mounts().len(), 0);
    }

    #[test]
    fn test_ms_flags() {
        let flags = MsFlags::new(MsFlags::MS_BIND | MsFlags::MS_REC);
        assert!(flags.is_bind());
    }
}
