// 简单的 RISC-V 用户程序入口点（纯汇编）
//
// 这个程序直接调用 sys_write 系统调用打印 "Hi!\n"，然后退出

.global _start

.section .text
_start:
    // 调用 sys_write(1, "Hi!\n", 4)
    // a0 = 1 (fd = stdout)
    // a1 = message address
    // a2 = 4 (length)
    // a7 = 64 (SYS_WRITE)

    li a0, 1           // fd = stdout
    la a1, message     // message address
    li a2, 4           // length
    li a7, 64          // SYS_WRITE
    ecall

    // 调用 sys_exit(0)
    // a0 = 0 (exit code)
    // a7 = 93 (SYS_EXIT)

    li a0, 0           // exit code
    li a7, 93          // SYS_EXIT
    ecall

    // 如果到达这里，循环
loop:
    wfi
    j loop

.section .rodata
message:
    .string "Hi!\n"
