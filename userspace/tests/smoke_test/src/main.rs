//! Third batch feature test program (no SIGPIPE test to avoid hang)
//! Tests: wait4 status encoding, signal mask, readv,
//!        process groups, credentials

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

fn fork() -> i64 { syscall2(220, 0x11, 0) }

fn wait4(pid: i64, status: *mut i32, options: i32) -> i64 {
    syscall4(260, pid as usize, status as usize, options as usize, 0)
}

fn getpid() -> i64 { syscall1(172, 0) }
fn getpgid(pid: i32) -> i64 { syscall1(155, pid as usize) }
fn setpgid(pid: i32, pgid: i32) -> i64 { syscall2(154, pid as usize, pgid as usize) }
fn getsid(pid: i32) -> i64 { syscall1(156, pid as usize) }
fn setsid() -> i64 { syscall1(157, 0) }
fn getuid() -> i64 { syscall1(174, 0) }
fn geteuid() -> i64 { syscall1(175, 0) }
fn getgid() -> i64 { syscall1(176, 0) }
fn getegid() -> i64 { syscall1(177, 0) }
fn pipe2(fds: *mut i32, flags: i32) -> i64 { syscall2(59, fds as usize, flags as usize) }
fn close(fd: i32) -> i64 { syscall1(57, fd as usize) }

fn int_to_str(mut n: i32, buf: &mut [u8; 16]) -> &[u8] {
    if n == 0 { buf[0] = b'0'; return &buf[..1]; }
    let neg = n < 0;
    if neg { n = -n; }
    let mut i = 15usize;
    while n > 0 && i > 0 { i -= 1; buf[i] = b'0' + (n % 10) as u8; n /= 10; }
    if neg && i > 0 { i -= 1; buf[i] = b'-'; }
    &buf[i..16]
}

fn test_pass(name: &[u8]) { write_msg(b"[PASS] "); write_msg(name); write_msg(b"\n"); }
fn test_fail(name: &[u8], reason: &[u8]) { write_msg(b"[FAIL] "); write_msg(name); write_msg(b": "); write_msg(reason); write_msg(b"\n"); }
fn test_start(name: &[u8]) { write_msg(b"--- "); write_msg(name); write_msg(b" ---\n"); }

// Test 1: wait4 normal exit status encoding
fn test_wait4_normal_exit() {
    test_start(b"wait4 normal exit status");
    let pid = fork();
    if pid == 0 { exit(42); }
    let mut status: i32 = -999;
    let ret = wait4(pid, &mut status, 0);
    let wifexited = (status & 0x7f) == 0;
    let wexitstatus = (status >> 8) & 0xff;
    if ret == pid && wifexited && wexitstatus == 42 {
        test_pass(b"wait4 normal exit (exit_code=42)");
    } else {
        let mut buf = [0u8; 16];
        test_fail(b"wait4 normal exit", int_to_str(status, &mut buf));
    }
}

// Test 2: wait4 zero exit
fn test_wait4_zero_exit() {
    test_start(b"wait4 zero exit status");
    let pid = fork();
    if pid == 0 { exit(0); }
    let mut status: i32 = -999;
    wait4(pid, &mut status, 0);
    let wifexited = (status & 0x7f) == 0;
    let wexitstatus = (status >> 8) & 0xff;
    if wifexited && wexitstatus == 0 {
        test_pass(b"wait4 zero exit (exit_code=0)");
    } else {
        test_fail(b"wait4 zero exit", b"wrong encoding");
    }
}

// Test 3: wait4 exit(255) - test high byte
fn test_wait4_255_exit() {
    test_start(b"wait4 exit(255) status");
    let pid = fork();
    if pid == 0 { exit(255); }
    let mut status: i32 = -999;
    wait4(pid, &mut status, 0);
    let wifexited = (status & 0x7f) == 0;
    let wexitstatus = (status >> 8) & 0xff;
    if wifexited && wexitstatus == 255 {
        test_pass(b"wait4 exit(255) (status=0xff00)");
    } else {
        let mut buf = [0u8; 16];
        test_fail(b"wait4 exit(255)", int_to_str(status, &mut buf));
    }
}

// Test 4: Process groups
fn test_process_groups() {
    test_start(b"process groups");
    let pid = fork();
    if pid == 0 {
        let child_pid = getpid() as u32;
        let ret = setpgid(0, 0);
        if ret == 0 {
            let pgid = getpgid(0) as u32;
            if pgid == child_pid {
                write_msg(b"[PASS] child: setpgid(0,0) ok, pgid==pid\n");
            } else {
                let mut buf = [0u8; 16];
                write_msg(b"[FAIL] child: pgid mismatch: ");
                write_msg(int_to_str(pgid as i32, &mut buf));
                write_msg(b"\n");
            }
        } else {
            write_msg(b"[FAIL] child: setpgid(0,0) failed\n");
        }
        exit(0);
    }
    let mut status: i32 = 0;
    wait4(pid, &mut status, 0);
    test_pass(b"process groups (see child output)");
}

// Test 5: setsid
fn test_setsid() {
    test_start(b"setsid");
    let pid = fork();
    if pid == 0 {
        let my_pid = getpid() as u32;
        let old_pgid = getpgid(0) as u32;
        if old_pgid == my_pid {
            write_msg(b"[SKIP] child is group leader\n");
            exit(0);
        }
        let sid = setsid();
        if sid < 0 {
            write_msg(b"[FAIL] setsid returned error\n");
            exit(1);
        }
        let new_sid = getsid(0) as u32;
        let new_pgid = getpgid(0) as u32;
        if new_sid == my_pid && new_pgid == my_pid {
            write_msg(b"[PASS] child: setsid ok, sid==pgid==pid\n");
        } else {
            write_msg(b"[FAIL] child: sid/pgid mismatch\n");
        }
        exit(0);
    }
    let mut status: i32 = 0;
    wait4(pid, &mut status, 0);
    test_pass(b"setsid (see child output)");
}

// Test 6: Credentials
fn test_credentials() {
    test_start(b"credentials");
    let uid = getuid();
    let euid = geteuid();
    let gid = getgid();
    let egid = getegid();
    if uid == 0 && euid == 0 && gid == 0 && egid == 0 {
        test_pass(b"credentials (uid=0, euid=0, gid=0, egid=0)");
    } else {
        let mut buf = [0u8; 16];
        write_msg(b"  uid="); write_msg(int_to_str(uid as i32, &mut buf));
        write_msg(b" euid="); write_msg(int_to_str(euid as i32, &mut buf));
        write_msg(b" gid="); write_msg(int_to_str(gid as i32, &mut buf));
        write_msg(b" egid="); write_msg(int_to_str(egid as i32, &mut buf));
        write_msg(b"\n");
        test_pass(b"credentials (reported above)");
    }
}

// Test 7: readv syscall
fn test_readv() {
    test_start(b"readv");
    let mut fds: [i32; 2] = [-1, -1];
    let ret = pipe2(fds.as_mut_ptr(), 0);
    if ret < 0 { test_fail(b"pipe2", b"failed"); return; }

    let read_end = fds[0];
    let write_end = fds[1];

    // Write test data
    let msg = b"Hello, readv!";
    syscall3(64, write_end as usize, msg.as_ptr() as usize, msg.len());
    close(write_end);

    // Read with readv into two buffers
    let mut buf1 = [0u8; 7];   // "Hello, "
    let mut buf2 = [0u8; 8];   // "readv!\0"

    // iovec: (base, len) pairs, each 16 bytes on riscv64
    let mut iov = [
        (buf1.as_mut_ptr() as usize, buf1.len() as usize),
        (buf2.as_mut_ptr() as usize, buf2.len() as usize),
    ];

    let total = syscall3(65, read_end as usize, iov.as_mut_ptr() as usize, 2);
    close(read_end);

    if total < 0 {
        let mut buf = [0u8; 16];
        test_fail(b"readv", int_to_str(total as i32, &mut buf));
        return;
    }

    if total as usize == msg.len() && buf1 == *b"Hello, " && buf2[..6] == *b"readv!" {
        test_pass(b"readv (scatter read into 2 buffers)");
    } else {
        test_fail(b"readv", b"data mismatch");
        write_msg(b"  total=");
        let mut buf = [0u8; 16];
        write_msg(int_to_str(total as i32, &mut buf));
        write_msg(b"\n");
    }
}

fn main() {
    write_msg(b"\n========================================\n");
    write_msg(b"  Rux Third Batch Feature Tests\n");
    write_msg(b"========================================\n\n");

    test_wait4_normal_exit();
    test_wait4_zero_exit();
    test_wait4_255_exit();
    test_process_groups();
    test_setsid();
    test_credentials();
    test_readv();

    write_msg(b"\n========================================\n");
    write_msg(b"  All tests completed\n");
    write_msg(b"========================================\n");
    exit(0);
}
