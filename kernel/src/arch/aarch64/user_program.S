// User program assembly
// This file contains the user program that makes system calls

.section .text.user_program
.global user_program_function
.user_program_function:
//.align 4

    // Load address of the message (PC-relative)
    adrp    x1, message
    add     x1, x1, :lo12:message

    // Setup for write syscall:
    // x0 = fd (1 = stdout)
    // x1 = buffer address (loaded above)
    // x2 = buffer size
    // x8 = syscall number
    mov     x0, #1              // fd = stdout
    mov     x2, #23             // buffer size
    mov     x8, #1              // syscall number for write
    svc     #0                  // invoke system call

    // Exit after printing
    mov     x0, #0              // exit code 0
    mov     x8, #60             // syscall number for exit
    svc     #0                  // invoke system call

    // Infinite loop (if exit returns, which it shouldn't)
1:
    b       1b

.section .rodata.user_message
.align 4
message:
    .ascii  "Hello from user space!\n"
