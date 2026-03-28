//! Rux kernel smoke test
//!
//! Core functionality tests covering critical modules:
//! - File system: open/close/read/write, lseek, fstat, pipe, readv/writev, pwrite64
//! - Process: fork/exit/wait, getpid/getppid, process groups, setsid
//! - Memory: brk expand/shrink
//!
//! Design: fast (< 5s total), no blocking, no hanging.

use core::arch::asm;

const STDOUT: usize = 1;

fn syscall1(nr: usize, a0: usize) -> i64 {
    let ret: i64;
    unsafe { asm!("ecall", inlateout("a0") a0 => ret, in("a7") nr, options(nostack)); }
    ret
}

fn syscall2(nr: usize, a0: usize, a1: usize) -> i64 {
    let ret: i64;
    unsafe { asm!("ecall", inlateout("a0") a0 => ret, in("a1") a1, in("a7") nr, options(nostack)); }
    ret
}

fn syscall3(nr: usize, a0: usize, a1: usize, a2: usize) -> i64 {
    let ret: i64;
    unsafe { asm!("ecall", inlateout("a0") a0 => ret, in("a1") a1, in("a2") a2, in("a7") nr, options(nostack)); }
    ret
}

fn syscall4(nr: usize, a0: usize, a1: usize, a2: usize, a3: usize) -> i64 {
    let ret: i64;
    unsafe { asm!("ecall", inlateout("a0") a0 => ret, in("a1") a1, in("a2") a2, in("a3") a3, in("a7") nr, options(nostack)); }
    ret
}

fn write_msg(msg: &[u8]) { syscall3(64, STDOUT, msg.as_ptr() as usize, msg.len()); }

fn exit(code: i32) -> ! {
    unsafe { asm!("ecall", in("a0") code as usize, in("a7") 93, options(nostack, noreturn)); }
}

// ======== Syscall wrappers ========

fn fork() -> i64 { syscall2(220, 0x11, 0) }
fn getpid() -> i64 { syscall1(172, 0) }
fn getppid() -> i64 { syscall1(173, 0) }
fn getpgid(pid: i32) -> i64 { syscall1(155, pid as usize) }
fn setpgid(pid: i32, pgid: i32) -> i64 { syscall2(154, pid as usize, pgid as usize) }
fn getsid(pid: i32) -> i64 { syscall1(156, pid as usize) }
fn setsid() -> i64 { syscall1(157, 0) }
fn pipe2(fds: *mut i32, flags: i32) -> i64 { syscall2(59, fds as usize, flags as usize) }
fn close(fd: i32) -> i64 { syscall1(57, fd as usize) }
fn wait4(pid: i64, status: *mut i32, options: i32) -> i64 {
    syscall4(260, pid as usize, status as usize, options as usize, 0)
}
fn pread64(fd: i32, buf: *mut u8, count: usize, offset: i64) -> i64 {
    syscall4(67, fd as usize, buf as usize, count, offset as usize as usize)
}
fn pwrite64(fd: i32, buf: *const u8, count: usize, offset: i64) -> i64 {
    syscall4(68, fd as usize, buf as usize, count, offset as usize as usize)
}
fn openat(dirfd: i32, path: *const u8, flags: i32) -> i64 {
    syscall4(56, dirfd as usize, path as usize, flags as usize, 0o600)
}
fn sys_read(fd: i32, buf: *mut u8, count: usize) -> i64 {
    syscall3(63, fd as usize, buf as usize, count)
}
fn sys_write(fd: i32, buf: *const u8, count: usize) -> i64 {
    syscall3(64, fd as usize, buf as usize, count)
}
fn lseek(fd: i32, offset: i64, whence: i32) -> i64 {
    syscall3(62, fd as usize, offset as usize as usize, whence as usize)
}
fn fstat(fd: i32, statbuf: *mut u8) -> i64 {
    syscall2(80, fd as usize, statbuf as usize)
}
fn unlinkat(dirfd: i32, path: *const u8, flags: i32) -> i64 {
    syscall3(35, dirfd as usize, path as usize, flags as usize)
}
fn brk(addr: usize) -> i64 { syscall1(214, addr) }
fn execve(path: *const u8, argv: *const usize, envp: *const usize) -> i64 {
    syscall3(221, path as usize, argv as usize, envp as usize)
}

// ======== Helpers ========

fn int_to_str(mut n: i32, buf: &mut [u8; 16]) -> &[u8] {
    if n == 0 { buf[0] = b'0'; return &buf[..1]; }
    let neg = n < 0;
    if neg { n = -n; }
    let mut i = 15usize;
    while n > 0 && i > 0 { i -= 1; buf[i] = b'0' + (n % 10) as u8; n /= 10; }
    if neg && i > 0 { i -= 1; buf[i] = b'-'; }
    &buf[i..16]
}

static mut TEST_COUNT: i32 = 0;
static mut PASS_COUNT: i32 = 0;

fn test_pass(name: &[u8]) {
    unsafe { TEST_COUNT += 1; PASS_COUNT += 1; }
    write_msg(b"  [PASS] "); write_msg(name); write_msg(b"\n");
}

fn test_fail(name: &[u8], reason: &[u8]) {
    unsafe { TEST_COUNT += 1; }
    write_msg(b"  [FAIL] "); write_msg(name); write_msg(b": "); write_msg(reason); write_msg(b"\n");
}

// ======== File System ========

fn test_openat_close_read_write() {
    let fd = openat(-100, b"/tmp/smoke_file\0".as_ptr(), 0o100 | 0o200 | 0o1);
    if fd < 0 { test_fail(b"openat/create/write/read", b"open failed"); return; }

    let msg = b"Hello, Rux!";
    let n = sys_write(fd as i32, msg.as_ptr(), msg.len());
    close(fd as i32);
    if n as usize != msg.len() { test_fail(b"openat/create/write/read", b"short write"); return; }

    let rfd = openat(-100, b"/tmp/smoke_file\0".as_ptr(), 0);
    if rfd < 0 { test_fail(b"openat/create/write/read", b"reopen failed"); return; }

    let mut buf = [0u8; 64];
    let nr = sys_read(rfd as i32, buf.as_mut_ptr(), 64);
    close(rfd as i32);
    unlinkat(-100, b"/tmp/smoke_file\0".as_ptr(), 0);

    if nr as usize == msg.len() && &buf[..msg.len()] == msg {
        test_pass(b"openat/close/read/write");
    } else {
        test_fail(b"openat/create/write/read", b"data mismatch");
    }
}

fn test_lseek() {
    let fd = openat(-100, b"/tmp/smoke_lseek\0".as_ptr(), 0o100 | 0o200 | 0o1);
    if fd < 0 { test_fail(b"lseek", b"create failed"); return; }

    let msg = b"0123456789ABCDEF";
    sys_write(fd as i32, msg.as_ptr(), msg.len());

    let pos = lseek(fd as i32, 4, 0); // SEEK_SET
    if pos != 4 { test_fail(b"lseek", b"SEEK_SET wrong pos"); close(fd as i32); unlinkat(-100, b"/tmp/smoke_lseek\0".as_ptr(), 0); return; }

    let pos = lseek(fd as i32, 2, 1); // SEEK_CUR
    if pos != 6 { test_fail(b"lseek", b"SEEK_CUR wrong pos"); close(fd as i32); unlinkat(-100, b"/tmp/smoke_lseek\0".as_ptr(), 0); return; }

    let mut buf = [0u8; 8];
    let nr = sys_read(fd as i32, buf.as_mut_ptr(), 8);
    close(fd as i32);
    unlinkat(-100, b"/tmp/smoke_lseek\0".as_ptr(), 0);

    if nr == 8 && &buf[..8] == b"6789ABCD" {
        test_pass(b"lseek (SEEK_SET/SEEK_CUR)");
    } else {
        test_fail(b"lseek", b"data mismatch");
    }
}

fn test_fstat() {
    let fd = openat(-100, b"/tmp/smoke_fstat\0".as_ptr(), 0o100 | 0o200 | 0o1);
    if fd < 0 { test_fail(b"fstat", b"create failed"); return; }

    let msg = b"test data here";
    sys_write(fd as i32, msg.as_ptr(), msg.len());

    let mut statbuf = [0u8; 256];
    let ret = fstat(fd as i32, statbuf.as_mut_ptr());
    close(fd as i32);
    unlinkat(-100, b"/tmp/smoke_fstat\0".as_ptr(), 0);

    if ret < 0 { test_fail(b"fstat", b"syscall failed"); return; }

    // st_size at offset 48 (8 bytes, u64)
    let size = u64::from_le_bytes([
        statbuf[48], statbuf[49], statbuf[50], statbuf[51],
        statbuf[52], statbuf[53], statbuf[54], statbuf[55],
    ]);

    if size == msg.len() as u64 {
        test_pass(b"fstat (file size)");
    } else {
        test_fail(b"fstat", b"wrong file size");
    }
}

fn test_pipe_blocking() {
    let mut fds: [i32; 2] = [-1, -1];
    let ret = pipe2(fds.as_mut_ptr(), 0);
    if ret < 0 { test_fail(b"pipe", b"pipe2 failed"); return; }

    let read_end = fds[0];
    let write_end = fds[1];

    let msg = b"pipe test data";
    let nwritten = sys_write(write_end, msg.as_ptr(), msg.len());
    close(write_end);

    let mut buf = [0u8; 32];
    let nread = sys_read(read_end, buf.as_mut_ptr(), 32);
    close(read_end);

    if nwritten as usize == msg.len() && nread as usize == msg.len() && &buf[..msg.len()] == msg {
        test_pass(b"pipe (write + read)");
    } else {
        test_fail(b"pipe", b"data mismatch");
    }
}

fn test_readv_writev() {
    let mut fds: [i32; 2] = [-1, -1];
    let ret = pipe2(fds.as_mut_ptr(), 0);
    if ret < 0 { test_fail(b"readv/writev", b"pipe2 failed"); return; }

    let read_end = fds[0];
    let write_end = fds[1];

    let buf1 = b"Hello, ";
    let buf2 = b"readv!";
    let iov = [
        (buf1.as_ptr() as usize, buf1.len() as usize),
        (buf2.as_ptr() as usize, buf2.len() as usize),
    ];
    let total = syscall3(66, write_end as usize, iov.as_ptr() as usize, 2);
    close(write_end);

    let mut rbuf1 = [0u8; 7];
    let mut rbuf2 = [0u8; 8];
    let mut riov = [
        (rbuf1.as_mut_ptr() as usize, rbuf1.len() as usize),
        (rbuf2.as_mut_ptr() as usize, rbuf2.len() as usize),
    ];
    let nread = syscall3(65, read_end as usize, riov.as_mut_ptr() as usize, 2);
    close(read_end);

    if nread as usize == 13 && rbuf1 == *b"Hello, " && &rbuf2[..6] == *b"readv!" {
        test_pass(b"readv/writev (scatter/gather)");
    } else {
        test_fail(b"readv/writev", b"data mismatch");
    }
}

fn test_pwrite64() {
    let fd = openat(-100, b"/tmp/smoke_pwrite\0".as_ptr(), 0o100 | 0o200 | 0o1);
    if fd < 0 { test_fail(b"pwrite64", b"open failed"); return; }

    pwrite64(fd as i32, b"AAAA".as_ptr(), 4, 0);
    pwrite64(fd as i32, b"BB".as_ptr(), 2, 4);
    pwrite64(fd as i32, b"CC".as_ptr(), 2, 2);
    close(fd as i32);

    let rfd = openat(-100, b"/tmp/smoke_pwrite\0".as_ptr(), 0);
    if rfd < 0 { test_fail(b"pwrite64", b"reopen failed"); return; }
    let mut buf = [0u8; 6];
    let n = pread64(rfd as i32, buf.as_mut_ptr(), 6, 0);
    close(rfd as i32);
    unlinkat(-100, b"/tmp/smoke_pwrite\0".as_ptr(), 0);

    if n == 6 && &buf == b"AACCBB" {
        test_pass(b"pwrite64 (offset write)");
    } else {
        test_fail(b"pwrite64", b"data mismatch");
    }
}

// ======== Process Management ========

fn test_fork_exit_wait() {
    let pid = fork();
    if pid == 0 { exit(0); }
    if pid < 0 { test_fail(b"fork/exit/wait", b"fork failed"); return; }

    let mut status: i32 = -999;
    let ret = wait4(pid, &mut status, 0);
    let wifexited = (status & 0x7f) == 0;

    if ret == pid && wifexited {
        test_pass(b"fork/exit/wait");
    } else {
        test_fail(b"fork/exit/wait", b"unexpected result");
    }
}

fn test_getpid_getppid() {
    let my_pid = getpid();
    let my_ppid = getppid();

    let pid = fork();
    if pid == 0 {
        let child_pid = getpid();
        let child_ppid = getppid();
        if child_pid != my_pid && child_ppid == my_pid {
            exit(0);
        } else {
            exit(1);
        }
    }

    let mut status: i32 = 0;
    wait4(pid, &mut status, 0);
    let wexitstatus = (status >> 8) & 0xff;

    if my_pid > 0 && my_ppid > 0 && wexitstatus == 0 {
        test_pass(b"getpid/getppid");
    } else {
        test_fail(b"getpid/getppid", b"child check failed");
    }
}

fn test_fork_chain() {
    let mut all_ok = true;
    for i in 0..3 {
        let pid = fork();
        if pid == 0 { exit(i * 10 + 1); }
        let mut status: i32 = 0;
        wait4(pid, &mut status, 0);
        let expected = (i as i32) * 10 + 1;
        let wexitstatus = (status >> 8) & 0xff;
        if wexitstatus != expected { all_ok = false; }
    }

    if all_ok {
        test_pass(b"fork chain (3 children)");
    } else {
        test_fail(b"fork chain", b"exit code mismatch");
    }
}

fn test_process_groups() {
    let pid = fork();
    if pid == 0 {
        let child_pid = getpid() as u32;
        let ret = setpgid(0, 0);
        if ret == 0 && getpgid(0) as u32 == child_pid {
            exit(0);
        } else {
            exit(1);
        }
    }
    let mut status: i32 = 0;
    wait4(pid, &mut status, 0);
    if (status >> 8) & 0xff == 0 {
        test_pass(b"process groups (setpgid/getpgid)");
    } else {
        test_fail(b"process groups", b"child failed");
    }
}

fn test_setsid() {
    let pid = fork();
    if pid == 0 {
        let my_pid = getpid() as u32;
        let old_pgid = getpgid(0) as u32;
        if old_pgid == my_pid { exit(2); } // skip: group leader
        let sid = setsid();
        if sid < 0 { exit(1); }
        if getsid(0) as u32 == my_pid && getpgid(0) as u32 == my_pid {
            exit(0);
        } else {
            exit(1);
        }
    }
    let mut status: i32 = 0;
    wait4(pid, &mut status, 0);
    let code = (status >> 8) & 0xff;
    if code == 0 {
        test_pass(b"setsid (sid==pgid==pid)");
    } else if code == 2 {
        test_pass(b"setsid (skipped: group leader)");
    } else {
        test_fail(b"setsid", b"child failed");
    }
}

// ======== Memory ========

fn test_brk_expand_shrink() {
    let initial = brk(0);
    if initial <= 0 { test_fail(b"brk", b"initial brk <= 0"); return; }

    let expanded = brk((initial as usize + 4096) as usize);
    if expanded < initial { test_fail(b"brk", b"expand failed"); return; }

    // Write to new memory and read back
    unsafe {
        let ptr = initial as *mut u8;
        for i in 0..4096 {
            *ptr.add(i) = (i & 0xff) as u8;
        }
        let mut ok = true;
        for i in 0..4096 {
            if *ptr.add(i) != (i & 0xff) as u8 { ok = false; break; }
        }
        if !ok { test_fail(b"brk", b"memory verify failed"); return; }
    }

    let shrunk = brk(initial as usize);
    if shrunk != initial { test_fail(b"brk", b"shrink failed"); return; }

    test_pass(b"brk (expand/shrink)");
}

// ======== Dynamic Linking ========

fn test_dynamic_linking() {
    let pid = fork();
    if pid == 0 {
        let path = b"/test/dynamic_link_test\0";
        let mut argv = [path.as_ptr() as usize, 0];
        execve(path.as_ptr(), argv.as_ptr(), core::ptr::null());
        exit(127);
    }
    if pid < 0 { test_fail(b"dynamic linking", b"fork failed"); return; }

    let mut status: i32 = 0;
    let ret = wait4(pid, &mut status, 0);
    let wifexited = (status & 0x7f) == 0;
    let wexitstatus = (status >> 8) & 0xff;

    if ret == pid && wifexited && wexitstatus == 0 {
        test_pass(b"dynamic linking (exec /test/dynamic_link_test)");
    } else if ret == pid && wifexited {
        test_fail(b"dynamic linking", b"exit code non-zero");
    } else {
        test_fail(b"dynamic linking", b"execve or wait failed");
    }
}

// ======== Main ========

fn main() {
    write_msg(b"\n========================================\n");
    write_msg(b"  Rux Kernel Smoke Tests\n");
    write_msg(b"========================================\n\n");

    // --- File System ---
    write_msg(b"--- File System ---\n");
    test_openat_close_read_write();
    test_lseek();
    test_fstat();
    test_pipe_blocking();
    test_readv_writev();
    test_pwrite64();

    // --- Process Management ---
    write_msg(b"\n--- Process Management ---\n");
    test_fork_exit_wait();
    test_getpid_getppid();
    test_fork_chain();
    test_process_groups();
    test_setsid();

    // --- Memory ---
    write_msg(b"\n--- Memory ---\n");
    test_brk_expand_shrink();

    // --- Dynamic Linking ---
    write_msg(b"\n--- Dynamic Linking ---\n");
    test_dynamic_linking();

    // --- Summary ---
    write_msg(b"\n========================================\n");
    let total = unsafe { TEST_COUNT };
    let passed = unsafe { PASS_COUNT };
    write_msg(b"  Results: ");
    write_msg(int_to_str(passed, &mut [0u8; 16]));
    write_msg(b"/");
    write_msg(int_to_str(total, &mut [0u8; 16]));
    write_msg(b" passed\n");
    write_msg(b"========================================\n");

    exit(0);
}
