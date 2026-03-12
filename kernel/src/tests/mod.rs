//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! Unit test module
//!
//! All unit test functions are in this module, controlled by `unit-test` feature.
//!
//! Run tests:
//! ```bash
//! cargo build --package rux --features riscv64,unit-test
//! qemu-system-riscv64 -M virt -cpu rv64 -m 2G -nographic \
//!   -kernel target/riscv64gc-unknown-none-elf/debug/rux
//! ```

use crate::println;
use core::sync::atomic::{AtomicUsize, Ordering};
use spin::Mutex;

/// Maximum number of failed tests to record
const MAX_FAILED_TESTS: usize = 32;

/// Failed test record
#[cfg(feature = "unit-test")]
struct FailedTest {
    name: [u8; 64],
    name_len: usize,
    reason: [u8; 128],
    reason_len: usize,
}

#[cfg(feature = "unit-test")]
impl FailedTest {
    const fn new() -> Self {
        Self {
            name: [0; 64],
            name_len: 0,
            reason: [0; 128],
            reason_len: 0,
        }
    }

    fn set(&mut self, name: &str, reason: &str) {
        self.name_len = name.as_bytes().len().min(64);
        self.name[..self.name_len].copy_from_slice(&name.as_bytes()[..self.name_len]);
        self.reason_len = reason.as_bytes().len().min(128);
        self.reason[..self.reason_len].copy_from_slice(&reason.as_bytes()[..self.reason_len]);
    }

    fn name(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len]).unwrap_or("???")
    }

    fn reason(&self) -> &str {
        core::str::from_utf8(&self.reason[..self.reason_len]).unwrap_or("???")
    }
}

/// Global test statistics
#[cfg(feature = "unit-test")]
static TEST_PASSED: AtomicUsize = AtomicUsize::new(0);
#[cfg(feature = "unit-test")]
static TEST_FAILED: AtomicUsize = AtomicUsize::new(0);
#[cfg(feature = "unit-test")]
static TEST_CURRENT: AtomicUsize = AtomicUsize::new(0);

/// Failed test list
#[cfg(feature = "unit-test")]
static FAILED_TESTS: Mutex<[FailedTest; MAX_FAILED_TESTS]> = Mutex::new([const { FailedTest::new() }; MAX_FAILED_TESTS]);
#[cfg(feature = "unit-test")]
static FAILED_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Record test pass
#[cfg(feature = "unit-test")]
pub fn test_pass(name: &str) {
    TEST_PASSED.fetch_add(1, Ordering::SeqCst);
    println!("test:   \u{1b}[32mPASS\u{1b}[0m {}", name);
}

/// Record test fail
#[cfg(feature = "unit-test")]
pub fn test_fail(name: &str, reason: &str) {
    TEST_FAILED.fetch_add(1, Ordering::SeqCst);
    println!("test:   \u{1b}[31mFAIL\u{1b}[0m {} - {}", name, reason);

    // Record failed test
    let idx = FAILED_COUNT.fetch_add(1, Ordering::SeqCst);
    if idx < MAX_FAILED_TESTS {
        let mut failed = FAILED_TESTS.lock();
        failed[idx].set(name, reason);
    }
}

/// Record test skip
#[cfg(feature = "unit-test")]
pub fn test_skip(name: &str, reason: &str) {
    println!("test:   \u{1b}[33mSKIP\u{1b}[0m {} - {}", name, reason);
}

/// Start a test group
#[cfg(feature = "unit-test")]
pub fn test_group_start(name: &str) {
    let idx = TEST_CURRENT.fetch_add(1, Ordering::SeqCst);
    println!("\ntest: [{}] {} ================================", idx + 1, name);
}

/// Assert macro - records but doesn't panic on failure
#[cfg(feature = "unit-test")]
#[macro_export]
macro_rules! test_assert {
    ($cond:expr, $name:expr) => {
        if $cond {
            $crate::tests::test_pass($name);
        } else {
            $crate::tests::test_fail($name, "assertion failed");
        }
    };
    ($cond:expr, $name:expr, $reason:expr) => {
        if $cond {
            $crate::tests::test_pass($name);
        } else {
            $crate::tests::test_fail($name, $reason);
        }
    };
}

/// Assert equal macro
#[cfg(feature = "unit-test")]
#[macro_export]
macro_rules! test_assert_eq {
    ($left:expr, $right:expr, $name:expr) => {
        if $left == $right {
            $crate::tests::test_pass($name);
        } else {
            $crate::tests::test_fail($name, concat!("expected ", stringify!($left), " == ", stringify!($right)));
        }
    };
}

#[cfg(feature = "unit-test")]
pub mod file_open;
#[cfg(feature = "unit-test")]
pub mod listhead;
#[cfg(feature = "unit-test")]
pub mod path;
#[cfg(feature = "unit-test")]
pub mod file_flags;
#[cfg(feature = "unit-test")]
pub mod fdtable;
#[cfg(feature = "unit-test")]
pub mod heap_allocator;
#[cfg(feature = "unit-test")]
pub mod page_allocator;
#[cfg(feature = "unit-test")]
pub mod scheduler;
#[cfg(feature = "unit-test")]
pub mod signal;
#[cfg(feature = "unit-test")]
pub mod smp;
#[cfg(feature = "unit-test")]
pub mod process_tree;
#[cfg(feature = "unit-test")]
pub mod fork;
#[cfg(feature = "unit-test")]
pub mod execve;
#[cfg(feature = "unit-test")]
pub mod wait4;
#[cfg(feature = "unit-test")]
pub mod boundary;
#[cfg(feature = "unit-test")]
pub mod smp_schedule;
#[cfg(feature = "unit-test")]
pub mod getpid;
#[cfg(feature = "unit-test")]
pub mod quick;
#[cfg(feature = "unit-test")]
pub mod user_syscall;
#[cfg(feature = "unit-test")]
pub mod preemptive_scheduler;
#[cfg(feature = "unit-test")]
pub mod sleep_wakeup;
#[cfg(feature = "unit-test")]
pub mod virtio_queue;
#[cfg(feature = "unit-test")]
pub mod ext4_allocator;
#[cfg(feature = "unit-test")]
pub mod ext4_file_write;
#[cfg(feature = "unit-test")]
pub mod ext4_indirect_blocks;
#[cfg(feature = "unit-test")]
pub mod dcache;
#[cfg(feature = "unit-test")]
pub mod icache;
#[cfg(feature = "unit-test")]
pub mod standard_alloc;
#[cfg(feature = "unit-test")]
pub mod fstat;
#[cfg(feature = "unit-test")]
pub mod fcntl;
#[cfg(feature = "unit-test")]
pub mod mkdir_unlink;
#[cfg(feature = "unit-test")]
pub mod link;
#[cfg(feature = "unit-test")]
pub mod tcp_handshake;
#[cfg(feature = "unit-test")]
pub mod virtio_net;
#[cfg(feature = "unit-test")]
pub mod network;
#[cfg(feature = "unit-test")]
pub mod pipe2;
#[cfg(feature = "unit-test")]
pub mod signal_procmask;
#[cfg(feature = "unit-test")]
pub mod ipc_poll;
#[cfg(feature = "unit-test")]
pub mod ipc_epoll;
#[cfg(feature = "unit-test")]
pub mod ipc_eventfd;
#[cfg(feature = "unit-test")]
pub mod mem_mmap;
#[cfg(feature = "unit-test")]
pub mod mem_cow;
#[cfg(feature = "unit-test")]
pub mod framebuffer;

// ========== System call tests ==========
#[cfg(feature = "unit-test")]
pub mod syscall_file;
#[cfg(feature = "unit-test")]
pub mod syscall_io;
#[cfg(feature = "unit-test")]
pub mod syscall_process;
#[cfg(feature = "unit-test")]
pub mod syscall_memory;
#[cfg(feature = "unit-test")]
pub mod syscall_time;
#[cfg(feature = "unit-test")]
pub mod syscall_network;
#[cfg(feature = "unit-test")]
pub mod syscall_sched;
#[cfg(feature = "unit-test")]
pub mod syscall_signal;
#[cfg(feature = "unit-test")]
pub mod syscall_misc;

#[cfg(feature = "unit-test")]
pub fn run_all_tests() {
    println!("test: ===== Starting Rux OS Unit Tests =====");

    // 1. file_open functionality test
    file_open::test_file_open();

    // 2. ListHead doubly-linked list test
    listhead::test_listhead();

    // 3. Path parsing test
    path::test_path();

    // 4. FileFlags file flags test
    file_flags::test_file_flags();

    // 5. FdTable file descriptor management test
    fdtable::test_fdtable();

    // 6. Heap allocator test
    heap_allocator::test_heap_allocator();

    // 7. Page allocator test
    page_allocator::test_page_allocator();

    // 8. Scheduler test
    scheduler::test_scheduler();

    // 9. Signal handling test
    signal::test_signal();

    // 10. SMP multi-core startup test
    smp::test_smp();

    // 11. Process tree management test
    process_tree::test_process_tree();

    // 12. fork system call test
    fork::test_fork();

    // 13. Boundary condition test (will exhaust task pool, put at end)
    boundary::test_boundary();

    // 14. execve system call test
    execve::test_execve();

    // 14. wait4 system call test
    wait4::test_wait4();

    // 15. SMP scheduling verification test
    smp_schedule::test_smp_schedule();

    // 17. getpid/getppid system call test
    getpid::test_getpid();

    // 18. User mode system call test
    user_syscall::test_user_syscall();

    // 19. Preemptive scheduler test
    preemptive_scheduler::test_preemptive_scheduler();

    // 20. Process sleep and wakeup test
    sleep_wakeup::test_sleep_and_wakeup();

    // 21. VirtIO queue test
    virtio_queue::test_virtio_queue();

    // 22. ext4 allocator test
    ext4_allocator::test_ext4_allocator();

    // 23. ext4 file write test
    ext4_file_write::test_ext4_file_write();

    // 24. ext4 indirect block test
    ext4_indirect_blocks::test_ext4_indirect_blocks();

    // 25. Dentry cache test
    dcache::test_dcache();

    // 26. Inode cache test
    icache::test_icache();

    // 27. fstat system call test
    fstat::test_fstat();

    // 28. fcntl system call test
    fcntl::test_fcntl();

    // 29. mkdir/rmdir/unlink system call test
    mkdir_unlink::test_mkdir_unlink();

    // 30. link system call test
    link::test_link();

    // 31. TCP three-way handshake test
    tcp_handshake::test_tcp_handshake();

    // 32. VirtIO-Net network device driver test
    virtio_net::test_virtio_net();

    // 33. Network subsystem test
    network::test_network();

    // 34. pipe2 system call test
    pipe2::test_pipe2();

    // 35. rt_sigprocmask system call test
    signal_procmask::test_sigprocmask();

    // 36. poll system call test
    ipc_poll::test_poll();

    // 37. epoll system call test
    ipc_epoll::test_epoll();

    // 38. eventfd system call test
    ipc_eventfd::test_eventfd();

    // 39. mmap series memory management system call test
    mem_mmap::test_mmap_syscalls();

    // 40. Copy-on-Write (COW) test
    mem_cow::test_cow();

    // 41. Standard alloc crate type test
    // standard_alloc::test_standard_alloc();

    // 42. Framebuffer drawing test
    framebuffer::test_framebuffer();

    // ========== System call tests ==========
    // 43. File system related system call test
    syscall_file::test_syscall_file();

    // 44. IO related system call test
    syscall_io::test_syscall_io();

    // 45. Process related system call test
    syscall_process::test_syscall_process();

    // 46. Memory related system call test
    syscall_memory::test_syscall_memory();

    // 47. Time related system call test
    syscall_time::test_syscall_time();

    // 48. Network related system call test
    syscall_network::test_syscall_network();

    // 49. Scheduler related system call test
    syscall_sched::test_syscall_sched();

    // 50. Signal related system call test
    syscall_signal::test_syscall_signal();

    // 51. Miscellaneous system call test
    syscall_misc::test_syscall_misc();

    // Print test summary
    print_test_summary();
}

/// Print test summary
#[cfg(feature = "unit-test")]
pub fn print_test_summary() {
    let passed = TEST_PASSED.load(Ordering::SeqCst);
    let failed = TEST_FAILED.load(Ordering::SeqCst);
    let total = passed + failed;

    println!("\n\u{1b}[36m========================================\u{1b}[0m");
    println!("\u{1b}[36m             TEST SUMMARY\u{1b}[0m");
    println!("\u{1b}[36m========================================\u{1b}[0m");

    if failed == 0 {
        println!("\u{1b}[32m  All tests passed!\u{1b}[0m");
    } else {
        println!("\u{1b}[31m  Some tests failed!\u{1b}[0m");
    }

    println!();
    println!("  Total:   {} tests", total);
    println!("  \u{1b}[32mPassed:  {}\u{1b}[0m", passed);
    if failed > 0 {
        println!("  \u{1b}[31mFailed:  {}\u{1b}[0m", failed);
    } else {
        println!("  Failed:  0");
    }

    // Print failed test list
    if failed > 0 {
        let failed_count = FAILED_COUNT.load(Ordering::SeqCst).min(MAX_FAILED_TESTS);
        let failed_tests = FAILED_TESTS.lock();
        println!();
        println!("\u{1b}[31m  Failed tests:\u{1b}[0m");
        for i in 0..failed_count {
            println!("    \u{1b}[31m{}\u{1b}[0m - {}", failed_tests[i].name(), failed_tests[i].reason());
        }
        if failed > MAX_FAILED_TESTS {
            println!("    ... and {} more", failed - MAX_FAILED_TESTS);
        }
    }

    println!("\u{1b}[36m========================================\u{1b}[0m");

    // If there are failed tests, print obvious failure marker
    if failed > 0 {
        println!("\u{1b}[31m!!! TESTS FAILED !!!\u{1b}[0m");
    } else {
        println!("\u{1b}[32m*** ALL TESTS PASSED ***\u{1b}[0m");
    }
}

/// Get failed test count
#[cfg(feature = "unit-test")]
pub fn get_failed_count() -> usize {
    TEST_FAILED.load(Ordering::SeqCst)
}
