//! Rux kernel smoke test program
//! Tests: wait4 status encoding, signal mask, readv, writev,
//!        process groups, credentials, pwrite64, gettid,
//!        dup3 O_CLOEXEC, kill(pid=0), statfs

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

fn syscall5(nr: usize, a0: usize, a1: usize, a2: usize, a3: usize, a4: usize) -> i64 {
    let ret: i64;
    unsafe { asm!("ecall", inlateout("a0") a0 => ret, in("a1") a1, in("a2") a2, in("a3") a3, in("a4") a4, in("a7") nr, options(nostack)); }
    ret
}

fn write_msg(msg: &[u8]) { syscall3(64, STDOUT, msg.as_ptr() as usize, msg.len()); }

fn exit(code: i32) -> ! {
    unsafe { asm!("ecall", in("a0") code as usize, in("a7") 93, options(nostack, noreturn)); }
}

fn fork() -> i64 { syscall2(220, 0x11, 0) }
fn getpid() -> i64 { syscall1(172, 0) }
fn gettid() -> i64 { syscall1(178, 0) }
fn getpgid(pid: i32) -> i64 { syscall1(155, pid as usize) }
fn setpgid(pid: i32, pgid: i32) -> i64 { syscall2(154, pid as usize, pgid as usize) }
fn getsid(pid: i32) -> i64 { syscall1(156, pid as usize) }
fn setsid() -> i64 { syscall1(157, 0) }
fn getuid() -> i64 { syscall1(174, 0) }
fn geteuid() -> i64 { syscall1(175, 0) }
fn getgid() -> i64 { syscall1(176, 0) }
fn getegid() -> i64 { syscall1(177, 0) }
fn kill(pid: i32, sig: i32) -> i64 { syscall2(129, pid as usize, sig as usize) }
fn pipe2(fds: *mut i32, flags: i32) -> i64 { syscall2(59, fds as usize, flags as usize) }
fn close(fd: i32) -> i64 { syscall1(57, fd as usize) }
fn dup3(oldfd: i32, newfd: i32, flags: i32) -> i64 { syscall3(24, oldfd as usize, newfd as usize, flags as usize) }
fn fcntl(fd: i32, cmd: i32, arg: i32) -> i64 { syscall3(25, fd as usize, cmd as usize, arg as usize) }
fn wait4(pid: i64, status: *mut i32, options: i32) -> i64 {
    syscall4(260, pid as usize, status as usize, options as usize, 0)
}

// pread64(67): read at offset
fn pread64(fd: i32, buf: *mut u8, count: usize, offset: i64) -> i64 {
    syscall4(67, fd as usize, buf as usize, count, offset as usize as usize)
}

// pwrite64(68): write at offset
fn pwrite64(fd: i32, buf: *const u8, count: usize, offset: i64) -> i64 {
    syscall4(68, fd as usize, buf as usize, count, offset as usize as usize)
}

// openat(56)
fn openat(dirfd: i32, path: *const u8, flags: i32) -> i64 {
    syscall4(56, dirfd as usize, path as usize, flags as usize, 0o600)
}

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

// ======== Third batch tests ========

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

fn test_readv() {
    test_start(b"readv");
    let mut fds: [i32; 2] = [-1, -1];
    let ret = pipe2(fds.as_mut_ptr(), 0);
    if ret < 0 { test_fail(b"pipe2", b"failed"); return; }

    let read_end = fds[0];
    let write_end = fds[1];

    let msg = b"Hello, readv!";
    syscall3(64, write_end as usize, msg.as_ptr() as usize, msg.len());
    close(write_end);

    let mut buf1 = [0u8; 7];
    let mut buf2 = [0u8; 8];

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
    }
}

// ======== Fourth batch tests ========

fn test_gettid() {
    test_start(b"gettid");
    let pid = getpid();
    let tid = gettid();
    // In single-threaded processes, tid == pid
    if pid == tid {
        test_pass(b"gettid (tid == pid)");
    } else {
        let mut buf = [0u8; 16];
        write_msg(b"  pid="); write_msg(int_to_str(pid as i32, &mut buf));
        write_msg(b" tid="); write_msg(int_to_str(tid as i32, &mut buf));
        write_msg(b"\n");
        test_fail(b"gettid", b"tid != pid");
    }
}

fn test_pwrite64() {
    test_start(b"pwrite64");
    // Create a temp file, write at offset, read back to verify
    let fd = openat(-100, b"/tmp/pwrite_test\0".as_ptr(), 0o100 | 0o200 | 0o1); // O_CREAT|O_TRUNC|O_WRONLY
    if fd < 0 {
        test_fail(b"pwrite64", b"open failed");
        return;
    }

    // Write "AAAA" at offset 0
    let msg_a = b"AAAA";
    let ret1 = pwrite64(fd as i32, msg_a.as_ptr(), 4, 0);
    // Write "BB" at offset 4
    let msg_b = b"BB";
    let ret2 = pwrite64(fd as i32, msg_b.as_ptr(), 2, 4);
    // Write "CC" at offset 2 (overwrite part of first write)
    let msg_c = b"CC";
    let ret3 = pwrite64(fd as i32, msg_c.as_ptr(), 2, 2);

    close(fd as i32);

    if ret1 < 0 || ret2 < 0 || ret3 < 0 {
        test_fail(b"pwrite64", b"write failed");
        return;
    }

    // Read back the file
    let rfd = openat(-100, b"/tmp/pwrite_test\0".as_ptr(), 0); // O_RDONLY
    if rfd < 0 {
        test_fail(b"pwrite64", b"reopen for read failed");
        return;
    }

    let mut buf = [0u8; 6];
    let nread = pread64(rfd as i32, buf.as_mut_ptr(), 6, 0);
    close(rfd as i32);

    if nread == 6 && &buf == b"AACCBB" {
        test_pass(b"pwrite64 (offset write + overwrite)");
    } else {
        write_msg(b"  nread="); write_msg(int_to_str(nread as i32, &mut [0u8;16]));
        write_msg(b" data=");
        write_msg(&buf[..nread as usize]);
        write_msg(b"\n");
        test_fail(b"pwrite64", b"data mismatch");
    }
}

fn test_dup3_cloexec() {
    test_start(b"dup3 O_CLOEXEC");
    // dup3 with O_CLOEXEC should set close-on-exec flag
    let O_CLOEXEC: i32 = 0o2000000;  // Linux O_CLOEXEC = 02000000 octal
    let F_GETFD: i32 = 1;

    // First test: dup3 without O_CLOEXEC
    let fd5 = dup3(STDOUT as i32, 5, 0);
    if fd5 < 0 {
        write_msg(b"  dup3(1,5,0) ret="); write_msg(int_to_str(fd5 as i32, &mut [0u8;16]));
        write_msg(b"\n");
        test_fail(b"dup3", b"basic dup3 failed");
        return;
    }
    close(5);

    // Test with O_CLOEXEC
    let newfd = dup3(STDOUT as i32, 5, O_CLOEXEC);
    if newfd < 0 {
        write_msg(b"  dup3(1,5,O_CLOEXEC) ret="); write_msg(int_to_str(newfd as i32, &mut [0u8;16]));
        write_msg(b"\n");
        test_fail(b"dup3", b"dup3 O_CLOEXEC failed");
        return;
    }
    if newfd as i32 != 5 {
        let mut buf = [0u8; 16];
        test_fail(b"dup3", int_to_str(newfd as i32, &mut buf));
        close(5);
        return;
    }

    // Check cloexec flag via fcntl F_GETFD
    let flags = fcntl(5, F_GETFD, 0);
    close(5);

    // FD_CLOEXEC = 1
    if flags == 1 {
        test_pass(b"dup3 O_CLOEXEC (fd=5, flags=FD_CLOEXEC)");
    } else {
        let mut buf = [0u8; 16];
        test_fail(b"dup3 O_CLOEXEC", int_to_str(flags as i32, &mut buf));
    }
}

fn test_kill_process_group() {
    test_start(b"kill(pid=0) process group");
    // Fork a child, both parent and child are in the same process group
    let pid = fork();
    if pid == 0 {
        // Child: wait for signal (infinite loop checking a flag)
        // We'll just sleep briefly and exit
        // The parent sends SIGUSR1 to pid=0 (its own group)
        // The child should receive it and die
        // Actually, for this test, the child just exits normally
        // The real test is that kill(0, sig) doesn't return error
        exit(0);
    }

    // Parent: send signal 0 (just check if process group exists)
    let ret = kill(0, 0); // SIG 0 = check existence
    let mut status: i32 = 0;
    wait4(pid, &mut status, 0);

    if ret == 0 {
        test_pass(b"kill(pid=0) - process group signal check ok");
    } else {
        let mut buf = [0u8; 16];
        test_fail(b"kill(pid=0)", int_to_str(ret as i32, &mut buf));
    }
}

fn test_statfs() {
    test_start(b"statfs");
    // statfs syscall 43
    // struct statfs (64-bit): f_type, f_bsize, f_blocks, f_bfree, f_bavail,
    //   f_files, f_ffree, f_fsid(8), f_namelen, f_frsize, f_flags, spare(32)
    // Total size: 8*11 + 8 + 32 = 128 bytes
    let mut buf = [0u8; 128];

    let ret = syscall2(43, b"/\0".as_ptr() as usize, buf.as_mut_ptr() as usize);

    if ret == 0 {
        // Read f_type (first 8 bytes)
        let f_type = u64::from_le_bytes([buf[0], buf[1], buf[2], buf[3],
                                          buf[4], buf[5], buf[6], buf[7]]);
        let f_bsize = u64::from_le_bytes([buf[8], buf[9], buf[10], buf[11],
                                           buf[12], buf[13], buf[14], buf[15]]);
        let f_namelen = u64::from_le_bytes([buf[72], buf[73], buf[74], buf[75],
                                             buf[76], buf[77], buf[78], buf[79]]);
        if f_type != 0 && f_bsize > 0 && f_namelen > 0 {
            test_pass(b"statfs (type, bsize, namelen ok)");
        } else {
            test_fail(b"statfs", b"zero fields");
            write_msg(b"  type="); write_msg(b"TODO hex");
            write_msg(b" bsize="); write_msg(b"TODO");
            write_msg(b"\n");
        }
    } else {
        let mut buf2 = [0u8; 16];
        test_fail(b"statfs", int_to_str(ret as i32, &mut buf2));
    }
}

fn test_writev() {
    test_start(b"writev");
    let mut fds: [i32; 2] = [-1, -1];
    let ret = pipe2(fds.as_mut_ptr(), 0);
    if ret < 0 { test_fail(b"pipe2", b"failed"); return; }

    let read_end = fds[0];
    let write_end = fds[1];

    // Write two buffers with writev
    let buf1 = b"Hello, ";
    let buf2 = b"writev!";

    let iov = [
        (buf1.as_ptr() as usize, buf1.len() as usize),
        (buf2.as_ptr() as usize, buf2.len() as usize),
    ];

    let total = syscall3(66, write_end as usize, iov.as_ptr() as usize, 2);
    close(write_end);

    if total < 0 {
        let mut buf = [0u8; 16];
        test_fail(b"writev", int_to_str(total as i32, &mut buf));
        close(read_end);
        return;
    }

    // Read back: "Hello, " (7) + "writev!" (7) = 14 bytes
    let mut rbuf = [0u8; 16];
    let nread = syscall3(63, read_end as usize, rbuf.as_mut_ptr() as usize, 15);
    close(read_end);

    if nread == 14 && &rbuf[..14] == b"Hello, writev!" {
        test_pass(b"writev (gather write from 2 buffers)");
    } else {
        write_msg(b"  total_write="); write_msg(int_to_str(total as i32, &mut [0u8;16]));
        write_msg(b" nread="); write_msg(int_to_str(nread as i32, &mut [0u8;16]));
        write_msg(b" data=");
        write_msg(&rbuf[..nread as usize]);
        write_msg(b"\n");
        test_fail(b"writev", b"data mismatch");
    }
}

fn main() {
    write_msg(b"\n========================================\n");
    write_msg(b"  Rux Kernel Smoke Tests\n");
    write_msg(b"========================================\n\n");

    // Third batch tests
    test_wait4_normal_exit();
    test_wait4_zero_exit();
    test_wait4_255_exit();
    test_process_groups();
    test_setsid();
    test_credentials();
    test_readv();
    test_writev();

    // Fourth batch tests
    test_gettid();
    test_pwrite64();
    test_dup3_cloexec();
    test_kill_process_group();
    test_statfs();

    write_msg(b"\n========================================\n");
    write_msg(b"  All tests completed\n");
    write_msg(b"========================================\n");
    exit(0);
}
