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

use core::sync::atomic::{AtomicUsize, Ordering};
use spin::Mutex;

/// Direct UART output for test results.
/// println! routes through printk (ring buffer only, no UART output).
/// Tests need visible output, so we write directly to UART via console::puts.
#[cfg(feature = "unit-test")]
pub fn test_println(args: core::fmt::Arguments) {
    use core::fmt::Write;
    let mut uart = crate::console::lock();
    let _ = write!(uart, "{}", args);
    uart.putc(b'\r');
    uart.putc(b'\n');
}

/// Macro wrapper for test_println (convenience, same as println! syntax).
#[cfg(feature = "unit-test")]
#[macro_export]
macro_rules! test_println {
    () => ({
        $crate::tests::test_println_str("\n")
    });
    ($($arg:tt)*) => ({
        $crate::tests::test_println(format_args!($($arg)*))
    });
}

#[cfg(feature = "unit-test")]
fn test_println_str(s: &str) {
    let mut uart = crate::console::lock();
    for b in s.bytes() {
        uart.putc(b);
        if b == b'\n' {
            uart.putc(b'\r');
        }
    }
}

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
    test_println!("test:   \u{1b}[32mPASS\u{1b}[0m {}", name);
}

/// Record test fail
#[cfg(feature = "unit-test")]
pub fn test_fail(name: &str, reason: &str) {
    TEST_FAILED.fetch_add(1, Ordering::SeqCst);
    test_println!("test:   \u{1b}[31mFAIL\u{1b}[0m {} - {}", name, reason);

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
    test_println!("test:   \u{1b}[33mSKIP\u{1b}[0m {} - {}", name, reason);
}

/// Start a test group
#[cfg(feature = "unit-test")]
pub fn test_group_start(name: &str) {
    let idx = TEST_CURRENT.fetch_add(1, Ordering::SeqCst);
    test_println!("\ntest: [{}] {} ================================", idx + 1, name);
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

// ===== Pure logic tests =====
#[cfg(feature = "unit-test")]
pub mod dev_t;
#[cfg(feature = "unit-test")]
pub mod checksum;
#[cfg(feature = "unit-test")]
pub mod errno_test;
#[cfg(feature = "unit-test")]
pub mod config_test;
#[cfg(feature = "unit-test")]
pub mod vma_flags;

// ===== Core data structures =====
#[cfg(feature = "unit-test")]
pub mod listhead;
#[cfg(feature = "unit-test")]
pub mod path;
#[cfg(feature = "unit-test")]
pub mod file_flags;
#[cfg(feature = "unit-test")]
pub mod fdtable;
#[cfg(feature = "unit-test")]
pub mod signal;

// ===== Memory management =====
#[cfg(feature = "unit-test")]
pub mod heap_allocator;
#[cfg(feature = "unit-test")]
pub mod page_allocator;
#[cfg(feature = "unit-test")]
pub mod buffer_state;
#[cfg(feature = "unit-test")]
pub mod mount_flags;

// ===== Process management =====
#[cfg(feature = "unit-test")]
pub mod scheduler;
#[cfg(feature = "unit-test")]
pub mod process_tree;
#[cfg(feature = "unit-test")]
pub mod fork;
#[cfg(feature = "unit-test")]
pub mod execve;
#[cfg(feature = "unit-test")]
pub mod wait4;
#[cfg(feature = "unit-test")]
pub mod getpid;
#[cfg(feature = "unit-test")]
pub mod sleep_wakeup;
#[cfg(feature = "unit-test")]
pub mod pid_test;

// ===== Synchronization =====
#[cfg(feature = "unit-test")]
pub mod semaphore;
#[cfg(feature = "unit-test")]
pub mod futex_test;

// ===== Scheduler =====
#[cfg(feature = "unit-test")]
pub mod smp;
#[cfg(feature = "unit-test")]
pub mod smp_schedule;
#[cfg(feature = "unit-test")]
pub mod preemptive_scheduler;

// ===== Filesystem =====
#[cfg(feature = "unit-test")]
pub mod file_open;
#[cfg(feature = "unit-test")]
pub mod dcache;
#[cfg(feature = "unit-test")]
pub mod icache;
#[cfg(feature = "unit-test")]
pub mod fstat;
#[cfg(feature = "unit-test")]
pub mod fcntl;
#[cfg(feature = "unit-test")]
pub mod mkdir_unlink;
#[cfg(feature = "unit-test")]
pub mod link;
#[cfg(feature = "unit-test")]
pub mod pipe2;
#[cfg(feature = "unit-test")]
pub mod ext4_allocator;
#[cfg(feature = "unit-test")]
pub mod ext4_file_write;

// ===== IPC =====
#[cfg(feature = "unit-test")]
pub mod signal_procmask;
#[cfg(feature = "unit-test")]
pub mod ipc_poll;
#[cfg(feature = "unit-test")]
pub mod ipc_epoll;
#[cfg(feature = "unit-test")]
pub mod ipc_eventfd;

// ===== Memory syscalls =====
#[cfg(feature = "unit-test")]
pub mod mem_mmap;
#[cfg(feature = "unit-test")]
pub mod mem_cow;

// ===== Network =====
#[cfg(feature = "unit-test")]
pub mod tcp_handshake;
#[cfg(feature = "unit-test")]
pub mod virtio_net;
#[cfg(feature = "unit-test")]
pub mod network;

// ===== Drivers =====
#[cfg(feature = "unit-test")]
pub mod virtio_queue;
#[cfg(feature = "unit-test")]
pub mod framebuffer;

// ===== Boundary (destructive) =====
#[cfg(feature = "unit-test")]
pub mod boundary;

// ===== System call interface =====
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
    test_println!("test: ===== Starting Rux OS Unit Tests =====");

    // ===== 1. Pure logic tests =====
    test_group_start("dev_t");
    dev_t::test_dev_t();

    test_group_start("checksum");
    checksum::test_checksum();

    test_group_start("errno");
    errno_test::test_errno();

    test_group_start("config");
    config_test::test_config();

    test_group_start("vma_flags");
    vma_flags::test_vma_flags();

    // ===== 2. Core data structures =====
    test_group_start("listhead");
    listhead::test_listhead();

    test_group_start("path");
    path::test_path();

    test_group_start("file_flags");
    file_flags::test_file_flags();

    test_group_start("fdtable");
    fdtable::test_fdtable();

    test_group_start("signal");
    signal::test_signal();

    // ===== 3. Memory management =====
    test_group_start("heap_allocator");
    heap_allocator::test_heap_allocator();

    test_group_start("page_allocator");
    page_allocator::test_page_allocator();

    test_group_start("buffer_state");
    buffer_state::test_buffer_state();

    test_group_start("mount_flags");
    mount_flags::test_mount_flags();

    // ===== 4. Process management =====
    test_group_start("scheduler");
    scheduler::test_scheduler();

    test_group_start("process_tree");
    process_tree::test_process_tree();

    test_group_start("fork");
    fork::test_fork();

    test_group_start("execve");
    execve::test_execve();

    test_group_start("wait4");
    wait4::test_wait4();

    test_group_start("getpid");
    getpid::test_getpid();

    test_group_start("sleep_wakeup");
    sleep_wakeup::test_sleep_and_wakeup();

    test_group_start("pid");
    pid_test::test_pid();

    // ===== 5. Synchronization =====
    test_group_start("semaphore");
    semaphore::test_semaphore();

    test_group_start("futex");
    futex_test::test_futex();

    // ===== 6. Scheduler =====
    test_group_start("smp");
    smp::test_smp();

    test_group_start("smp_schedule");
    smp_schedule::test_smp_schedule();

    test_group_start("preemptive_scheduler");
    preemptive_scheduler::test_preemptive_scheduler();

    // ===== 7. Filesystem =====
    test_group_start("file_open");
    file_open::test_file_open();

    test_group_start("dcache");
    dcache::test_dcache();

    test_group_start("icache");
    icache::test_icache();

    test_group_start("fstat");
    fstat::test_fstat();

    test_group_start("fcntl");
    fcntl::test_fcntl();

    test_group_start("mkdir_unlink");
    mkdir_unlink::test_mkdir_unlink();

    test_group_start("link");
    link::test_link();

    test_group_start("pipe2");
    pipe2::test_pipe2();

    test_group_start("ext4_allocator");
    ext4_allocator::test_ext4_allocator();

    test_group_start("ext4_file_write");
    ext4_file_write::test_ext4_file_write();

    // ===== 8. IPC =====
    test_group_start("signal_procmask");
    signal_procmask::test_sigprocmask();

    test_group_start("ipc_poll");
    ipc_poll::test_poll();

    test_group_start("ipc_epoll");
    ipc_epoll::test_epoll();

    test_group_start("ipc_eventfd");
    ipc_eventfd::test_eventfd();

    // ===== 9. Memory syscalls =====
    test_group_start("mem_mmap");
    mem_mmap::test_mmap_syscalls();

    test_group_start("mem_cow");
    mem_cow::test_cow();

    // ===== 10. Network =====
    test_group_start("tcp_handshake");
    tcp_handshake::test_tcp_handshake();

    test_group_start("virtio_net");
    virtio_net::test_virtio_net();

    test_group_start("network");
    network::test_network();

    // ===== 11. Drivers =====
    test_group_start("virtio_queue");
    virtio_queue::test_virtio_queue();

    test_group_start("framebuffer");
    framebuffer::test_framebuffer();

    // ===== 12. System call interface =====
    test_group_start("syscall_file");
    syscall_file::test_syscall_file();

    test_group_start("syscall_io");
    syscall_io::test_syscall_io();

    test_group_start("syscall_process");
    syscall_process::test_syscall_process();

    test_group_start("syscall_memory");
    syscall_memory::test_syscall_memory();

    test_group_start("syscall_time");
    syscall_time::test_syscall_time();

    test_group_start("syscall_network");
    syscall_network::test_syscall_network();

    test_group_start("syscall_sched");
    syscall_sched::test_syscall_sched();

    test_group_start("syscall_signal");
    syscall_signal::test_syscall_signal();

    test_group_start("syscall_misc");
    syscall_misc::test_syscall_misc();

    // ===== 13. Boundary (destructive, MUST be last) =====
    test_group_start("boundary");
    boundary::test_boundary();

    // Print test summary
    print_test_summary();
}

/// Print test summary
#[cfg(feature = "unit-test")]
pub fn print_test_summary() {
    let passed = TEST_PASSED.load(Ordering::SeqCst);
    let failed = TEST_FAILED.load(Ordering::SeqCst);
    let total = passed + failed;

    test_println!("\n\u{1b}[36m========================================\u{1b}[0m");
    test_println!("\u{1b}[36m             TEST SUMMARY\u{1b}[0m");
    test_println!("\u{1b}[36m========================================\u{1b}[0m");

    if failed == 0 {
        test_println!("\u{1b}[32m  All tests passed!\u{1b}[0m");
    } else {
        test_println!("\u{1b}[31m  Some tests failed!\u{1b}[0m");
    }

    test_println!();
    test_println!("  Total:   {} tests", total);
    test_println!("  \u{1b}[32mPassed:  {}\u{1b}[0m", passed);
    if failed > 0 {
        test_println!("  \u{1b}[31mFailed:  {}\u{1b}[0m", failed);
    } else {
        test_println!("  Failed:  0");
    }

    // Print failed test list
    if failed > 0 {
        let failed_count = FAILED_COUNT.load(Ordering::SeqCst).min(MAX_FAILED_TESTS);
        let failed_tests = FAILED_TESTS.lock();
        test_println!();
        test_println!("\u{1b}[31m  Failed tests:\u{1b}[0m");
        for i in 0..failed_count {
            test_println!("    \u{1b}[31m{}\u{1b}[0m - {}", failed_tests[i].name(), failed_tests[i].reason());
        }
        if failed > MAX_FAILED_TESTS {
            test_println!("    ... and {} more", failed - MAX_FAILED_TESTS);
        }
    }

    test_println!("\u{1b}[36m========================================\u{1b}[0m");

    // If there are failed tests, print obvious failure marker
    if failed > 0 {
        test_println!("\u{1b}[31m!!! TESTS FAILED !!!\u{1b}[0m");
    } else {
        test_println!("\u{1b}[32m*** ALL TESTS PASSED ***\u{1b}[0m");
    }
}

/// Get failed test count
#[cfg(feature = "unit-test")]
pub fn get_failed_count() -> usize {
    TEST_FAILED.load(Ordering::SeqCst)
}
