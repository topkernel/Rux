//! Simple fork test program
//! Tests that fork returns 0 to child and child PID to parent

use core::arch::asm;

fn write(msg: &[u8]) {
    unsafe {
        asm!(
            "ecall",
            in("a7") 64usize,      // SYS_write
            in("a0") 1usize,       // stdout
            in("a1") msg.as_ptr() as usize,
            in("a2") msg.len() as usize,
            options(nostack)
        );
    }
}

fn fork() -> i64 {
    let ret: i64;
    unsafe {
        asm!(
            "ecall",
            inlateout("a0") 0x11usize => ret,  // SIGCHLD
            in("a7") 220usize,                  // SYS_clone
            options(nostack)
        );
    }
    ret
}

fn exit(code: i32) {
    unsafe {
        asm!(
            "ecall",
            in("a0") code as usize,
            in("a7") 93usize,  // SYS_exit
            options(nostack, noreturn)
        );
    }
}

fn wait4(pid: i64, status: *mut i32) {
    unsafe {
        asm!(
            "ecall",
            in("a0") pid as usize,
            in("a1") status as usize,
            in("a2") 0usize,      // options
            in("a3") 0usize,      // rusage
            in("a7") 260usize,    // SYS_wait4
            options(nostack)
        );
    }
}

fn main() {
    write(b"fork_test: starting\n");

    // fork() - clone with SIGCHLD
    let pid = fork();

    if pid == 0 {
        // Child process
        write(b"fork_test: CHILD (return value=0)\n");
        exit(0);
    } else if pid > 0 {
        // Parent process
        write(b"fork_test: PARENT (child pid > 0)\n");

        // Wait for child
        let mut status: i32 = 0;
        wait4(pid, &mut status);

        write(b"fork_test: child exited\n");
        exit(0);
    } else {
        // Error (negative return)
        write(b"fork_test: FAILED (negative return)\n");
        exit(1);
    }
}
