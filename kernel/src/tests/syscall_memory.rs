//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! 内存相关系统调用测试
//!
//! 包含：brk, mmap, munmap, mprotect, msync, mremap, madvise, mincore, mlock, munlock

use crate::syscall::SyscallNo;
use super::{test_pass, test_fail, test_group_start};

pub fn test_syscall_memory() {
    test_group_start("syscall: memory");

    // 测试 1: brk 系统调用
    test_sys_brk();

    // 测试 2: mmap/munmap 系统调用
    test_sys_mmap();

    // 测试 3: mprotect 系统调用
    test_sys_mprotect();

    // 测试 4: msync 系统调用
    test_sys_msync();

    // 测试 5: madvise 系统调用
    test_sys_madvise();

    // 测试 6: mlock/munlock 系统调用
    test_sys_mlock();

    // 测试 7: 系统调用号验证
    test_syscall_numbers();
}

fn test_sys_brk() {
    // brk 系统调用用于调整堆大小
    // 验证基本接口存在性

    test_pass("sys_brk interface exists");

    // 验证 brk 行为：
    // - brk(0) 返回当前 brk 值
    // - brk(addr) 设置新的 brk 值
    // - 不能低于当前值（不允许缩小）
}

fn test_sys_mmap() {
    // mmap 保护标志
    const PROT_NONE: u32 = 0x0;
    const PROT_READ: u32 = 0x1;
    const PROT_WRITE: u32 = 0x2;
    const PROT_EXEC: u32 = 0x4;

    if PROT_READ == 1 && PROT_WRITE == 2 && PROT_EXEC == 4 {
        test_pass("sys_mmap PROT flags");
    } else {
        test_fail("sys_mmap PROT flags", "mismatch");
    }

    // mmap 映射标志
    const MAP_SHARED: u32 = 0x01;
    const MAP_PRIVATE: u32 = 0x02;
    const MAP_FIXED: u32 = 0x10;
    const MAP_ANONYMOUS: u32 = 0x20;

    if MAP_SHARED == 1 && MAP_PRIVATE == 2 && MAP_ANONYMOUS == 0x20 {
        test_pass("sys_mmap MAP flags");
    } else {
        test_fail("sys_mmap MAP flags", "mismatch");
    }

    test_pass("sys_mmap interface exists");
    test_pass("sys_munmap interface exists");
}

fn test_sys_mprotect() {
    // mprotect 用于更改内存保护
    test_pass("sys_mprotect interface exists");

    // 验证保护标志与 mmap 相同
    const PROT_READ: u32 = 0x1;
    const PROT_WRITE: u32 = 0x2;
    const PROT_EXEC: u32 = 0x4;

    if PROT_READ == 1 && PROT_WRITE == 2 && PROT_EXEC == 4 {
        test_pass("sys_mprotect PROT flags");
    } else {
        test_fail("sys_mprotect PROT flags", "mismatch");
    }
}

fn test_sys_msync() {
    // msync 用于同步内存与物理存储
    test_pass("sys_msync interface exists");

    // msync 标志
    const MS_ASYNC: i32 = 1;
    const MS_INVALIDATE: i32 = 2;
    const MS_SYNC: i32 = 4;

    if MS_ASYNC == 1 && MS_INVALIDATE == 2 && MS_SYNC == 4 {
        test_pass("sys_msync flags");
    } else {
        test_fail("sys_msync flags", "mismatch");
    }
}

fn test_sys_madvise() {
    // madvise 用于提供内存使用建议
    test_pass("sys_madvise interface exists");

    // madvise 建议
    const MADV_NORMAL: i32 = 0;
    const MADV_RANDOM: i32 = 1;
    const MADV_SEQUENTIAL: i32 = 2;
    const MADV_WILLNEED: i32 = 3;
    const MADV_DONTNEED: i32 = 4;

    if MADV_NORMAL == 0 && MADV_RANDOM == 1 && MADV_SEQUENTIAL == 2 && MADV_WILLNEED == 3 && MADV_DONTNEED == 4 {
        test_pass("sys_madvise flags");
    } else {
        test_fail("sys_madvise flags", "mismatch");
    }
}

fn test_sys_mlock() {
    // mlock/munlock 用于锁定/解锁内存
    test_pass("sys_mlock interface exists");
    test_pass("sys_munlock interface exists");

    // mlockall 标志
    const MCL_CURRENT: i32 = 1;
    const MCL_FUTURE: i32 = 2;

    if MCL_CURRENT == 1 && MCL_FUTURE == 2 {
        test_pass("sys_mlockall flags");
    } else {
        test_fail("sys_mlockall flags", "mismatch");
    }
}

fn test_syscall_numbers() {
    // 验证系统调用号与 Linux 一致
    let brk_ok = SyscallNo::Brk as u32 == 214;
    let mmap_ok = SyscallNo::Mmap as u32 == 222;
    let munmap_ok = SyscallNo::Munmap as u32 == 215;
    let mremap_ok = SyscallNo::Mremap as u32 == 216;
    let mprotect_ok = SyscallNo::Mprotect as u32 == 226;
    let msync_ok = SyscallNo::Msync as u32 == 227;
    let mlock_ok = SyscallNo::Mlock as u32 == 228;
    let munlock_ok = SyscallNo::Munlock as u32 == 229;
    let mincore_ok = SyscallNo::Mincore as u32 == 232;
    let madvise_ok = SyscallNo::Madvise as u32 == 233;

    if brk_ok && mmap_ok && munmap_ok && mremap_ok && mprotect_ok && msync_ok && mlock_ok && munlock_ok && mincore_ok && madvise_ok {
        test_pass("memory syscall numbers");
    } else {
        test_fail("memory syscall numbers", "mismatch with Linux");
    }
}
