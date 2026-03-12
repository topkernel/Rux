//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! Path Resolution Module
//!
//!
//! Core concepts:
//! - Pathname resolution: Resolve pathname to dentry chain
//! - Absolute path: Path starting from root directory
//! - Relative path: Path starting from current directory
//! - Symbolic link resolution: Follow symbolic links

use crate::errno;

#[repr(C)]
pub struct NameiData<'a> {
    /// Current position
    pub path: Path<'a>,
    /// Last component
    pub last: Option<PathComponent<'a>>,
    /// Lookup flags
    pub flags: u32,
}

pub mod namei_flags {
    pub const LOOKUP_FOLLOW: u32 = 0x0001;  // Follow symbolic links
    pub const LOOKUP_DIRECTORY: u32 = 0x0002;  // Must be a directory
    pub const LOOKUP_AUTOMOUNT: u32 = 0x0004;  // Endpoint automount
    pub const LOOKUP_EMPTY: u32 = 0x0008;  // Empty path
    pub const LOOKUP_DOWN: u32 = 0x0010;  // Lookup descend
    pub const LOOKUP_MOUNTPOINT: u32 = 0x0020;  // Find mount point
    pub const LOOKUP_REVAL: u32 = 0x0040;  // Revalidate dentry
    pub const LOOKUP_RCU: u32 = 0x0080;  // RCU mode lookup
    pub const LOOKUP_NO_SYMLINKS: u32 = 0x0100;  // Don't follow symbolic links
    pub const LOOKUP_NO_RECURSE: u32 = 0x0200;  // Don't recurse
    pub const LOOKUP_PARENT: u32 = 0x0010;  // Only find parent directory
}

#[derive(Debug, Clone, Copy)]
pub struct PathComponent<'a> {
    /// Component name
    pub name: &'a str,
    /// Component length
    pub len: usize,
}

impl<'a> PathComponent<'a> {
    /// Create new path component
    pub fn new(name: &'a str) -> Self {
        Self {
            name,
            len: name.len(),
        }
    }

    /// Get name
    pub fn name(&self) -> &'a str {
        self.name
    }

    /// Check if current directory (.)
    pub fn is_current(&self) -> bool {
        self.name == "."
    }

    /// Check if parent directory (..)
    pub fn is_parent(&self) -> bool {
        self.name == ".."
    }

    /// Check if root directory
    pub fn is_root(&self) -> bool {
        self.name == "/"
    }

    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.name.is_empty()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Path<'a> {
    /// Path string
    pub path: &'a str,
}

impl<'a> Path<'a> {
    /// Create new path
    pub fn new(path: &'a str) -> Self {
        Self { path }
    }

    /// Check if absolute path
    pub fn is_absolute(&self) -> bool {
        self.path.starts_with('/')
    }

    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.path.is_empty()
    }

    /// Get path string
    pub fn as_str(&self) -> &'a str {
        self.path
    }

    /// Split path into components
    pub fn components(&self) -> PathComponents<'a> {
        PathComponents {
            path: self.path,
            pos: 0,
        }
    }

    /// Get parent directory path
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

    /// Get filename
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

    /// Append path
    pub fn join(&self, other: &str) -> Path<'a> {
        if self.path.ends_with('/') || other.starts_with('/') {
            Path::new(self.path)
        } else {
            Path::new(self.path)
        }
    }
}

pub struct PathComponents<'a> {
    /// Path string
    path: &'a str,
    /// Current position
    pos: usize,
}

impl<'a> Iterator for PathComponents<'a> {
    type Item = PathComponent<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        // Skip leading '/'
        while self.pos < self.path.len() && self.path.as_bytes()[self.pos] == b'/' {
            self.pos += 1;
        }

        // Check if reached end
        if self.pos >= self.path.len() {
            return None;
        }

        // Find next '/'
        let start = self.pos;
        while self.pos < self.path.len() && self.path.as_bytes()[self.pos] != b'/' {
            self.pos += 1;
        }

        Some(PathComponent::new(&self.path[start..self.pos]))
    }
}

pub fn filename_parentname(filename: &str, flags: u32) -> Result<NameiData<'_>, i32> {
    if filename.is_empty() {
        return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
    }

    // Create NameiData
    let nd = NameiData {
        path: Path::new(filename),
        last: None,
        flags,
    };

    // TODO: Implement complete path resolution
    // - Parse path components
    // - Find dentry
    // - Handle symbolic links
    // - Handle mount points

    Ok(nd)
}

pub fn path_normalize(path: &str) -> alloc::string::String {
    use alloc::vec::Vec;
    use alloc::string::String;

    if path.is_empty() {
        return String::new();
    }

    // Check if absolute path
    let is_absolute = path.starts_with('/');

    // Split path into components
    let components: Vec<&str> = path.split('/')
        .filter(|s| !s.is_empty() && *s != ".")
        .collect();

    // Handle .. and regular components
    let mut result: Vec<&str> = Vec::new();

    for component in components {
        if component == ".." {
            // Handle parent directory reference
            if is_absolute {
                // Absolute path: if at root, ignore ..
                if !result.is_empty() {
                    result.pop();
                }
            } else {
                // Relative path: handle .. normally
                if result.last() == Some(&"..") {
                    // If last one is also .., keep it
                    result.push("..");
                } else if !result.is_empty() {
                    result.pop();
                } else {
                    // Already at top level, add ..
                    result.push("..");
                }
            }
        } else {
            // Regular component
            result.push(component);
        }
    }

    // Rebuild path
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

    // Ensure root returns /
    if normalized.is_empty() && is_absolute {
        normalized.push('/');
    }

    normalized
}

pub fn path_lookup(filename: &str, _flags: u32) -> Result<Path<'_>, i32> {
    if filename.is_empty() {
        return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
    }

    // TODO: Implement path lookup
    // - Start from current directory or root directory
    // - Find path components one by one
    // - Return final found path

    Err(errno::Errno::FunctionNotImplemented.as_neg_i32())
}

pub fn follow_mount(_path: &mut Path) -> bool {
    // TODO: Implement mount point following
    false
}

pub fn follow_link(_path: &mut Path) -> Result<(), i32> {
    // TODO: Implement symbolic link following
    Err(errno::Errno::FunctionNotImplemented.as_neg_i32())
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
        // Basic absolute path
        assert_eq!(path_normalize("/usr/bin"), "/usr/bin");
        assert_eq!(path_normalize("/usr/bin/"), "/usr/bin");

        // Handle .
        assert_eq!(path_normalize("/usr/./bin"), "/usr/bin");
        assert_eq!(path_normalize("/./usr/bin"), "/usr/bin");

        // Handle ..
        assert_eq!(path_normalize("/usr/../bin"), "/bin");
        assert_eq!(path_normalize("/usr/local/../bin"), "/usr/bin");

        // Extra /
        assert_eq!(path_normalize("//usr///bin"), "/usr/bin");

        // Root directory
        assert_eq!(path_normalize("/"), "/");
        assert_eq!(path_normalize("//"), "/");
        assert_eq!(path_normalize("/.."), "/");
        assert_eq!(path_normalize("/../.."), "/");

        // Complex path
        assert_eq!(path_normalize("/a/b/../c/./d"), "/a/c/d");
    }

    #[test]
    fn test_path_normalize_relative() {
        // Basic relative path
        assert_eq!(path_normalize("usr/bin"), "usr/bin");
        assert_eq!(path_normalize("usr/bin/"), "usr/bin");

        // Handle .
        assert_eq!(path_normalize("usr/./bin"), "usr/bin");

        // Handle ..
        assert_eq!(path_normalize("usr/../bin"), "bin");
        assert_eq!(path_normalize("../usr/bin"), "../usr/bin");
        assert_eq!(path_normalize("usr/local/../../bin"), "../bin");

        // Empty
        assert_eq!(path_normalize(""), "");
    }

    #[test]
    fn test_path_normalize_edge_cases() {
        // Multiple consecutive ..
        assert_eq!(path_normalize("a/b/c/../../.."), "..");
        assert_eq!(path_normalize("/a/b/c/../../.."), "/");

        // Mix of . and ..
        assert_eq!(path_normalize("/a/./b/../c"), "/a/c");

        // Only .
        assert_eq!(path_normalize("."), "");
        assert_eq!(path_normalize("/."), "/");

        // Only ..
        assert_eq!(path_normalize(".."), "..");
        assert_eq!(path_normalize("/.."), "/");
    }
}
