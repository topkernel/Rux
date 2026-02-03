/* Simple user program - Hello World */
/* This program demonstrates user-space execution */

.section .text
.globl _start
_start:

    /* Make a write syscall (syscall 1) */
    /* x0 = fd (1 for stdout) */
    /* x1 = buffer pointer */
    /* x2 = count */
    /* x8 = syscall number (1 = write) */

    mov x0, #1          /* fd = STDOUT_FILENO */
    ldr x1, =message     /* buffer */
    ldr x2, =len         /* count */
    mov x8, #1           /* syscall number: write */
    svc #0              /* invoke syscall */

    /* Make an exit syscall (syscall 60) */
    /* x0 = exit code */
    /* x8 = syscall number (60 = exit) */

    mov x0, #42         /* exit code */
    mov x8, #60         /* syscall number: exit */
    svc #0              /* invoke syscall */

.section .rodata
message: .ascii "Hello from user space!\n"
len = . - message
