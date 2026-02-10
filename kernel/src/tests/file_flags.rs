//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

// 测试：FileFlags 文件标志
use crate::println;
use crate::fs::file::FileFlags;

pub fn test_file_flags() {
    println!("test: Testing FileFlags...");

    // 测试 1: 基本访问模式
    println!("test: 1. Testing access modes...");
    let rdonly = FileFlags::O_RDONLY;
    let wronly = FileFlags::O_WRONLY;
    let rdwr = FileFlags::O_RDWR;

    assert_eq!(rdonly & FileFlags::O_ACCMODE, FileFlags::O_RDONLY, "O_RDONLY should match");
    assert_eq!(wronly & FileFlags::O_ACCMODE, FileFlags::O_WRONLY, "O_WRONLY should match");
    assert_eq!(rdwr & FileFlags::O_ACCMODE, FileFlags::O_RDWR, "O_RDWR should match");
    println!("test:    SUCCESS - access modes work");

    // 测试 2: 标志位组合
    println!("test: 2. Testing flag combinations...");
    let creat = FileFlags::O_CREAT;
    let trunc = FileFlags::O_TRUNC;
    let excl = FileFlags::O_EXCL;

    let flags = rdwr | creat | trunc;
    assert_eq!(flags & FileFlags::O_ACCMODE, FileFlags::O_RDWR, "Should preserve access mode");
    assert_eq!(flags & FileFlags::O_CREAT, FileFlags::O_CREAT, "Should include O_CREAT");
    assert_eq!(flags & FileFlags::O_TRUNC, FileFlags::O_TRUNC, "Should include O_TRUNC");
    assert_eq!(flags & FileFlags::O_EXCL, 0, "Should not include O_EXCL");
    println!("test:    SUCCESS - flag combinations work");

    // 测试 3: 标志位检查
    println!("test: 3. Testing flag presence checks...");
    let flags = FileFlags::O_RDWR | FileFlags::O_CREAT | FileFlags::O_APPEND;

    // 检查 O_RDWR
    if (flags & FileFlags::O_ACCMODE) == FileFlags::O_RDWR {
        // OK
    } else {
        panic!("O_RDWR flag check failed");
    }

    // 检查 O_CREAT
    if (flags & FileFlags::O_CREAT) != 0 {
        // OK
    } else {
        panic!("O_CREAT flag check failed");
    }

    // 检查 O_APPEND
    if (flags & FileFlags::O_APPEND) != 0 {
        // OK
    } else {
        panic!("O_APPEND flag check failed");
    }

    // 检查 O_TRUNC（不应该存在）
    if (flags & FileFlags::O_TRUNC) != 0 {
        panic!("O_TRUNC should not be set");
    }

    println!("test:    SUCCESS - flag presence checks work");

    println!("test: FileFlags testing completed.");
}
