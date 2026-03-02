//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! devfs - 设备文件系统
//!
//! 提供类似 Linux devfs 的功能：
//! - 挂载在 /dev
//! - 管理设备节点
//! - 支持字符设备和块设备

pub mod registry;

use alloc::string::String;
use alloc::vec::Vec;
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use spin::Mutex;
use crate::fs::file::FileOps;
use super::dev_t::DevNo;

// 重导出设备号定义
pub use super::dev_t;

// ============================================================================
// devfs 目录项
// ============================================================================

/// devfs 目录项类型
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DevEntryType {
    /// 目录
    Directory,
    /// 字符设备
    CharDevice,
    /// 块设备 (未实现)
    BlockDevice,
}

/// devfs 目录项
pub struct DevfsEntry {
    /// 名称
    pub name: String,
    /// 类型
    pub entry_type: DevEntryType,
    /// 子目录项 (仅目录类型有效)
    pub children: Mutex<BTreeMap<String, Arc<DevfsEntry>>>,
    /// 设备号 (仅设备类型有效)
    pub devno: DevNo,
    /// 权限 (默认 0666)
    pub mode: u32,
}

impl DevfsEntry {
    /// 创建目录
    pub fn new_dir(name: &str) -> Self {
        Self {
            name: String::from(name),
            entry_type: DevEntryType::Directory,
            children: Mutex::new(BTreeMap::new()),
            devno: DevNo::default(),
            mode: 0o755,
        }
    }

    /// 创建字符设备
    pub fn new_char_device(name: &str, devno: DevNo) -> Self {
        Self {
            name: String::from(name),
            entry_type: DevEntryType::CharDevice,
            children: Mutex::new(BTreeMap::new()),
            devno,
            mode: 0o666,
        }
    }

    /// 创建字符设备（带自定义权限）
    pub fn new_char_device_with_mode(name: &str, devno: DevNo, mode: u32) -> Self {
        Self {
            name: String::from(name),
            entry_type: DevEntryType::CharDevice,
            children: Mutex::new(BTreeMap::new()),
            devno,
            mode: mode & 0o777,
        }
    }

    /// 是否为目录
    pub fn is_dir(&self) -> bool {
        self.entry_type == DevEntryType::Directory
    }

    /// 是否为字符设备
    pub fn is_char_device(&self) -> bool {
        self.entry_type == DevEntryType::CharDevice
    }
}

// ============================================================================
// devfs 文件系统
// ============================================================================

/// devfs 全局实例
static DEVFS_ROOT: Mutex<Option<Arc<DevfsEntry>>> = Mutex::new(None);

/// 初始化 devfs
pub fn init() {
    let mut root = DEVFS_ROOT.lock();

    // 创建根目录
    let root_entry = Arc::new(DevfsEntry::new_dir("dev"));

    // 创建 /dev/input 目录
    let input_dir = Arc::new(DevfsEntry::new_dir("input"));

    // 添加 input 到根目录
    root_entry.children.lock().insert(String::from("input"), input_dir);

    *root = Some(root_entry);
}

/// 创建设备节点
///
/// # 参数
/// - path: 设备路径 (如 "/input/event0")
/// - devno: 设备号
/// - mode: 文件模式 (S_IFCHR 等)
///
/// # 返回
/// 成功返回 Ok(()), 失败返回 Err(())
pub fn mknod(path: &str, devno: DevNo, mode: u32) -> Result<(), ()> {
    // 去掉开头的 /
    let path = path.strip_prefix('/').unwrap_or(path);

    if path.is_empty() {
        return Err(());
    }

    let root = DEVFS_ROOT.lock();
    let root = match root.as_ref() {
        Some(r) => r,
        None => return Err(()),
    };

    // 解析路径
    let components: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if components.is_empty() {
        return Err(());
    }

    // 遍历到最后一个组件的父目录
    let mut current = root.clone();
    for i in 0..components.len() - 1 {
        let component = components[i];
        let children = current.children.lock();
        match children.get(component) {
            Some(child) => {
                let child = child.clone();
                drop(children);
                current = child;
            }
            None => return Err(()),
        }
    }

    // 创建设备节点
    let device_name = components.last().unwrap();
    let entry = Arc::new(DevfsEntry::new_char_device_with_mode(device_name, devno, mode));

    current.children.lock().insert(String::from(*device_name), entry);

    Ok(())
}

/// 创建目录
pub fn mkdir(path: &str) -> Result<(), ()> {
    // 去掉开头的 /
    let path = path.strip_prefix('/').unwrap_or(path);

    if path.is_empty() {
        return Err(());
    }

    let root = DEVFS_ROOT.lock();
    let root = match root.as_ref() {
        Some(r) => r,
        None => return Err(()),
    };

    // 解析路径
    let components: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if components.is_empty() {
        return Err(());
    }

    // 遍历到最后一个组件的父目录
    let mut current = root.clone();
    for i in 0..components.len() - 1 {
        let component = components[i];
        let children = current.children.lock();
        match children.get(component) {
            Some(child) => {
                let child = child.clone();
                drop(children);
                current = child;
            }
            None => return Err(()),
        }
    }

    // 创建目录
    let dir_name = components.last().unwrap();
    let entry = Arc::new(DevfsEntry::new_dir(dir_name));

    current.children.lock().insert(String::from(*dir_name), entry);

    Ok(())
}

/// 查找路径
///
/// # 返回
/// 找到返回 (entry, is_char_device, devno)
pub fn lookup(path: &str) -> Option<(Arc<DevfsEntry>, bool, DevNo)> {
    // 去掉开头的 /
    let path = path.strip_prefix('/').unwrap_or(path);

    // 空路径或 "." 表示根目录
    if path.is_empty() || path == "." {
        // 返回根目录
        let root = DEVFS_ROOT.lock();
        let root = root.as_ref()?;
        return Some((root.clone(), false, DevNo::default()));
    }

    let root = DEVFS_ROOT.lock();
    let root = root.as_ref()?;

    // 解析路径，过滤掉 "." 和 ".."
    let components: Vec<&str> = path.split('/').filter(|s| !s.is_empty() && *s != "." && *s != "..").collect();

    // 如果过滤后为空，返回根目录
    if components.is_empty() {
        return Some((root.clone(), false, DevNo::default()));
    }

    // 遍历路径
    let mut current = root.clone();
    for component in &components {
        let children = current.children.lock();
        match children.get(*component) {
            Some(child) => {
                let child = child.clone();
                drop(children);
                current = child;
            }
            None => return None,
        }
    }

    Some((
        current.clone(),
        current.is_char_device(),
        current.devno,
    ))
}

/// 检查 devfs 是否已初始化
pub fn is_mounted() -> bool {
    DEVFS_ROOT.lock().is_some()
}

/// 目录项信息 (name, is_dir, ino)
pub type DevfsDirEntry = (String, bool, u64);

/// 列出目录内容
///
/// # 参数
/// - path: devfs 内部路径 (如 "" 表示根目录, "input" 表示 /dev/input)
///
/// # 返回
/// 成功返回目录项列表，失败返回 None
pub fn list_dir(path: &str) -> Option<Vec<DevfsDirEntry>> {
    let root = DEVFS_ROOT.lock();
    let root = root.as_ref()?;

    // 空路径或 "." 表示根目录
    if path.is_empty() || path == "/" || path == "." {
        let children = root.children.lock();
        let mut entries = Vec::new();
        let mut ino = 1u64;
        for (name, entry) in children.iter() {
            entries.push((name.clone(), entry.is_dir(), ino));
            ino += 1;
        }
        return Some(entries);
    }

    // 解析路径，过滤掉 "." 和 ".."
    let components: Vec<&str> = path.split('/').filter(|s| !s.is_empty() && *s != "." && *s != "..").collect();

    // 遍历到目标目录
    let mut current = root.clone();
    for component in &components {
        let children = current.children.lock();
        match children.get(*component) {
            Some(child) => {
                let child = child.clone();
                drop(children);
                current = child;
            }
            None => return None,
        }
    }

    // 检查是否是目录
    if !current.is_dir() {
        return None;
    }

    // 列出子项
    let children = current.children.lock();
    let mut entries = Vec::new();
    let mut ino = 1u64;
    for (name, entry) in children.iter() {
        entries.push((name.clone(), entry.is_dir(), ino));
        ino += 1;
    }
    Some(entries)
}

/// 获取设备路径（检查是否在 /dev 下）
///
/// 如果路径以 /dev 开头，返回 devfs 路径（去掉 /dev 前缀）
pub fn parse_dev_path(path: &str) -> Option<&str> {
    if path == "/dev" {
        return Some("");
    }
    if path.starts_with("/dev/") {
        return Some(&path[5..]);
    }
    None
}
