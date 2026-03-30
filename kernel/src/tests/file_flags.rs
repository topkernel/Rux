//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

// Test: FileFlags file flags
use crate::println;
use crate::fs::file::FileFlags;
use super::{test_pass, test_fail, test_group_start};

pub fn test_file_flags() {
    test_group_start("FileFlags");

    // Test 1: Basic access modes
    let rdonly = FileFlags::O_RDONLY;
    let wronly = FileFlags::O_WRONLY;
    let rdwr = FileFlags::O_RDWR;

    if (rdonly & FileFlags::O_ACCMODE) == FileFlags::O_RDONLY
        && (wronly & FileFlags::O_ACCMODE) == FileFlags::O_WRONLY
        && (rdwr & FileFlags::O_ACCMODE) == FileFlags::O_RDWR {
        test_pass("access modes");
    } else {
        test_fail("access modes", "mode check failed");
    }

    // Test 2: Flag combinations
    let creat = FileFlags::O_CREAT;
    let trunc = FileFlags::O_TRUNC;

    let flags = rdwr | creat | trunc;
    if (flags & FileFlags::O_ACCMODE) == FileFlags::O_RDWR
        && (flags & FileFlags::O_CREAT) == FileFlags::O_CREAT
        && (flags & FileFlags::O_TRUNC) == FileFlags::O_TRUNC
        && (flags & FileFlags::O_EXCL) == 0 {
        test_pass("flag combinations");
    } else {
        test_fail("flag combinations", "combination check failed");
    }

    // Test 3: Flag presence checks
    let flags = FileFlags::O_RDWR | FileFlags::O_CREAT | FileFlags::O_APPEND;

    if (flags & FileFlags::O_ACCMODE) == FileFlags::O_RDWR
        && (flags & FileFlags::O_CREAT) != 0
        && (flags & FileFlags::O_APPEND) != 0
        && (flags & FileFlags::O_TRUNC) == 0 {
        test_pass("flag presence checks");
    } else {
        test_fail("flag presence checks", "presence check failed");
    }

    test_println!("test: FileFlags testing completed.");
}
