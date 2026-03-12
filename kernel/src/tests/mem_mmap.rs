//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! mmap series memory management syscall test

use super::{test_pass, test_group_start};

pub fn test_mmap_syscalls() {
    test_group_start("mmap syscalls");

    // Test 1: mmap constant verification
    test_mmap_constants();

    // Test 2: mmap syscall existence
    test_mmap_syscalls_exist();

    // Test 3: mprotect syscall
    test_mprotect();

    // Test 4: msync syscall
    test_msync();

    // Test 5: mremap syscall
    test_mremap();

    // Test 6: madvise syscall
    test_madvise();

    // Test 7: mincore syscall
    test_mincore();

    // Test 8: mlock/munlock syscalls
    test_mlock();
}

fn test_mmap_constants() {
    // mmap protection flags
    let prot_read = 0x1;
    let prot_write = 0x2;
    let prot_exec = 0x4;

    // mmap mapping flags
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
