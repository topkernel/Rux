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
pub mod arc_alloc;
#[cfg(feature = "unit-test")]
pub mod quick;

/// 运行所有单元测试
///
/// 这个函数应该在 main() 函数中调用
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

    // 13. execve 系统调用测试
    execve::test_execve();

    // 14. wait4 系统调用测试
    wait4::test_wait4();

    // 15. 边界条件测试
    boundary::test_boundary();

    // 16. SMP 调度验证测试
    smp_schedule::test_smp_schedule();

    // 17. getpid/getppid 系统调用测试
    getpid::test_getpid();

    println!("test: ===== All Unit Tests Completed =====");
}
