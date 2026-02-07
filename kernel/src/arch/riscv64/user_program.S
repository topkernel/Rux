//! RISC-V 用户程序测试代码
//!
//! 简单的用户程序，用于测试系统调用和用户模式执行

.section .text.user_program
.global user_program_function
user_program_function:
    // 测试 sys_write
    // a0 = fd (1 = stdout)
    // a1 = message buffer
    // a2 = length
    // a7 = syscall number (64 = sys_write)

    la      a1, user_message       // 加载消息地址
    li      a0, 1                  // fd = stdout
    li      a2, 34                 // 消息长度
    li      a7, 64                 // sys_write = 64
    ecall                          // 系统调用

    // 测试 sys_getpid
    li      a7, 172                // sys_getpid = 172
    ecall                          // 系统调用 (a0 = pid)

    // 测试 sys_exit
    li      a0, 0                  // exit code = 0
    li      a7, 93                 // sys_exit = 93
    ecall                          // 系统调用

    // 不应该到达这里
1:  j       1b

.section .rodata.user_message
user_message:
    .ascii "Hello from user space on RISC-V!\n"
