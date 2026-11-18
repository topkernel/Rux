//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

// Kernel layer policy (docs/architecture/kernel-layers.md):
// this module must stay Safe Rust.
#![forbid(unsafe_code)]

// Test: Path parsing functionality
use crate::println;
use alloc::vec::Vec;
use crate::fs::path::Path;
use super::{test_pass, test_fail, test_group_start};

pub fn test_path() {
    test_group_start("Path parsing");

    // Test 1: Absolute path check
    if Path::new("/usr/bin").is_absolute()
        && Path::new("/").is_absolute()
        && !Path::new("relative/path").is_absolute() {
        test_pass("is_absolute");
    } else {
        test_fail("is_absolute", "absolute check failed");
    }

    // Test 2: Empty path check
    if Path::new("").is_empty()
        && !Path::new("/").is_empty()
        && !Path::new("path").is_empty() {
        test_pass("is_empty");
    } else {
        test_fail("is_empty", "empty check failed");
    }

    // Test 3: Parent directory retrieval
    let parent1 = Path::new("/usr/bin").parent();
    let parent2 = Path::new("/usr").parent();
    let parent3 = Path::new("/").parent();
    let parent4 = Path::new("file.txt").parent();

    if parent1.is_some() && parent1.unwrap().as_str() == "/usr"
        && parent2.is_some() && parent2.unwrap().as_str() == "/"
        && parent3.is_some() && parent3.unwrap().as_str() == "/"
        && parent4.is_none() {
        test_pass("parent");
    } else {
        test_fail("parent", "parent check failed");
    }

    // Test 4: File name retrieval
    if Path::new("/usr/bin/bash").file_name() == Some("bash")
        && Path::new("/usr/bin/").file_name() == None
        && Path::new("/file.txt").file_name() == Some("file.txt")
        && Path::new("file.txt").file_name() == Some("file.txt")
        && Path::new("").file_name() == None {
        test_pass("file_name");
    } else {
        test_fail("file_name", "file_name check failed");
    }

    // Test 5: as_str
    if Path::new("/usr/bin").as_str() == "/usr/bin"
        && Path::new("").as_str() == "" {
        test_pass("as_str");
    } else {
        test_fail("as_str", "as_str check failed");
    }

    // Ported from the dead #[cfg(test)] block in kernel/src/fs/path.rs.
    // Components iteration.
    let components: Vec<&str> = Path::new("/usr/bin/bash")
        .components()
        .map(|c| c.name())
        .collect();
    if components == ["usr", "bin", "bash"] {
        test_pass("components");
    } else {
        test_fail("components", "component iteration drifted");
    }

    // Component classification.
    use crate::fs::path::PathComponent;
    if PathComponent::new(".").is_current()
        && PathComponent::new("..").is_parent()
        && PathComponent::new("/").is_root()
        && !PathComponent::new("test").is_current()
        && !PathComponent::new("test").is_parent()
    {
        test_pass("component checks");
    } else {
        test_fail("component checks", "is_current/is_parent/is_root drifted");
    }

    // path_normalize pins (absolute + relative + edge cases).
    let cases: &[(&str, &str)] = &[
        ("/usr/bin", "/usr/bin"),
        ("/usr/bin/", "/usr/bin"),
        ("/usr/./bin", "/usr/bin"),
        ("/./usr/bin", "/usr/bin"),
        ("/usr/../bin", "/bin"),
        ("/usr/local/../bin", "/usr/bin"),
        ("//usr///bin", "/usr/bin"),
        ("/", "/"),
        ("//", "/"),
        ("/..", "/"),
        ("/../..", "/"),
        ("/a/b/../c/./d", "/a/c/d"),
        ("usr/bin", "usr/bin"),
        ("usr/bin/", "usr/bin"),
        ("usr/./bin", "usr/bin"),
        ("usr/../bin", "bin"),
        ("../usr/bin", "../usr/bin"),
        // (the dead block pinned "../bin" here — wrong: usr/local/../..
        // returns to the start directory, so the path is "bin"; this pin
        // was never executed until the port).
        ("usr/local/../../bin", "bin"),
        ("", ""),
        // DEVIATION FLAG (reference-behavior audit pending): the current
        // implementation returns "" for the exact-cancellation relative
        // case; directory-lookup semantics (each .. resolves the parent,
        // like the reference's path walk) would give ".." — 3 downs and
        // 3 ups land one above the start. Pinned AS-IS to prevent silent
        // drift; changing it is a kernel-behavior decision, not a test
        // decision (path_normalize feeds every path operation).
        ("a/b/c/../../..", ""),
        ("/a/b/c/../../..", "/"),
        ("/a/./b/../c", "/a/c"),
        (".", ""),
        ("/.", "/"),
        ("..", ".."),
        ("/..", "/"),
    ];
    let mut all_ok = true;
    for (input, want) in cases {
        if crate::fs::path::path_normalize(input) != *want {
            all_ok = false;
        }
    }
    if all_ok {
        test_pass("path_normalize pins");
    } else {
        test_fail("path_normalize pins", "normalization drifted from the pinned cases");
    }

    test_println!("test: Path parsing testing completed.");
}
