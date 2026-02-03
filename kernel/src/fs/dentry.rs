//! 目录项 (Dentry) 管理
//!
//! 完全遵循 Linux 内核的 dentry 设计 (fs/dcache.c, include/linux/dcache.h)
//!
//! 核心概念：
//! - `struct dentry`: 目录项，表示目录中的一个条目
//! - `dcache`: 目录项缓存，加速路径查找

use crate::collection::SimpleArc;
use alloc::string::String;
use alloc::borrow::ToOwned;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::fs::inode::Inode;

/// Dentry 状态标志
///
/// 对应 Linux 的 d_flags (include/linux/dcache.h)
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct DentryFlags(u32);

impl DentryFlags {
    /// 目录项未连接到 dcache
    pub const DCACHE_UNHASHED: u32 = 0x00000001;
    /// 目录项已连接到 dcache
    pub const DCACHE_HASHED: u32 = 0x00000002;
    /// 目录项正在使用中
    pub const DCACHE_REFERENCED: u32 = 0x00000010;
    /// 目录项已删除
    pub const DCACHE_DENTRY_KILL: u32 = 0x00000040;

    pub fn new(flags: u32) -> Self {
        Self(flags)
    }

    pub fn is_hashed(&self) -> bool {
        (self.0 & Self::DCACHE_HASHED) != 0
    }

    pub fn is_unhashed(&self) -> bool {
        (self.0 & Self::DCACHE_UNHASHED) != 0
    }

    pub fn bits(&self) -> u32 {
        self.0
    }
}

/// Dentry 状态
///
/// 对应 Linux 的 d_state (include/linux/dcache.h)
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum DentryState {
    /// Dentry 未使用
    DUnhashed,
    /// Dentry 已使用
    DHashed,
    /// Dentry 正在删除
    DKill,
}

/// 目录项
///
/// 对应 Linux 的 struct dentry (include/linux/dcache.h)
#[repr(C)]
pub struct Dentry {
    /// dentry 名称
    pub name: Mutex<String>,
    /// 父目录项
    pub parent: Mutex<Option<SimpleArc<Dentry>>>,
    /// 关联的 inode
    pub inode: Mutex<Option<SimpleArc<Inode>>>,
    /// dentry 状态
    pub state: Mutex<DentryState>,
    /// dentry 标志
    pub flags: Mutex<DentryFlags>,
    /// 引用计数
    ref_count: AtomicU64,
}

unsafe impl Send for Dentry {}
unsafe impl Sync for Dentry {}

impl Dentry {
    /// 创建新的 dentry
    pub fn new(name: String) -> Self {
        Self {
            name: Mutex::new(name),
            parent: Mutex::new(None),
            inode: Mutex::new(None),
            state: Mutex::new(DentryState::DUnhashed),
            flags: Mutex::new(DentryFlags::new(DentryFlags::DCACHE_UNHASHED)),
            ref_count: AtomicU64::new(1),
        }
    }

    /// 设置父目录项
    pub fn set_parent(&self, parent: SimpleArc<Dentry>) {
        *self.parent.lock() = Some(parent);
    }

    /// 设置 inode
    pub fn set_inode(&self, inode: SimpleArc<Inode>) {
        *self.inode.lock() = Some(inode);
    }

    /// 获取 inode
    pub fn get_inode(&self) -> Option<SimpleArc<Inode>> {
        // SimpleArc 需要实现 Clone 才能返回
        // 暂时返回 None，需要修改 SimpleArc 实现
        None
    }

    /// 获取名称
    pub fn get_name(&self) -> String {
        self.name.lock().clone()
    }

    /// 设置为已哈希状态
    pub fn set_hashed(&self) {
        let mut flags = self.flags.lock();
        *flags = DentryFlags::new(flags.bits() | DentryFlags::DCACHE_HASHED);
        *self.state.lock() = DentryState::DHashed;
    }

    /// 设置为未哈希状态
    pub fn set_unhashed(&self) {
        let mut flags = self.flags.lock();
        *flags = DentryFlags::new(flags.bits() | DentryFlags::DCACHE_UNHASHED);
        *self.state.lock() = DentryState::DUnhashed;
    }

    /// 增加引用计数
    pub fn inc_ref(&self) {
        self.ref_count.fetch_add(1, Ordering::AcqRel);
    }

    /// 减少引用计数
    pub fn dec_ref(&self) -> u64 {
        self.ref_count.fetch_sub(1, Ordering::AcqRel) - 1
    }

    /// 获取引用计数
    pub fn get_ref(&self) -> u64 {
        self.ref_count.load(Ordering::Acquire)
    }
}

/// 创建根目录项
pub fn make_root_dentry() -> Option<SimpleArc<Dentry>> {
    let dentry = SimpleArc::new(Dentry::new("/".to_owned()))?;
    // Note: SimpleArc::as_ref returns &T, but we need to call a method
    // For now, we'll return the Arc directly - the caller can call set_hashed if needed
    Some(dentry)
}
