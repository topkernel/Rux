//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

// Test: Path parsing functionality
use crate::println;
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

    test_println!("test: Path parsing testing completed.");
}
