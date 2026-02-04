//! 路径解析模块
//!
//! 完全遵循 Linux 内核的路径解析设计 (fs/namei.c)
//!
//! 核心概念：
//! - 路径名解析：将路径名解析为 dentry 链
//! - 绝对路径：从根目录开始的路径
//! - 相对路径：从当前目录开始的路径
//! - 符号链接解析：跟随符号链接

use alloc::string::String;
use alloc::vec::Vec;

/// 路径查找上下文
///
/// 对应 Linux 的 struct nameidata (include/linux/namei.h)
#[repr(C)]
pub struct NameiData<'a> {
    /// 当前位置
    pub path: Path<'a>,
    /// 最后一个组件
    pub last: Option<PathComponent<'a>>,
    /// 查找标志
    pub flags: u32,
}

/// 路径查找标志
///
/// 对应 Linux 的 LOOKUP_* 宏 (include/linux/namei.h)
pub mod namei_flags {
    pub const LOOKUP_FOLLOW: u32 = 0x0001;  // 跟随符号链接
    pub const LOOKUP_DIRECTORY: u32 = 0x0002;  // 必须是目录
    pub const LOOKUP_AUTOMOUNT: u32 = 0x0004;  // 终点自动挂载
    pub const LOOKUP_EMPTY: u32 = 0x0008;  // 空路径
    pub const LOOKUP_DOWN: u32 = 0x0010;  // 查找下降
    pub const LOOKUP_MOUNTPOINT: u32 = 0x0020;  // 查找挂载点
    pub const LOOKUP_REVAL: u32 = 0x0040;  // 重新验证 dentry
    pub const LOOKUP_RCU: u32 = 0x0080;  // RCU 模式查找
    pub const LOOKUP_NO_SYMLINKS: u32 = 0x0100;  // 不跟随符号链接
    pub const LOOKUP_NO_RECURSE: u32 = 0x0200;  // 不递归
    pub const LOOKUP_PARENT: u32 = 0x0010;  // 只查找父目录
}

/// 路径组件
///
/// 表示路径中的一个组件
#[derive(Debug, Clone, Copy)]
pub struct PathComponent<'a> {
    /// 组件名称
    pub name: &'a str,
    /// 组件长度
    pub len: usize,
}

impl<'a> PathComponent<'a> {
    /// 创建新的路径组件
    pub fn new(name: &'a str) -> Self {
        Self {
            name,
            len: name.len(),
        }
    }

    /// 获取名称
    pub fn name(&self) -> &'a str {
        self.name
    }

    /// 检查是否是当前目录 (.)
    pub fn is_current(&self) -> bool {
        self.name == "."
    }

    /// 检查是否是父目录 (..)
    pub fn is_parent(&self) -> bool {
        self.name == ".."
    }

    /// 检查是否是根目录
    pub fn is_root(&self) -> bool {
        self.name == "/"
    }

    /// 检查是否为空
    pub fn is_empty(&self) -> bool {
        self.name.is_empty()
    }
}

/// 路径
///
/// 表示一个文件系统路径
#[derive(Debug, Clone, Copy)]
pub struct Path<'a> {
    /// 路径字符串
    pub path: &'a str,
}

impl<'a> Path<'a> {
    /// 创建新路径
    pub fn new(path: &'a str) -> Self {
        Self { path }
    }

    /// 检查是否是绝对路径
    pub fn is_absolute(&self) -> bool {
        self.path.starts_with('/')
    }

    /// 检查是否为空
    pub fn is_empty(&self) -> bool {
        self.path.is_empty()
    }

    /// 获取路径字符串
    pub fn as_str(&self) -> &'a str {
        self.path
    }

    /// 分割路径为组件
    pub fn components(&self) -> PathComponents<'a> {
        PathComponents {
            path: self.path,
            pos: 0,
        }
    }

    /// 获取父目录路径
    pub fn parent(&self) -> Option<Path<'a>> {
        if let Some(idx) = self.path.rfind('/') {
            if idx == 0 {
                Some(Path::new("/"))
            } else {
                Some(Path::new(&self.path[..idx]))
            }
        } else {
            None
        }
    }

    /// 获取文件名
    pub fn file_name(&self) -> Option<&'a str> {
        if let Some(idx) = self.path.rfind('/') {
            if idx + 1 < self.path.len() {
                Some(&self.path[idx + 1..])
            } else {
                None
            }
        } else if !self.path.is_empty() {
            Some(self.path)
        } else {
            None
        }
    }

    /// 追加路径
    pub fn join(&self, other: &str) -> Path<'a> {
        if self.path.ends_with('/') || other.starts_with('/') {
            Path::new(self.path)
        } else {
            Path::new(&self.path)
        }
    }
}

/// 路径组件迭代器
///
/// 用于遍历路径的各个组件
pub struct PathComponents<'a> {
    /// 路径字符串
    path: &'a str,
    /// 当前位置
    pos: usize,
}

impl<'a> Iterator for PathComponents<'a> {
    type Item = PathComponent<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        // 跳过开头的 '/'
        while self.pos < self.path.len() && self.path.as_bytes()[self.pos] == b'/' {
            self.pos += 1;
        }

        // 检查是否到达末尾
        if self.pos >= self.path.len() {
            return None;
        }

        // 查找下一个 '/'
        let start = self.pos;
        while self.pos < self.path.len() && self.path.as_bytes()[self.pos] != b'/' {
            self.pos += 1;
        }

        Some(PathComponent::new(&self.path[start..self.pos]))
    }
}

/// 路径解析函数
///
/// 将路径名解析为路径查找上下文
///
/// # 参数
/// - `filename`: 要解析的路径名
/// - `flags`: 查找标志
///
/// # 返回
/// 成功返回路径查找上下文，失败返回错误码
pub fn filename_parentname(filename: &str, flags: u32) -> Result<NameiData<'_>, i32> {
    if filename.is_empty() {
        return Err(-2_i32);  // ENOENT
    }

    // 创建 NameiData
    let nd = NameiData {
        path: Path::new(filename),
        last: None,
        flags,
    };

    // TODO: 实现完整的路径解析
    // - 解析路径组件
    // - 查找 dentry
    // - 处理符号链接
    // - 处理挂载点

    Ok(nd)
}

/// 路径规范化
///
/// 对应 Linux 的 path_init() (fs/namei.c)
///
/// 规范化路径：
/// - 移除多余的 `/`
/// - 处理 `.` (当前目录)
/// - 处理 `..` (父目录)
/// - 移除尾部的 `/`（除了根目录）
///
/// # 参数
/// - `path`: 要规范化的路径
///
/// # 返回
/// 规范化后的路径字符串
pub fn path_normalize(path: &str) -> alloc::string::String {
    use alloc::vec::Vec;
    use alloc::string::String;

    if path.is_empty() {
        return String::new();
    }

    // 判断是否是绝对路径
    let is_absolute = path.starts_with('/');

    // 分割路径为组件
    let components: Vec<&str> = path.split('/')
        .filter(|s| !s.is_empty() && *s != ".")
        .collect();

    // 处理 .. 和普通组件
    let mut result: Vec<&str> = Vec::new();

    for component in components {
        if component == ".." {
            // 处理父目录引用
            if is_absolute {
                // 绝对路径：如果在根目录，忽略 ..
                if !result.is_empty() {
                    result.pop();
                }
            } else {
                // 相对路径：正常处理 ..
                if result.last() == Some(&"..") {
                    // 如果最后一个也是 ..，保留
                    result.push("..");
                } else if !result.is_empty() {
                    result.pop();
                } else {
                    // 已经到达顶层，添加 ..
                    result.push("..");
                }
            }
        } else {
            // 普通组件
            result.push(component);
        }
    }

    // 重建路径
    let mut normalized = if is_absolute {
        String::from("/")
    } else {
        String::new()
    };

    for (i, component) in result.iter().enumerate() {
        if i > 0 || !is_absolute {
            if i > 0 {
                normalized.push('/');
            }
            normalized.push_str(component);
        } else if is_absolute && !component.is_empty() {
            normalized.push_str(component);
        }
    }

    // 确保根目录返回 /
    if normalized.is_empty() && is_absolute {
        normalized.push('/');
    }

    normalized
}

/// 路径查找辅助函数
///
/// 对应 Linux 的 path_lookup (fs/namei.c)
pub fn path_lookup(filename: &str, flags: u32) -> Result<Path, i32> {
    if filename.is_empty() {
        return Err(-2_i32);  // ENOENT
    }

    // TODO: 实现路径查找
    // - 从当前目录或根目录开始
    // - 逐个查找路径组件
    // - 返回最终找到的路径

    Err(-38_i32)  // ENOSYS: 暂时未实现
}

/// 检查路径是否在挂载点
///
/// 对应 Linux 的 __follow_mount (fs/namei.c)
pub fn follow_mount(path: &mut Path) -> bool {
    // TODO: 实现挂载点跟随
    false
}

/// 检查并跟随符号链接
///
/// 对应 Linux名的 follow_link (fs/namei.c)
pub fn follow_link(path: &mut Path) -> Result<(), i32> {
    // TODO: 实现符号链接跟随
    Err(-38_i32)  // ENOSYS: 暂时未实现
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_path_is_absolute() {
        assert!(Path::new("/").is_absolute());
        assert!(Path::new("/usr/bin").is_absolute());
        assert!(!Path::new("usr/bin").is_absolute());
    }

    #[test]
    fn test_path_components() {
        let path = Path::new("/usr/bin/bash");
        let components: Vec<_> = path.components().map(|c| c.name()).collect();
        assert_eq!(components, vec!["usr", "bin", "bash"]);
    }

    #[test]
    fn test_path_parent() {
        assert_eq!(Path::new("/usr/bin/bash").parent().unwrap().as_str(), "/usr/bin");
        assert_eq!(Path::new("/usr").parent().unwrap().as_str(), "/");
        assert!(Path::new("/").parent().is_none());
    }

    #[test]
    fn test_path_file_name() {
        assert_eq!(Path::new("/usr/bin/bash").file_name(), Some("bash"));
        assert_eq!(Path::new("/usr/bin/").file_name(), None);
        assert_eq!(Path::new("/").file_name(), None);
    }

    #[test]
    fn test_path_component_checks() {
        assert!(PathComponent::new(".").is_current());
        assert!(PathComponent::new("..").is_parent());
        assert!(PathComponent::new("/").is_root());
        assert!(!PathComponent::new("test").is_current());
        assert!(!PathComponent::new("test").is_parent());
    }

    #[test]
    fn test_path_normalize_absolute() {
        // 基本绝对路径
        assert_eq!(path_normalize("/usr/bin"), "/usr/bin");
        assert_eq!(path_normalize("/usr/bin/"), "/usr/bin");

        // 处理 .
        assert_eq!(path_normalize("/usr/./bin"), "/usr/bin");
        assert_eq!(path_normalize("/./usr/bin"), "/usr/bin");

        // 处理 ..
        assert_eq!(path_normalize("/usr/../bin"), "/bin");
        assert_eq!(path_normalize("/usr/local/../bin"), "/usr/bin");

        // 多余的 /
        assert_eq!(path_normalize("//usr///bin"), "/usr/bin");

        // 根目录
        assert_eq!(path_normalize("/"), "/");
        assert_eq!(path_normalize("//"), "/");
        assert_eq!(path_normalize("/.."), "/");
        assert_eq!(path_normalize("/../.."), "/");

        // 复杂路径
        assert_eq!(path_normalize("/a/b/../c/./d"), "/a/c/d");
    }

    #[test]
    fn test_path_normalize_relative() {
        // 基本相对路径
        assert_eq!(path_normalize("usr/bin"), "usr/bin");
        assert_eq!(path_normalize("usr/bin/"), "usr/bin");

        // 处理 .
        assert_eq!(path_normalize("usr/./bin"), "usr/bin");

        // 处理 ..
        assert_eq!(path_normalize("usr/../bin"), "bin");
        assert_eq!(path_normalize("../usr/bin"), "../usr/bin");
        assert_eq!(path_normalize("usr/local/../../bin"), "../bin");

        // 空
        assert_eq!(path_normalize(""), "");
    }

    #[test]
    fn test_path_normalize_edge_cases() {
        // 多个连续的 ..
        assert_eq!(path_normalize("a/b/c/../../.."), "..");
        assert_eq!(path_normalize("/a/b/c/../../.."), "/");

        // . 和 .. 混合
        assert_eq!(path_normalize("/a/./b/../c"), "/a/c");

        // 只有 .
        assert_eq!(path_normalize("."), "");
        assert_eq!(path_normalize("/."), "/");

        // 只有 ..
        assert_eq!(path_normalize(".."), "..");
        assert_eq!(path_normalize("/.."), "/");
    }
}
