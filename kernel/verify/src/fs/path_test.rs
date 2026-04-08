//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Path normalization and resolution invariant tests.
//!
//! Types copied from: kernel/src/fs/path.rs

use proptest::prelude::*;

// ============================================================================
// Copied types from kernel/src/fs/path.rs
// ============================================================================

#[derive(Debug, Clone, Copy)]
pub struct PathComponent<'a> {
    pub name: &'a str,
    pub len: usize,
}

impl<'a> PathComponent<'a> {
    pub fn new(name: &'a str) -> Self {
        Self { name, len: name.len() }
    }

    pub fn name(&self) -> &'a str {
        self.name
    }

    pub fn is_current(&self) -> bool {
        self.name == "."
    }

    pub fn is_parent(&self) -> bool {
        self.name == ".."
    }

    pub fn is_root(&self) -> bool {
        self.name == "/"
    }

    pub fn is_empty(&self) -> bool {
        self.name.is_empty()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Path<'a> {
    pub path: &'a str,
}

impl<'a> Path<'a> {
    pub fn new(path: &'a str) -> Self {
        Self { path }
    }

    pub fn is_absolute(&self) -> bool {
        self.path.starts_with('/')
    }

    pub fn is_empty(&self) -> bool {
        self.path.is_empty()
    }

    pub fn as_str(&self) -> &'a str {
        self.path
    }

    pub fn components(&self) -> PathComponents<'a> {
        PathComponents { path: self.path, pos: 0 }
    }

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
}

pub struct PathComponents<'a> {
    path: &'a str,
    pos: usize,
}

impl<'a> Iterator for PathComponents<'a> {
    type Item = PathComponent<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        while self.pos < self.path.len() && self.path.as_bytes()[self.pos] == b'/' {
            self.pos += 1;
        }
        if self.pos >= self.path.len() {
            return None;
        }
        let start = self.pos;
        while self.pos < self.path.len() && self.path.as_bytes()[self.pos] != b'/' {
            self.pos += 1;
        }
        Some(PathComponent::new(&self.path[start..self.pos]))
    }
}

pub fn path_normalize(path: &str) -> String {
    if path.is_empty() {
        return String::new();
    }

    let is_absolute = path.starts_with('/');

    let components: Vec<&str> = path.split('/')
        .filter(|s| !s.is_empty() && *s != ".")
        .collect();

    let mut result: Vec<&str> = Vec::new();

    for component in components {
        if component == ".." {
            if is_absolute {
                if !result.is_empty() {
                    result.pop();
                }
            } else {
                if result.last() == Some(&"..") {
                    result.push("..");
                } else if !result.is_empty() {
                    result.pop();
                } else {
                    result.push("..");
                }
            }
        } else {
            result.push(component);
        }
    }

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

    if normalized.is_empty() && is_absolute {
        normalized.push('/');
    }

    normalized
}

// ============================================================================
// Tests
// ============================================================================

proptest! {
    /// INV-PATH-1: Absolute paths stay absolute after normalization
    #[test]
    fn test_absolute_stays_absolute(
        parts in proptest::collection::vec("[a-z]{1,5}", 1..6),
    ) {
        let path = format!("/{}", parts.join("/"));
        let norm = path_normalize(&path);
        prop_assert!(norm.starts_with('/'));
    }

    /// INV-PATH-2: No "." in normalized output (absolute)
    #[test]
    fn test_no_dot_absolute(
        parts in proptest::collection::vec("[a-z./]{1,5}", 1..8),
    ) {
        let path = format!("/{}", parts.join("/"));
        let norm = path_normalize(&path);
        // No "." component should appear
        for comp in norm.split('/') {
            prop_assert_ne!(comp, ".");
        }
    }

    /// INV-PATH-3: No consecutive "//" in normalized output
    #[test]
    fn test_no_double_slash(
        parts in proptest::collection::vec("[a-z]{1,5}", 1..6),
    ) {
        let path = format!("///{}///", parts.join("///"));
        let norm = path_normalize(&path);
        prop_assert!(!norm.contains("//"));
    }

    /// INV-PATH-4: /.. normalizes to / (root escape prevention)
    #[test]
    fn test_root_escape(
        count in 1usize..10usize,
    ) {
        let dots: Vec<&str> = (0..count).map(|_| "..").collect();
        let path = format!("/{}", dots.join("/"));
        let norm = path_normalize(&path);
        prop_assert_eq!(norm, "/");
    }

    /// INV-PATH-5: is_absolute correct
    #[test]
    fn test_is_absolute(path in "[a-z/]{0,20}") {
        let p = Path::new(&path);
        prop_assert_eq!(p.is_absolute(), path.starts_with('/'));
    }

    /// INV-PATH-6: components splits correctly
    #[test]
    fn test_components(
        parts in proptest::collection::vec("[a-z]{1,5}", 1..6),
    ) {
        let path = format!("/{}", parts.join("/"));
        let comps: Vec<&str> = Path::new(&path).components().map(|c| c.name()).collect();
        prop_assert_eq!(comps, parts);
    }

    /// INV-PATH-7: parent/file_name consistency
    #[test]
    fn test_parent_filename(
        parts in proptest::collection::vec("[a-z]{1,5}", 2..6),
    ) {
        let path = format!("/{}", parts.join("/"));
        let p = Path::new(&path);
        prop_assert_eq!(p.file_name(), Some(parts.last().unwrap().as_str()));
        let parent = p.parent().unwrap();
        prop_assert!(parent.is_absolute());
    }

    /// INV-PATH-8: a/b/.. normalizes to a
    #[test]
    fn test_dotdot_cancel(name1 in "[a-z]{1,5}", name2 in "[a-z]{1,5}") {
        let path = format!("/{}//{}/..", name1, name2);
        let norm = path_normalize(&path);
        prop_assert_eq!(norm, format!("/{}", name1));
    }

    /// INV-PATH-9: Relative path with .. accumulates
    #[test]
    fn test_relative_dotdot(
        ups in 1usize..5usize,
    ) {
        let dots: Vec<&str> = (0..ups).map(|_| "..").collect();
        let path = dots.join("/");
        let norm = path_normalize(&path);
        // With no components to pop, all .. accumulate
        let expected: String = (0..ups).map(|_| "..").collect::<Vec<&str>>().join("/");
        prop_assert_eq!(norm, expected);
    }

    /// INV-PATH-10: PathComponent classification
    #[test]
    fn test_path_component(name in "[a-z.]{1,5}") {
        let c = PathComponent::new(&name);
        prop_assert_eq!(c.is_current(), name == ".");
        prop_assert_eq!(c.is_parent(), name == "..");
    }
}

#[test]
/// INV-PATH-11: Empty string normalizes to empty
fn test_normalize_empty() {
    assert_eq!(path_normalize(""), "");
}

#[test]
/// INV-PATH-12: Root normalizes to root
fn test_normalize_root() {
    assert_eq!(path_normalize("/"), "/");
    assert_eq!(path_normalize("//"), "/");
}

#[test]
/// INV-PATH-13: parent of root returns root
fn test_parent_of_root() {
    assert_eq!(Path::new("/").parent().unwrap().as_str(), "/");
}

#[test]
/// INV-PATH-14: known-answer complex path
fn test_normalize_complex() {
    assert_eq!(path_normalize("/a/b/../c/./d"), "/a/c/d");
    assert_eq!(path_normalize("/a/./b/../c"), "/a/c");
    assert_eq!(path_normalize("/a/b/c/../../.."), "/");
}
