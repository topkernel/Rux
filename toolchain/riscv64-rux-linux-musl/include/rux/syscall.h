#ifndef _RUX_SYSCALL_H
#define _RUX_SYSCALL_H

// RISC-V Linux 系统调用号
#define __NR_set_tid_address    96
#define __NR_set_robust_list    99
#define __NR_gettimeofday      169
#define __NR_clock_gettime     113
#define __NR_uname             160
#define __NR_exit               93
#define __NR_read               63
#define __NR_write              64
#define __NR_openat             56
#define __NR_close              57
#define __NR_brk               214
#define __NR_mmap              222
#define __NR_munmap            215
#define __NR_fork              220
#define __NR_execve            221
#define __NR_wait4             260
#define __NR_getpid            172
#define __NR_getppid           110

#endif /* _RUX_SYSCALL_H */
