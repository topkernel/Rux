//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

// 测试：Path 路径解析功能
use crate::println;
use crate::fs::path::Path;
use super::{test_pass, test_fail, test_group_start};

pub fn test_path() {
    test_group_start("Path parsing");

    // 测试 1: 绝对路径检查
    if Path::new("/usr/bin").is_absolute()
        && Path::new("/").is_absolute()
        && !Path::new("relative/path").is_absolute() {
        test_pass("is_absolute");
    } else {
        test_fail("is_absolute", "absolute check failed");
    }

    // 测试 2: 空路径检查
    if Path::new("").is_empty()
        && !Path::new("/").is_empty()
        && !Path::new("path").is_empty() {
        test_pass("is_empty");
    } else {
        test_fail("is_empty", "empty check failed");
    }

    // 测试 3: 父目录获取
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

    // 测试 4: 文件名获取
    if Path::new("/usr/bin/bash").file_name() == Some("bash")
        && Path::new("/usr/bin/").file_name() == None
        && Path::new("/file.txt").file_name() == Some("file.txt")
        && Path::new("file.txt").file_name() == Some("file.txt")
        && Path::new("").file_name() == None {
        test_pass("file_name");
    } else {
        test_fail("file_name", "file_name check failed");
    }

    // 测试 5: as_str
    if Path::new("/usr/bin").as_str() == "/usr/bin"
        && Path::new("").as_str() == "" {
        test_pass("as_str");
    } else {
        test_fail("as_str", "as_str check failed");
    }

    println!("test: Path parsing testing completed.");
}
