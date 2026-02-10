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
pub mod network;

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

    // 12. 边界条件测试（会耗尽任务池，放在其他测试之前）
    boundary::test_boundary();

    // 13. fork 系统调用测试
    fork::test_fork();

    // 14. execve 系统调用测试
    execve::test_execve();

    // 15. wait4 系统调用测试
    wait4::test_wait4();

    // 16. SMP 调度验证测试
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

    // 31. 网络子系统测试
    network::test_network();

    // 32. 标准 alloc crate 类型测试
    // standard_alloc::test_standard_alloc();

    println!("test: ===== All Unit Tests Completed =====");
}
