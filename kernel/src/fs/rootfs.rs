//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! RootFS - 基于内存的简单文件系统
//!
//! 对应 Linux 的 rootfs (fs/rootfs.c)
//!
//! RootFS 是一个简单的、基于内存的文件系统，
//! 用于内核启动时的初始根文件系统。
//!
//! 特性：
//! - 基于 RAM 的文件存储
//! - 支持目录和常规文件
//! - 不支持块设备
//! - 不需要磁盘

use crate::errno;
use crate::fs::superblock::{SuperBlock, SuperBlockFlags, FileSystemType, FsContext};
use crate::fs::mount::VfsMount;
use crate::fs::path::path_normalize;
use crate::collection::SimpleArc;
use alloc::vec::Vec;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::borrow::ToOwned;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, AtomicPtr, Ordering};

pub const ROOTFS_MAGIC: u32 = 0x73636673;  // "sfsf" - Simple File System

static GLOBAL_ROOTFS_SB: AtomicPtr<RootFSSuperBlock> = AtomicPtr::new(core::ptr::null_mut());

static GLOBAL_ROOT_MOUNT: AtomicPtr<VfsMount> = AtomicPtr::new(core::ptr::null_mut());

// ============================================================================
// RootFS 路径缓存 (Path Cache)
// ============================================================================

const ROOTFS_PATH_CACHE_SIZE: usize = 256;

struct RootFSPathCacheEntry {
    /// 完整路径
    path: String,
    /// 节点引用
    node: Option<SimpleArc<RootFSNode>>,
}

impl RootFSPathCacheEntry {
    fn new() -> Self {
        Self {
            path: String::new(),
            node: None,
        }
    }
}

struct RootFSPathCache {
    /// 哈希表桶
    buckets: [RootFSPathCacheEntry; ROOTFS_PATH_CACHE_SIZE],
    /// 缓存命中计数
    hits: AtomicU64,
    /// 缓存未命中计数
    misses: AtomicU64,
}

unsafe impl Send for RootFSPathCache {}
unsafe impl Sync for RootFSPathCache {}

static ROOTFS_PATH_CACHE: Mutex<Option<RootFSPathCache>> = Mutex::new(None);

fn rootfs_path_cache_init() {
    let mut cache = ROOTFS_PATH_CACHE.lock();
    if cache.is_some() {
        return;  // 已经初始化
    }

    let buckets: [RootFSPathCacheEntry; ROOTFS_PATH_CACHE_SIZE] =
        core::array::from_fn(|_| RootFSPathCacheEntry::new());

    *cache = Some(RootFSPathCache {
        buckets,
        hits: AtomicU64::new(0),
        misses: AtomicU64::new(0),
    });
}

fn rootfs_path_hash(path: &str) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;  // FNV offset basis
    for byte in path.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn rootfs_path_cache_lookup(path: &str) -> Option<SimpleArc<RootFSNode>> {
    rootfs_path_cache_init();

    let cache = ROOTFS_PATH_CACHE.lock();
    let cache_inner = cache.as_ref()?;

    let hash = rootfs_path_hash(path);
    let index = (hash as usize) % ROOTFS_PATH_CACHE_SIZE;

    let bucket = &cache_inner.buckets[index];
    if bucket.path == path {
        if let Some(ref node) = bucket.node {
            cache_inner.hits.fetch_add(1, Ordering::Relaxed);
            return Some(node.clone());
        }
    }

    cache_inner.misses.fetch_add(1, Ordering::Relaxed);
    None
}

fn rootfs_path_cache_add(path: &str, node: SimpleArc<RootFSNode>) {
    rootfs_path_cache_init();

    let mut cache = ROOTFS_PATH_CACHE.lock();
    let inner = cache.as_mut().expect("cache not initialized");

    let hash = rootfs_path_hash(path);
    let index = (hash as usize) % ROOTFS_PATH_CACHE_SIZE;

    // 简单的 LRU：直接覆盖旧条目
    inner.buckets[index].path = path.to_owned();
    inner.buckets[index].node = Some(node);
}

fn rootfs_path_cache_stats() -> (u64, u64) {
    rootfs_path_cache_init();

    let cache = ROOTFS_PATH_CACHE.lock();
    let cache_inner = cache.as_ref().expect("cache not initialized");

    (
        cache_inner.hits.load(Ordering::Relaxed),
        cache_inner.misses.load(Ordering::Relaxed),
    )
}

pub fn get_rootfs_sb() -> Option<*mut RootFSSuperBlock> {
    let ptr = GLOBAL_ROOTFS_SB.load(Ordering::Acquire);
    if ptr.is_null() {
        None
    } else {
        Some(ptr)
    }
}

pub fn get_root_mount() -> Option<*mut VfsMount> {
    let ptr = GLOBAL_ROOT_MOUNT.load(Ordering::Acquire);
    if ptr.is_null() {
        None
    } else {
        Some(ptr)
    }
}

#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum RootFSType {
    /// 目录
    Directory,
    /// 常规文件
    RegularFile,
    /// 符号链接
    SymbolicLink,
}

const MAX_SYMLINKS: usize = 40;

#[repr(C)]
pub struct RootFSNode {
    /// 节点名称
    pub name: Vec<u8>,
    /// 节点类型
    pub node_type: RootFSType,
    /// 节点数据（如果是文件）
    pub data: Option<Vec<u8>>,
    /// 符号链接目标（如果是符号链接）
    pub link_target: Option<Vec<u8>>,
    /// 子节点（如果是目录）
    pub children: Mutex<Vec<SimpleArc<RootFSNode>>>,
    /// 引用计数
    ref_count: AtomicU64,
    /// 节点 ID
    pub ino: u64,
}

unsafe impl Send for RootFSNode {}
unsafe impl Sync for RootFSNode {}

impl RootFSNode {
    /// 创建新节点
    pub fn new(name: Vec<u8>, node_type: RootFSType, ino: u64) -> Self {
        Self {
            name,
            node_type,
            data: None,
            link_target: None,
            children: Mutex::new(Vec::new()),
            ref_count: AtomicU64::new(1),
            ino,
        }
    }

    /// 创建目录节点
    pub fn new_dir(name: Vec<u8>, ino: u64) -> Self {
        Self::new(name, RootFSType::Directory, ino)
    }

    /// 创建文件节点
    pub fn new_file(name: Vec<u8>, data: Vec<u8>, ino: u64) -> Self {
        let mut node = Self::new(name, RootFSType::RegularFile, ino);
        node.data = Some(data);
        node
    }

    /// 创建符号链接节点
    pub fn new_symlink(name: Vec<u8>, target: Vec<u8>, ino: u64) -> Self {
        let mut node = Self::new(name, RootFSType::SymbolicLink, ino);
        node.link_target = Some(target);
        node
    }

    /// 增加引用计数
    pub fn get(&self) {
        self.ref_count.fetch_add(1, Ordering::AcqRel);
    }

    /// 减少引用计数
    pub fn put(&self) {
        if self.ref_count.fetch_sub(1, Ordering::AcqRel) == 1 {
            // 最后一个引用
        }
    }

    /// 添加子节点
    pub fn add_child(&self, child: SimpleArc<RootFSNode>) {
        let mut children = self.children.lock();
        children.push(child);
    }

    /// 移除子节点
    pub fn remove_child(&self, name: &[u8]) -> bool {
        let mut children = self.children.lock();
        if let Some(pos) = children.iter().position(|c| c.as_ref().name == name) {
            children.remove(pos);
            true
        } else {
            false
        }
    }

    /// 重命名子节点
    pub fn rename_child(&self, old_name: &[u8], new_name: Vec<u8>) -> Result<(), ()> {
        let children = self.children.lock();
        let pos = children.iter().position(|c| c.as_ref().name == old_name).ok_or(())?;

        // 由于 SimpleArc 不提供内部可变性，我们需要使用 unsafe
        // 这在文件系统中是安全的，因为我们持有父目录的锁
        unsafe {
            let node_ptr = children[pos].as_ptr();
            (*node_ptr).name = new_name;
        }

        Ok(())
    }

    /// 查找子节点
    pub fn find_child(&self, name: &[u8]) -> Option<SimpleArc<RootFSNode>> {
        let children = self.children.lock();
        for child in children.iter() {
            if child.as_ref().name == name {
                // SimpleArc 已实现 Clone trait
                return Some(child.clone());
            }
        }
        None
    }

    /// 获取所有子节点
    pub fn list_children(&self) -> Vec<SimpleArc<RootFSNode>> {
        let children = self.children.lock();
        // 克隆每个 SimpleArc 引用
        children.iter().map(|child| child.clone()).collect()
    }

    /// 检查是否是目录
    pub fn is_dir(&self) -> bool {
        self.node_type == RootFSType::Directory
    }

    /// 检查是否是文件
    pub fn is_file(&self) -> bool {
        self.node_type == RootFSType::RegularFile
    }

    /// 检查是否是符号链接
    pub fn is_symlink(&self) -> bool {
        self.node_type == RootFSType::SymbolicLink
    }

    /// 获取符号链接目标
    pub fn get_link_target(&self) -> Option<Vec<u8>> {
        self.link_target.clone()
    }

    /// 读取文件数据
    pub fn read_data(&self, offset: usize, buf: &mut [u8]) -> usize {
        if let Some(ref data) = self.data {
            if offset >= data.len() {
                return 0;
            }
            let remaining = &data[offset..];
            let to_copy = core::cmp::min(remaining.len(), buf.len());
            buf[..to_copy].copy_from_slice(&remaining[..to_copy]);
            to_copy
        } else {
            0
        }
    }

    /// 写入文件数据
    pub fn write_data(&mut self, offset: usize, data: &[u8]) -> usize {
        if self.data.is_none() {
            self.data = Some(Vec::new());
        }

        if let Some(ref mut existing_data) = self.data {
            // 确保向量足够大
            let required_size = offset + data.len();
            if existing_data.len() < required_size {
                existing_data.resize(required_size, 0);
            }

            // 从 offset 位置开始写入数据
            existing_data[offset..offset + data.len()].copy_from_slice(data);
            data.len()
        } else {
            0
        }
    }
}

pub struct RootFSSuperBlock {
    /// 基础超级块
    pub sb: SuperBlock,
    /// 根节点
    pub root_node: SimpleArc<RootFSNode>,
    /// 下一个 inode ID
    next_ino: AtomicU64,
}

impl RootFSSuperBlock {
    /// 创建新的 RootFS 超级块
    pub fn new() -> Self {
        // 创建根目录节点
        let root_node = SimpleArc::new(RootFSNode::new_dir(b"/".to_vec(), 1)).expect("Failed to create root node");

        // 创建超级块
        let mut sb = SuperBlock::new(4096, ROOTFS_MAGIC);
        sb.set_flags(SuperBlockFlags::new(SuperBlockFlags::SB_ACTIVE));

        Self {
            sb,
            root_node,
            next_ino: AtomicU64::new(2),
        }
    }

    /// 获取根节点
    pub fn get_root(&self) -> Option<SimpleArc<RootFSNode>> {
        // SimpleArc 已经实现了 Clone trait (collection.rs)
        Some(self.root_node.clone())
    }

    /// 分配新的 inode ID
    pub fn alloc_ino(&self) -> u64 {
        self.next_ino.fetch_add(1, Ordering::AcqRel)
    }

    /// 在指定路径创建文件
    pub fn create_file(&self, path: &str, data: Vec<u8>) -> Result<(), i32> {
        // 解析路径
        let components: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

        if components.is_empty() {
            return Err(errno::Errno::FileExists.as_neg_i32());
        }

        let mut current = self.root_node.clone();

        // 遍历路径，找到父目录
        for i in 0..components.len() - 1 {
            let component = components[i].as_bytes();
            match current.find_child(component) {
                Some(child) => {
                    if !child.is_dir() {
                        return Err(errno::Errno::NotADirectory.as_neg_i32());
                    }
                    current = child;
                }
                None => {
                    return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
                }
            }
        }

        // 创建新文件
        let filename = components.last().unwrap().as_bytes().to_vec();
        let ino = self.alloc_ino();
        let new_file = SimpleArc::new(RootFSNode::new_file(filename, data, ino)).expect("Failed to create file");
        current.add_child(new_file);

        Ok(())
    }

    /// 查找文件
    pub fn lookup(&self, path: &str) -> Option<SimpleArc<RootFSNode>> {
        // 处理空路径
        if path.is_empty() {
            return Some(self.root_node.clone());
        }

        // 检查是否是相对路径
        let is_relative = !path.starts_with('/');

        // 规范化路径（处理 . 和 ..）
        let normalized = path_normalize(path);

        // 如果是相对路径，暂时不支持（需要当前工作目录）
        if is_relative && !normalized.is_empty() && !normalized.starts_with("..") {
            // TODO: 支持相对路径（需要当前工作目录）
            // 对于简单的相对路径如 "usr/bin"，可以尝试从根目录查找
            // 但正确的行为应该是从进程的当前工作目录开始
            return None;
        }

        // 如果规范化后为空，返回根目录
        let normalized_path = if normalized.is_empty() {
            "/"
        } else {
            normalized.as_str()
        };

        // 尝试从路径缓存查找
        if let Some(cached) = rootfs_path_cache_lookup(normalized_path) {
            return Some(cached);
        }

        // 缓存未命中，执行路径遍历（支持符号链接）
        let result = self.lookup_follow(normalized_path, 0);

        // 将结果添加到缓存
        if let Some(ref node) = result {
            rootfs_path_cache_add(normalized_path, node.clone());
        }

        result
    }

    /// 实际执行路径遍历的内部函数（不支持符号链接）
    fn lookup_walk(&self, path: &str) -> Option<SimpleArc<RootFSNode>> {
        if path == "/" {
            return Some(self.root_node.clone());
        }

        let components: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

        if components.is_empty() {
            return Some(self.root_node.clone());
        }

        // 从根节点开始遍历路径
        let mut current = self.root_node.clone();

        for (i, component) in components.iter().enumerate() {
            let component_bytes = component.as_bytes();

            // 从树中查找子节点
            match current.find_child(component_bytes) {
                Some(child) => {
                    if !child.is_dir() && i < components.len() - 1 {
                        // 不是目录，但路径还没结束
                        return None;
                    }

                    current = child;
                }
                None => {
                    // 查找失败
                    return None;
                }
            }
        }

        Some(current)
    }

    /// 列出目录内容
    pub fn list_dir(&self, path: &str) -> Result<Vec<SimpleArc<RootFSNode>>, i32> {
        let node = self.lookup(path).ok_or(errno::Errno::NoSuchFileOrDirectory.as_neg_i32())?;

        if !node.is_dir() {
            return Err(errno::Errno::NotADirectory.as_neg_i32());
        }

        Ok(node.list_children())
    }

    /// 创建目录
    ///
    /// 对应 Linux 的 vfs_mkdir() (fs/namei.c)
    pub fn mkdir(&self, path: &str) -> Result<(), i32> {
        // 规范化路径
        let normalized = path_normalize(path);

        // 分割路径
        let components: Vec<&str> = normalized.split('/')
            .filter(|s| !s.is_empty())
            .collect();

        if components.is_empty() {
            return Err(errno::Errno::FileExists.as_neg_i32());
        }

        let mut current = self.root_node.clone();

        // 遍历路径，找到父目录
        for i in 0..components.len() - 1 {
            let component = components[i].as_bytes();
            match current.find_child(component) {
                Some(child) => {
                    if !child.is_dir() {
                        return Err(errno::Errno::NotADirectory.as_neg_i32());
                    }
                    current = child;
                }
                None => {
                    return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
                }
            }
        }

        // 创建新目录
        let dirname = components.last().unwrap().as_bytes().to_vec();
        let ino = self.alloc_ino();
        let new_dir = SimpleArc::new(RootFSNode::new_dir(dirname, ino))
            .ok_or(errno::Errno::OutOfMemory.as_neg_i32())?;

        current.add_child(new_dir);

        Ok(())
    }

    /// 删除文件
    ///
    /// 对应 Linux 的 vfs_unlink() (fs/namei.c)
    pub fn unlink(&self, path: &str) -> Result<(), i32> {
        // 规范化路径
        let normalized = path_normalize(path);

        // 分割路径
        let components: Vec<&str> = normalized.split('/')
            .filter(|s| !s.is_empty())
            .collect();

        if components.is_empty() {
            return Err(errno::Errno::IsADirectory.as_neg_i32());
        }

        let mut current = self.root_node.clone();

        // 遍历路径，找到父目录
        for i in 0..components.len() - 1 {
            let component = components[i].as_bytes();
            match current.find_child(component) {
                Some(child) => {
                    if !child.is_dir() {
                        return Err(errno::Errno::NotADirectory.as_neg_i32());
                    }
                    current = child;
                }
                None => {
                    return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
                }
            }
        }

        // 删除文件
        let filename = components.last().unwrap().as_bytes();

        // 检查是否存在
        let target = current.find_child(filename).ok_or(errno::Errno::NoSuchFileOrDirectory.as_neg_i32())?;

        // 不能删除目录
        if target.is_dir() {
            return Err(errno::Errno::IsADirectory.as_neg_i32());
        }

        // 删除文件
        if !current.remove_child(filename) {
            return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
        }

        Ok(())
    }

    /// 删除目录
    ///
    /// 对应 Linux 的 vfs_rmdir() (fs/namei.c)
    pub fn rmdir(&self, path: &str) -> Result<(), i32> {
        // 规范化路径
        let normalized = path_normalize(path);

        // 分割路径
        let components: Vec<&str> = normalized.split('/')
            .filter(|s| !s.is_empty())
            .collect();

        if components.is_empty() {
            return Err(errno::Errno::IsADirectory.as_neg_i32());
        }

        let mut current = self.root_node.clone();

        // 遍历路径，找到父目录
        for i in 0..components.len() - 1 {
            let component = components[i].as_bytes();
            match current.find_child(component) {
                Some(child) => {
                    if !child.is_dir() {
                        return Err(errno::Errno::NotADirectory.as_neg_i32());
                    }
                    current = child;
                }
                None => {
                    return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
                }
            }
        }

        // 删除目录
        let dirname = components.last().unwrap().as_bytes();

        // 检查是否存在
        let target = current.find_child(dirname).ok_or(errno::Errno::NoSuchFileOrDirectory.as_neg_i32())?;

        // 必须是目录
        if !target.is_dir() {
            return Err(errno::Errno::NotADirectory.as_neg_i32());
        }

        // 目录必须为空
        if !target.list_children().is_empty() {
            return Err(errno::Errno::DirectoryNotEmpty.as_neg_i32());
        }

        // 删除目录
        if !current.remove_child(dirname) {
            return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
        }

        Ok(())
    }

    /// 重命名文件或目录
    ///
    /// 对应 Linux 的 vfs_rename() (fs/namei.c)
    pub fn rename(&self, oldpath: &str, newpath: &str) -> Result<(), i32> {
        // 规范化路径
        let old_normalized = path_normalize(oldpath);
        let new_normalized = path_normalize(newpath);

        // 分割旧路径
        let old_components: Vec<&str> = old_normalized.split('/')
            .filter(|s| !s.is_empty())
            .collect();

        if old_components.is_empty() {
            return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
        }

        // 找到旧文件的父目录
        let mut old_parent = self.root_node.clone();

        for i in 0..old_components.len() - 1 {
            let component = old_components[i].as_bytes();
            match old_parent.find_child(component) {
                Some(child) => {
                    if !child.is_dir() {
                        return Err(errno::Errno::NotADirectory.as_neg_i32());
                    }
                    old_parent = child;
                }
                None => {
                    return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
                }
            }
        }

        let old_name = old_components.last().unwrap().as_bytes();

        // 检查旧文件是否存在
        let _target = old_parent.find_child(old_name).ok_or(errno::Errno::NoSuchFileOrDirectory.as_neg_i32())?;

        // 分割新路径
        let new_components: Vec<&str> = new_normalized.split('/')
            .filter(|s| !s.is_empty())
            .collect();

        if new_components.is_empty() {
            return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
        }

        // 找到新文件的父目录
        let mut new_parent = self.root_node.clone();

        for i in 0..new_components.len() - 1 {
            let component = new_components[i].as_bytes();
            match new_parent.find_child(component) {
                Some(child) => {
                    if !child.is_dir() {
                        return Err(errno::Errno::NotADirectory.as_neg_i32());
                    }
                    new_parent = child;
                }
                None => {
                    return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
                }
            }
        }

        let new_name = new_components.last().unwrap().as_bytes().to_vec();

        // 检查新文件是否已存在
        if new_parent.find_child(&new_name).is_some() {
            // 如果目标存在，需要先删除
            new_parent.remove_child(&new_name);
        }

        // 从旧父目录中移除
        if !old_parent.remove_child(old_name) {
            return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
        }

        // 由于我们需要修改节点的名称，而 SimpleArc 不提供内部可变性
        // 我们需要重新创建节点
        // 这是一个简化实现，Linux 中有更复杂的处理

        // 暂时返回错误，因为需要重新创建节点
        // TODO: 实现完整的 rename 逻辑
        Err(errno::Errno::FunctionNotImplemented.as_neg_i32())
    }

    /// 创建符号链接
    ///
    /// 对应 Linux 的 vfs_symlink() (fs/namei.c)
    pub fn symlink(&self, target: &str, linkpath: &str) -> Result<(), i32> {
        // 规范化链接路径
        let link_normalized = path_normalize(linkpath);

        // 分割路径
        let components: Vec<&str> = link_normalized.split('/')
            .filter(|s| !s.is_empty())
            .collect();

        if components.is_empty() {
            return Err(errno::Errno::FileExists.as_neg_i32());
        }

        let mut current = self.root_node.clone();

        // 遍历路径，找到父目录
        for i in 0..components.len() - 1 {
            let component = components[i].as_bytes();
            match current.find_child(component) {
                Some(child) => {
                    if !child.is_dir() {
                        return Err(errno::Errno::NotADirectory.as_neg_i32());
                    }
                    current = child;
                }
                None => {
                    return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
                }
            }
        }

        // 创建新符号链接
        let linkname = components.last().unwrap().as_bytes().to_vec();
        let target_bytes = target.as_bytes().to_vec();
        let ino = self.alloc_ino();
        let new_symlink = SimpleArc::new(RootFSNode::new_symlink(linkname, target_bytes, ino))
            .ok_or(errno::Errno::OutOfMemory.as_neg_i32())?;

        current.add_child(new_symlink);

        Ok(())
    }

    /// 读取符号链接目标
    ///
    /// 对应 Linux 的 vfs_readlink() (fs/read_write.c)
    pub fn readlink(&self, path: &str) -> Result<Vec<u8>, i32> {
        // 查找符号链接节点
        let node = self.lookup(path).ok_or(errno::Errno::NoSuchFileOrDirectory.as_neg_i32())?;

        // 检查是否是符号链接
        if !node.is_symlink() {
            return Err(errno::Errno::InvalidArgument.as_neg_i32());
        }

        // 获取目标路径
        node.get_link_target().ok_or(errno::Errno::NoSuchFileOrDirectory.as_neg_i32())
    }

    /// 跟随符号链接（内部实现）
    ///
    /// 对应 Linux 的 follow_link() (fs/namei.c)
    ///
    /// # 参数
    /// - `link`: 符号链接节点
    /// - `depth`: 当前递归深度
    ///
    /// # 返回
    /// 成功返回符号链接指向的实际节点，失败返回错误
    fn follow_link_internal(
        &self,
        link: &SimpleArc<RootFSNode>,
        depth: usize,
    ) -> Option<SimpleArc<RootFSNode>> {
        // 检查递归深度
        if depth >= MAX_SYMLINKS {
            return None;  // ELOOP: 符号链接层级过深
        }

        // 获取目标路径
        let target_bytes = link.get_link_target()?;
        let target = core::str::from_utf8(&target_bytes).ok()?;

        // 规范化目标路径
        let normalized = path_normalize(target);

        // 查找目标节点（递归查找）
        self.lookup_follow(&normalized, depth + 1)
    }

    /// 查找路径，支持跟随符号链接（内部实现）
    ///
    /// # 参数
    /// - `path`: 规范化后的路径
    /// - `depth`: 当前递归深度
    fn lookup_follow(&self, path: &str, depth: usize) -> Option<SimpleArc<RootFSNode>> {
        if path == "/" {
            return Some(self.root_node.clone());
        }

        let components: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

        if components.is_empty() {
            return Some(self.root_node.clone());
        }

        // 从根节点开始遍历路径
        let mut current = self.root_node.clone();

        for (i, component) in components.iter().enumerate() {
            let component_bytes = component.as_bytes();

            // 从树中查找子节点
            match current.find_child(component_bytes) {
                Some(child) => {
                    // 如果是符号链接，跟随它
                    if child.is_symlink() && i < components.len() - 1 {
                        // 跟随符号链接
                        let target = self.follow_link_internal(&child, depth)?;
                        // 继续从符号链接目标查找
                        current = target;
                    } else {
                        current = child;
                    }

                    // 检查是否需要继续遍历
                    if !current.is_dir() && i < components.len() - 1 {
                        return None;
                    }
                }
                None => {
                    return None;
                }
            }
        }

        Some(current)
    }
}

unsafe extern "C" fn rootfs_mount(_fc: &FsContext) -> Result<*mut SuperBlock, i32> {
    // 创建 RootFS 超级块
    let rootfs_sb = Box::new(RootFSSuperBlock::new());

    // 提取原始指针
    let sb_ptr = Box::into_raw(Box::new(rootfs_sb.sb)) as *mut SuperBlock;

    Ok(sb_ptr)
}

pub static ROOTFS_FS_TYPE: FileSystemType = FileSystemType::new(
    "rootfs",
    Some(rootfs_mount),
    None,  // kill_sb - 使用默认实现
    0,     // fs_flags
);

pub fn init_rootfs() -> Result<(), i32> {
    use crate::fs::superblock::register_filesystem;
    use crate::fs::mount::MntFlags;
    use crate::println;

    println!("init_rootfs: Step 1 - register_filesystem");

    // 注册 rootfs 文件系统
    register_filesystem(&ROOTFS_FS_TYPE)?;

    println!("init_rootfs: Step 2 - create RootFSSuperBlock");

    // 创建并初始化全局 RootFS 超级块
    let rootfs_sb = Box::new(RootFSSuperBlock::new());
    let rootfs_sb_ptr = Box::into_raw(rootfs_sb) as *mut RootFSSuperBlock;

    println!("init_rootfs: Step 3 - store rootfs_sb_ptr = {:#x}", rootfs_sb_ptr as usize);

    // 保存到全局变量（使用 AtomicPtr 保护）
    GLOBAL_ROOTFS_SB.store(rootfs_sb_ptr, Ordering::Release);

    println!("init_rootfs: Step 4 - create VfsMount");

    // 创建根挂载点并泄漏到静态存储
    let mount = Box::new(VfsMount::new(
        b"/".to_vec(),      // 挂载点
        b"/".to_vec(),      // 根目录
        MntFlags::new(0),   // 无特殊标志
        Some(rootfs_sb_ptr as *mut u8),  // 超级块
    ));
    let mount_ptr = Box::into_raw(mount) as *mut VfsMount;

    println!("init_rootfs: Step 5 - store mount_ptr = {:#x}", mount_ptr as usize);

    // 保存到全局变量（使用 AtomicPtr 保护）
    GLOBAL_ROOT_MOUNT.store(mount_ptr, Ordering::Release);

    println!("init_rootfs: Step 6 - set mnt_id");

    // 设置挂载点 ID 为 1（根挂载点）
    unsafe {
        (*mount_ptr).mnt_id = 1;
    }

    println!("init_rootfs: [OK]");

    Ok(())
}

pub fn get_root_node() -> Option<&'static RootFSNode> {
    let sb_ptr = GLOBAL_ROOTFS_SB.load(Ordering::Acquire);
    if sb_ptr.is_null() {
        return None;
    }
    unsafe {
        sb_ptr.as_ref().map(|sb| sb.root_node.as_ref())
    }
}

pub fn get_rootfs() -> *const RootFSSuperBlock {
    GLOBAL_ROOTFS_SB.load(Ordering::Acquire)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rootfs_node() {
        let dir = RootFSNode::new_dir(b"test".to_vec(), 1);
        assert!(dir.is_dir());
        assert!(!dir.is_file());

        let file = RootFSNode::new_file(b"file.txt".to_vec(), b"hello".to_vec(), 2);
        assert!(file.is_file());
        assert!(!file.is_dir());
    }

    #[test]
    fn test_rootfs_superblock() {
        let sb = RootFSSuperBlock::new();
        let root = sb.get_root();
        assert!(root.is_dir());
    }

    #[test]
    fn test_rootfs_create_file() {
        let sb = RootFSSuperBlock::new();

        // 创建文件
        assert!(sb.create_file("/test.txt", b"hello".to_vec()).is_ok());

        // 查找文件
        let file = sb.lookup("/test.txt");
        assert!(file.is_some());
        assert!(file.unwrap().is_file());
    }

    #[test]
    fn test_rootfs_nested_path() {
        let sb = RootFSSuperBlock::new();

        // 创建嵌套目录和文件
        assert!(sb.create_file("/dir1/dir2/file.txt", b"data".to_vec()).is_err()); // 父目录不存在
    }

    #[test]
    fn test_rootfs_list() {
        let sb = RootFSSuperBlock::new();

        // 创建多个文件
        assert!(sb.create_file("/file1.txt", b"data1".to_vec()).is_ok());
        assert!(sb.create_file("/file2.txt", b"data2".to_vec()).is_ok());

        // 列出根目录
        let children = sb.list_dir("/").unwrap();
        assert_eq!(children.len(), 2);  // file1.txt 和 file2.txt
    }
}
