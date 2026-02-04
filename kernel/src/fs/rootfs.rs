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

use crate::fs::superblock::{SuperBlock, SuperBlockFlags, FileSystemType, FsContext};
use crate::fs::mount::VfsMount;
use crate::collection::SimpleArc;
use alloc::vec::Vec;
use alloc::boxed::Box;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, AtomicPtr, Ordering};

/// RootFS 魔数
///
/// 对应 Linux 的 ROOTFS_MAGIC (include/linux/magic.h)
pub const ROOTFS_MAGIC: u32 = 0x73636673;  // "sfsf" - Simple File System

/// 全局 RootFS 超级块
///
/// 对应 Linux 的 init_rootfs() 创建的根文件系统
/// 使用 AtomicPtr 保护并发访问
static GLOBAL_ROOTFS_SB: AtomicPtr<RootFSSuperBlock> = AtomicPtr::new(core::ptr::null_mut());

/// 全局根挂载点
///
/// 对应 Linux 的根文件系统的挂载点
/// 使用 AtomicPtr 保护并发访问
static GLOBAL_ROOT_MOUNT: AtomicPtr<VfsMount> = AtomicPtr::new(core::ptr::null_mut());

/// 获取全局 RootFS 超级块
///
/// 返回 RootFS 超级块的指针
pub fn get_rootfs_sb() -> Option<*mut RootFSSuperBlock> {
    let ptr = GLOBAL_ROOTFS_SB.load(Ordering::Acquire);
    if ptr.is_null() {
        None
    } else {
        Some(ptr)
    }
}

/// 获取全局根挂载点
///
/// 返回根挂载点的指针
pub fn get_root_mount() -> Option<*mut VfsMount> {
    let ptr = GLOBAL_ROOT_MOUNT.load(Ordering::Acquire);
    if ptr.is_null() {
        None
    } else {
        Some(ptr)
    }
}

/// RootFS 文件节点类型
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum RootFSType {
    /// 目录
    Directory,
    /// 常规文件
    RegularFile,
}

/// RootFS 文件节点
///
/// 表示 RootFS 中的一个文件或目录
#[repr(C)]
pub struct RootFSNode {
    /// 节点名称
    pub name: Vec<u8>,
    /// 节点类型
    pub node_type: RootFSType,
    /// 节点数据（如果是文件）
    pub data: Option<Vec<u8>>,
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
        // TODO: SimpleArc 需要实现 Vec push
        // children.push(child);
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

/// RootFS 超级块
///
/// RootFS 文件系统的超级块
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
        // TODO: SimpleArc 需要实现 clone
        // Some(self.root_node.clone())
        None
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
            return Err(-17_i32);  // EEXIST: 不能创建根目录
        }

        let mut current = self.root_node.clone();

        // 遍历路径，找到父目录
        for i in 0..components.len() - 1 {
            let component = components[i].as_bytes();
            match current.find_child(component) {
                Some(child) => {
                    if !child.is_dir() {
                        return Err(-20_i32);  // ENOTDIR: 不是目录
                    }
                    current = child;
                }
                None => {
                    return Err(-2_i32);  // ENOENT: 父目录不存在
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
        if path == "/" || path.is_empty() {
            // TODO: SimpleArc 需要实现 clone
            return None;  // Some(self.root_node.clone());
        }

        let components: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        // TODO: SimpleArc 需要实现 clone 以便在循环中传递所有权
        // let mut current = self.root_node.clone();
        return None;  // 暂时返回 None
    }

    /// 列出目录内容
    pub fn list_dir(&self, path: &str) -> Result<Vec<SimpleArc<RootFSNode>>, i32> {
        let node = self.lookup(path).ok_or(-2_i32)?;  // ENOENT

        if !node.is_dir() {
            return Err(-20_i32);  // ENOTDIR
        }

        Ok(node.list_children())
    }
}

/// RootFS 挂载函数
///
/// 对应 Linux 的 rootfs_mount (fs/rootfs.c)
unsafe fn rootfs_mount(fc: &FsContext) -> Result<*mut SuperBlock, i32> {
    // 创建 RootFS 超级块
    let rootfs_sb = Box::new(RootFSSuperBlock::new());

    // 提取原始指针
    let sb_ptr = Box::into_raw(Box::new(rootfs_sb.sb)) as *mut SuperBlock;

    Ok(sb_ptr)
}

/// RootFS 文件系统类型定义
///
/// 对应 Linux 的 rootfs_fs_type (fs/rootfs.c)
pub static ROOTFS_FS_TYPE: FileSystemType = FileSystemType::new(
    "rootfs",
    Some(rootfs_mount),
    None,  // kill_sb - 使用默认实现
    0,     // fs_flags
);

/// 初始化 RootFS
///
/// 注册 rootfs 文件系统并挂载为根文件系统
pub fn init_rootfs() -> Result<(), i32> {
    use crate::fs::superblock::register_filesystem;
    use crate::fs::mount::MntFlags;

    // 注册 rootfs 文件系统
    register_filesystem(&ROOTFS_FS_TYPE)?;

    // 创建并初始化全局 RootFS 超级块
    let rootfs_sb = Box::new(RootFSSuperBlock::new());
    let rootfs_sb_ptr = Box::into_raw(rootfs_sb) as *mut RootFSSuperBlock;

    // 保存到全局变量（使用 AtomicPtr 保护）
    GLOBAL_ROOTFS_SB.store(rootfs_sb_ptr, Ordering::Release);

    // 创建根挂载点并泄漏到静态存储
    let mount = Box::new(VfsMount::new(
        b"/".to_vec(),      // 挂载点
        b"/".to_vec(),      // 根目录
        MntFlags::new(0),   // 无特殊标志
        Some(rootfs_sb_ptr as *mut u8),  // 超级块
    ));
    let mount_ptr = Box::into_raw(mount) as *mut VfsMount;

    // 保存到全局变量（使用 AtomicPtr 保护）
    GLOBAL_ROOT_MOUNT.store(mount_ptr, Ordering::Release);

    // 设置挂载点 ID 为 1（根挂载点）
    unsafe {
        (*mount_ptr).mnt_id = 1;
    }

    Ok(())
}

/// 获取根节点
///
/// 返回 RootFS 的根目录节点
pub fn get_root_node() -> Option<&'static RootFSNode> {
    let sb_ptr = GLOBAL_ROOTFS_SB.load(Ordering::Acquire);
    if sb_ptr.is_null() {
        return None;
    }
    unsafe {
        sb_ptr.as_ref().map(|sb| sb.root_node.as_ref())
    }
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
