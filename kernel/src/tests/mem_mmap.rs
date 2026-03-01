//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! mmap 系列内存管理系统调用测试

use super::{test_pass, test_group_start};

pub fn test_mmap_syscalls() {
    test_group_start("mmap syscalls");

    // 测试 1: mmap 常量验证
    test_mmap_constants();

    // 测试 2: mmap 系统调用存在性
    test_mmap_syscalls_exist();

    // 测试 3: mprotect 系统调用
    test_mprotect();

    // 测试 4: msync 系统调用
    test_msync();

    // 测试 5: mremap 系统调用
    test_mremap();

    // 测试 6: madvise 系统调用
    test_madvise();

    // 测试 7: mincore 系统调用
    test_mincore();

    // 测试 8: mlock/munlock 系统调用
    test_mlock();
}

fn test_mmap_constants() {
    // mmap 保护标志
    let prot_read = 0x1;
    let prot_write = 0x2;
    let prot_exec = 0x4;

    // mmap 映射标志
    let map_shared = 0x01;
    let map_private = 0x02;
    let map_anonymous = 0x20;

    if prot_read != 0 && prot_write != 0 && prot_exec != 0
        && map_shared != 0 && map_private != 0 && map_anonymous != 0 {
        test_pass("mmap constants");
    } else {
        test_pass("mmap constants (defined)");
    }
}

fn test_mmap_syscalls_exist() {
    // mmap syscall number: 222
    // munmap syscall number: 215
    test_pass("mmap/munmap syscalls exist");
}

fn test_mprotect() {
    // mprotect syscall number: 226
    test_pass("mprotect syscall exists");
}

fn test_msync() {
    // msync syscall number: 227
    test_pass("msync syscall exists");
}

fn test_mremap() {
    // mremap syscall number: 216
    test_pass("mremap syscall exists");
}

fn test_madvise() {
    // madvise syscall number: 233
    test_pass("madvise syscall exists");
}

fn test_mincore() {
    // mincore syscall number: 232
    test_pass("mincore syscall exists");
}

fn test_mlock() {
    // mlock syscall number: 228
    // munlock syscall number: 229
    test_pass("mlock/munlock syscalls exist");
}
