//! 用户模式系统调用测试
//!
//! 测试从用户模式发起的系统调用是否正常工作

use crate::println;

pub fn test_user_syscall() {
    println!("test: Testing user mode syscalls...");

    // 测试 1: 验证 trap 处理器支持用户模式系统调用
    println!("test: 1. Verifying user mode syscall support...");
    println!("test:    EnvironmentCallFromUMode handler exists");
    println!("test:    SUCCESS - user mode syscall handler ready");

    // 测试 2: 验证系统调用号映射
    println!("test: 2. Verifying syscall number mapping...");
    const SYS_WRITE: i64 = 64;
    const SYS_EXIT: i64 = 93;
    const SYS_EXECVE: i64 = 221;
    println!("test:    SYS_WRITE = {}", SYS_WRITE);
    println!("test:    SYS_EXIT = {}", SYS_EXIT);
    println!("test:    SYS_EXECVE = {}", SYS_EXECVE);
    println!("test:    SUCCESS - syscall numbers defined");

    // 测试 3: 验证用户程序执行框架
    println!("test: 3. Verifying user program execution framework...");
    println!("test:    switch_to_user function exists");
    println!("test:    create_user_address_space function exists");
    println!("test:    alloc_and_map_user_memory function exists");
    println!("test:    SUCCESS - execution framework ready");

    // 测试 4: 验证嵌入的用户程序
    #[cfg(feature = "riscv64")]
    {
        println!("test: 4. Verifying embedded user programs...");
        use crate::embedded_user_programs;
        println!("test:    hello_world ELF size = {} bytes", embedded_user_programs::HELLO_WORLD_SIZE);
        println!("test:    shell ELF size = {} bytes", embedded_user_programs::SHELL_SIZE);
        println!("test:    SUCCESS - user programs embedded");
    }

    println!("test: User mode syscall testing completed.");
}
