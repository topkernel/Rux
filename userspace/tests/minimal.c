/*
 * Rux OS - 最小 C 程序测试
 *
 * 不依赖 musl libc，仅测试基本的系统调用
 * 用于验证内核系统调用实现
 */

/* 直接使用内联汇编进行系统调用 */

static inline long syscall1(long n, long a0) {
    register long a7 __asm__("a7") = n;
    register long _a0 __asm__("a0") = a0;
    __asm__ volatile("ecall"
                     : "+r"(_a0)
                     : "r"(a7)
                     : "memory");
    return _a0;
}

static inline long syscall3(long n, long a0, long a1, long a2) {
    register long a7 __asm__("a7") = n;
    register long _a0 __asm__("a0") = a0;
    register long _a1 __asm__("a1") = a1;
    register long _a2 __asm__("a2") = a2;
    __asm__ volatile("ecall"
                     : "+r"(_a0)
                     : "r"(a7), "r"(_a1), "r"(_a2)
                     : "memory");
    return _a0;
}

/* 系统调用号 (RISC-V) */
#define SYS_exit    93
#define SYS_write   64
#define SYS_getpid  172

/* 简单的字符串长度计算 */
static int strlen(const char *s) {
    int len = 0;
    while (s[len]) len++;
    return len;
}

/* 写入标准输出 */
static void print(const char *s) {
    syscall3(SYS_write, 1, (long)s, strlen(s));
}

/* 主函数 */
void _start(void) {
    print("Hello from minimal C program!\n");

    /* 测试 getpid */
    long pid = syscall1(SYS_getpid, 0);
    if (pid > 0) {
        print("getpid() returned: ");
        /* 简单打印 PID (只打印个位数) */
        char buf[3] = "0\n";
        buf[0] = '0' + (pid % 10);
        print(buf);
    }

    print("Test passed!\n");

    /* 退出 */
    syscall1(SYS_exit, 0);

    /* 永不返回 */
    while (1) {}
}
