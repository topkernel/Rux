//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! 单元测试模块
//!
//! 所有单元测试函数都在这个模块中，使用 `unit-test` 特性控制编译。
//!
//! 运行测试：
//! ```bash
//! cargo build --package rux --features riscv64,unit-test
//! qemu-system-riscv64 -M virt -cpu rv64 -m 2G -nographic \
//!   -kernel target/riscv64gc-unknown-none-elf/debug/rux
//! ```

use crate::println;
use core::sync::atomic::{AtomicUsize, Ordering};
use spin::Mutex;

/// 最大记录的失败测试数量
const MAX_FAILED_TESTS: usize = 32;

/// 失败测试记录
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

/// 全局测试统计
#[cfg(feature = "unit-test")]
static TEST_PASSED: AtomicUsize = AtomicUsize::new(0);
#[cfg(feature = "unit-test")]
static TEST_FAILED: AtomicUsize = AtomicUsize::new(0);
#[cfg(feature = "unit-test")]
static TEST_CURRENT: AtomicUsize = AtomicUsize::new(0);

/// 失败测试列表
#[cfg(feature = "unit-test")]
static FAILED_TESTS: Mutex<[FailedTest; MAX_FAILED_TESTS]> = Mutex::new([const { FailedTest::new() }; MAX_FAILED_TESTS]);
#[cfg(feature = "unit-test")]
static FAILED_COUNT: AtomicUsize = AtomicUsize::new(0);

/// 记录测试通过
#[cfg(feature = "unit-test")]
pub fn test_pass(name: &str) {
    TEST_PASSED.fetch_add(1, Ordering::SeqCst);
    println!("test:   \u{1b}[32mPASS\u{1b}[0m {}", name);
}

/// 记录测试失败
#[cfg(feature = "unit-test")]
pub fn test_fail(name: &str, reason: &str) {
    TEST_FAILED.fetch_add(1, Ordering::SeqCst);
    println!("test:   \u{1b}[31mFAIL\u{1b}[0m {} - {}", name, reason);

    // 记录失败的测试
    let idx = FAILED_COUNT.fetch_add(1, Ordering::SeqCst);
    if idx < MAX_FAILED_TESTS {
        let mut failed = FAILED_TESTS.lock();
        failed[idx].set(name, reason);
    }
}

/// 记录测试跳过
#[cfg(feature = "unit-test")]
pub fn test_skip(name: &str, reason: &str) {
    println!("test:   \u{1b}[33mSKIP\u{1b}[0m {} - {}", name, reason);
}

/// 开始一个测试组
#[cfg(feature = "unit-test")]
pub fn test_group_start(name: &str) {
    let idx = TEST_CURRENT.fetch_add(1, Ordering::SeqCst);
    println!("\ntest: [{}] {} ================================", idx + 1, name);
}

/// 断言宏 - 失败时记录但不 panic
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

/// 断言相等宏
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

#[cfg(feature = "unit-test")]
pub fn run_all_tests() {
    println!("test: ===== Starting Rux OS Unit Tests =====");

    // 1. file_open 功能测试
    file_open::test_file_open();

    // 2. ListHead 双向链表测试
    listhead::test_listhead();

    // 3. Path 路径解析测试
    path::test_path();

    // 4. FileFlags 文件标志测试
    file_flags::test_file_flags();

    // 5. FdTable 文件描述符管理测试
    fdtable::test_fdtable();

    // 6. 堆分配器测试
    heap_allocator::test_heap_allocator();

    // 7. 页分配器测试
    page_allocator::test_page_allocator();

    // 8. 调度器测试
    scheduler::test_scheduler();

    // 9. 信号处理测试
    signal::test_signal();

    // 10. SMP 多核启动测试
    smp::test_smp();

    // 11. 进程树管理测试
    process_tree::test_process_tree();

    // 12. fork 系统调用测试
    fork::test_fork();

    // 13. 边界条件测试（会耗尽任务池，放在最后）
    boundary::test_boundary();

    // 14. execve 系统调用测试
    execve::test_execve();

    // 14. wait4 系统调用测试
    wait4::test_wait4();

    // 15. SMP 调度验证测试
    smp_schedule::test_smp_schedule();

    // 17. getpid/getppid 系统调用测试
    getpid::test_getpid();

    // 18. 用户模式系统调用测试
    user_syscall::test_user_syscall();

    // 19. 抢占式调度器测试
    preemptive_scheduler::test_preemptive_scheduler();

    // 20. 进程睡眠和唤醒测试
    sleep_wakeup::test_sleep_and_wakeup();

    // 21. VirtIO 队列测试
    virtio_queue::test_virtio_queue();

    // 22. ext4 分配器测试
    ext4_allocator::test_ext4_allocator();

    // 23. ext4 文件写入测试
    ext4_file_write::test_ext4_file_write();

    // 24. ext4 间接块测试
    ext4_indirect_blocks::test_ext4_indirect_blocks();

    // 25. Dentry 缓存测试
    dcache::test_dcache();

    // 26. Inode 缓存测试
    icache::test_icache();

    // 27. fstat 系统调用测试
    fstat::test_fstat();

    // 28. fcntl 系统调用测试
    fcntl::test_fcntl();

    // 29. mkdir/rmdir/unlink 系统调用测试
    mkdir_unlink::test_mkdir_unlink();

    // 30. link 系统调用测试
    link::test_link();

    // 31. TCP 三次握手测试
    tcp_handshake::test_tcp_handshake();

    // 32. VirtIO-Net 网络设备驱动测试
    virtio_net::test_virtio_net();

    // 33. 网络子系统测试
    network::test_network();

    // 34. pipe2 系统调用测试
    pipe2::test_pipe2();

    // 35. rt_sigprocmask 系统调用测试
    signal_procmask::test_sigprocmask();

    // 36. poll 系统调用测试
    ipc_poll::test_poll();

    // 37. epoll 系统调用测试
    ipc_epoll::test_epoll();

    // 38. eventfd 系统调用测试
    ipc_eventfd::test_eventfd();

    // 39. mmap 系列内存管理系统调用测试
    mem_mmap::test_mmap_syscalls();

    // 40. Copy-on-Write (COW) 测试
    mem_cow::test_cow();

    // 41. 标准 alloc crate 类型测试
    // standard_alloc::test_standard_alloc();

    // 42. Framebuffer 绘制测试
    framebuffer::test_framebuffer();

    // 打印测试摘要
    print_test_summary();
}

/// 打印测试摘要
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

    // 打印失败的测试列表
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

    // 如果有失败的测试，打印明显的失败标记
    if failed > 0 {
        println!("\u{1b}[31m!!! TESTS FAILED !!!\u{1b}[0m");
    } else {
        println!("\u{1b}[32m*** ALL TESTS PASSED ***\u{1b}[0m");
    }
}

/// 获取失败测试数量
#[cfg(feature = "unit-test")]
pub fn get_failed_count() -> usize {
    TEST_FAILED.load(Ordering::SeqCst)
}
