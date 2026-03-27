//! Minimal pipe SIGPIPE test

use core::arch::asm;

fn syscall3(nr: usize, a0: usize, a1: usize, a2: usize) -> i64 {
    let ret: i64;
    unsafe { asm!("ecall", inlateout("a0") a0 => ret, in("a1") a1, in("a2") a2, in("a7") nr, options(nostack)); }
    ret
}

fn syscall2(nr: usize, a0: usize, a1: usize) -> i64 {
    let ret: i64;
    unsafe { asm!("ecall", inlateout("a0") a0 => ret, in("a1") a1, in("a7") nr, options(nostack)); }
    ret
}

fn syscall1(nr: usize, a0: usize) -> i64 {
    let ret: i64;
    unsafe { asm!("ecall", inlateout("a0") a0 => ret, in("a7") nr, options(nostack)); }
    ret
}

fn write_msg(msg: &[u8]) { syscall3(64, 1, msg.as_ptr() as usize, msg.len()); }

fn exit(code: i32) -> ! {
    unsafe { asm!("ecall", in("a0") code as usize, in("a7") 93, options(nostack, noreturn)); }
}

fn main() {
    write_msg(b"pipe_test: start\n");

    // Create pipe: fds[0]=read, fds[1]=write
    let mut fds: [i32; 2] = [0, 0];
    let ret = syscall2(59, fds.as_mut_ptr() as usize, 0);  // pipe2
    if ret < 0 {
        write_msg(b"pipe_test: pipe2 failed\n");
        exit(1);
    }
    write_msg(b"pipe_test: pipe created\n");

    let read_fd = fds[0];
    let write_fd = fds[1];

    write_msg(b"pipe_test: closing read end...\n");
    // Close read end
    syscall1(57, read_fd as usize);  // close

    write_msg(b"pipe_test: writing to pipe with no reader...\n");
    // Write to pipe with no reader - should get EPIPE
    let buf = [0u8; 1];
    let ret = syscall3(64, write_fd as usize, buf.as_ptr() as usize, 1);  // write

    write_msg(b"pipe_test: write returned\n");

    syscall1(57, write_fd as usize);  // close write end

    // EPIPE = 32
    if ret == -32 {
        write_msg(b"pipe_test: PASS (got -EPIPE)\n");
    } else {
        write_msg(b"pipe_test: FAIL (got unexpected return)\n");
    }

    exit(0);
}
