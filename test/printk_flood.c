/* printk_flood — freestanding printk wedge stress probe (printk-wedge lane).
 *
 * Reproduces the OH b140 kernel-printk flood shape from userspace: every
 * mremap(MREMAP_FIXED) emits an ungated MREMAP-CFI pr_info line
 * (kernel/src/syscall/memory.rs), the exact line family that was
 * mid-flight ("MREMAP-CFI pid 487 ... len ") when the machine wedged:
 * CPU0 stuck inside printk::emit_to_console with IF=0, CPU1 holding the
 * console emit lock mid-line, serial frozen.
 *
 * Raw syscalls only (no libc — the static glibc objects carry rv64+V
 * vector code the boot CPU cannot execute). Run as init:
 *   init=/test/printk_flood
 * Forks:
 *   - 2 mremap flooders   (kernel printk flood, task context, per CPU)
 *   - 1 console writer    (console::putchar path pressure via /dev/console)
 * runs for FLOOD_SECS, prints FLOOD-DONE markers, then idles forever
 * (the driver kills QEMU at its own timeout; init must not exit).
 *
 * Healthy result: all children report DONE with iters>0, no "BUG: soft
 * lockup" in the serial log, serial still flowing at driver timeout.
 */
typedef unsigned long u64;
typedef long s64;

/* RISC-V (asm-generic) syscall numbers. */
#define __NR_write 64
#define __NR_openat 56
#define __NR_close 57
#define __NR_exit 93
#define __NR_exit_group 94
#define __NR_nanosleep 101
#define __NR_clock_gettime 113
#define __NR_getpid 172
#define __NR_clone 220
#define __NR_mmap 222
#define __NR_mremap 216
#define __NR_munmap 215
#define __NR_mprotect 226
#define __NR_wait4 260

#define AT_FDCWD (-100L)
#define O_WRONLY 1

#define PROT_NONE 0
#define PROT_READ 1
#define PROT_WRITE 2
#define MAP_PRIVATE 0x02
#define MAP_ANONYMOUS 0x20
#define MAP_NORESERVE 0x4000
#define MREMAP_MAYMOVE 1
#define MREMAP_FIXED 2
#define SIGCHLD 17
#define CLOCK_MONOTONIC 1

static s64 sys3(s64 n, s64 a, s64 b, s64 c)
{
    register s64 a0 asm("a0") = a;
    register s64 a1 asm("a1") = b;
    register s64 a2 asm("a2") = c;
    register s64 a7 asm("a7") = n;
    asm volatile("ecall" : "+r"(a0) : "r"(a1), "r"(a2), "r"(a7) : "memory");
    return a0;
}

static s64 sys4(s64 n, s64 a, s64 b, s64 c, s64 d)
{
    register s64 a0 asm("a0") = a;
    register s64 a1 asm("a1") = b;
    register s64 a2 asm("a2") = c;
    register s64 a3 asm("a3") = d;
    register s64 a7 asm("a7") = n;
    asm volatile("ecall"
                 : "+r"(a0)
                 : "r"(a1), "r"(a2), "r"(a3), "r"(a7)
                 : "memory");
    return a0;
}

static s64 sys5(s64 n, s64 a, s64 b, s64 c, s64 d, s64 e)
{
    register s64 a0 asm("a0") = a;
    register s64 a1 asm("a1") = b;
    register s64 a2 asm("a2") = c;
    register s64 a3 asm("a3") = d;
    register s64 a4 asm("a4") = e;
    register s64 a7 asm("a7") = n;
    asm volatile("ecall"
                 : "+r"(a0)
                 : "r"(a1), "r"(a2), "r"(a3), "r"(a4), "r"(a7)
                 : "memory");
    return a0;
}

static s64 sys6(s64 n, s64 a, s64 b, s64 c, s64 d, s64 e, s64 f)
{
    register s64 a0 asm("a0") = a;
    register s64 a1 asm("a1") = b;
    register s64 a2 asm("a2") = c;
    register s64 a3 asm("a3") = d;
    register s64 a4 asm("a4") = e;
    register s64 a5 asm("a5") = f;
    register s64 a7 asm("a7") = n;
    asm volatile("ecall"
                 : "+r"(a0)
                 : "r"(a1), "r"(a2), "r"(a3), "r"(a4), "r"(a5), "r"(a7)
                 : "memory");
    return a0;
}

static void puts_(const char *s)
{
    int len = 0;
    while (s[len])
        len++;
    sys3(__NR_write, 1, (s64)s, len);
}

static void putdec_(unsigned long v)
{
    char tmp[24];
    int n = 0, i;
    do {
        tmp[n++] = '0' + (v % 10);
        v /= 10;
    } while (v);
    char buf[24];
    for (i = 0; n; n--, i++)
        buf[i] = tmp[n - 1];
    buf[i] = '\n';
    sys3(__NR_write, 1, (s64)buf, i + 1);
}

/* Monotonic seconds; -1 if clock_gettime is unsupported (callers fall
 * back to iteration caps). */
static s64 now_s(void)
{
    u64 ts[2];
    if (sys3(__NR_clock_gettime, CLOCK_MONOTONIC, (s64)ts, 0) != 0)
        return -1;
    return (s64)ts[0];
}

#define RESERVE_SZ 0x80000000UL /* 2GB CFI-shadow reserve, like OH musl */
#define PAGE 4096UL
/* 600 s of guest time is ~750+ s of host wall time under TCG on a
 * loaded host (guest clock lags), which overruns the suite's probe
 * budget; the suite wrapper trims it with -DFLOOD_SECS=240. */
#ifndef FLOOD_SECS
#define FLOOD_SECS 600
#endif

/* OH musl CFI-shadow fill shape, stripped to its syscall skeleton: each
 * iteration performs exactly one mremap(MREMAP_FIXED) onto the PROT_NONE
 * reserve = one MREMAP-CFI kernel printk line. */
static void mremap_flooder(int id)
{
    u64 reserve = (u64)sys6(__NR_mmap, 0, RESERVE_SZ, PROT_NONE,
                            MAP_PRIVATE | MAP_ANONYMOUS | MAP_NORESERVE,
                            -1, 0);
    if ((s64)reserve < 0 && (s64)reserve > -4096) {
        puts_("[flood] reserve mmap FAILED errno=\n");
        sys3(__NR_exit, 1, 0, 0);
    }

    s64 t0 = now_s();
    u64 iters = 0, fails = 0, off = 0;

    for (;;) {
        u64 len = 0x2000 + (iters & 0x3f) * PAGE;
        u64 dst = (reserve + ((off += 0x10000) & (RESERVE_SZ / 2 - 1)))
                  & ~(PAGE - 1);

        s64 tmp = sys6(__NR_mmap, 0, len, PROT_READ | PROT_WRITE,
                       MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        if (tmp < 0 && tmp > -4096) {
            fails++;
            continue;
        }
        if (sys3(__NR_mprotect, tmp, (s64)len, PROT_READ) != 0)
            fails++;
        s64 r = sys5(__NR_mremap, tmp, (s64)len, (s64)len,
                     MREMAP_MAYMOVE | MREMAP_FIXED, (s64)dst);
        if (r < 0 && r > -4096)
            fails++;

        iters++;
        s64 t = now_s();
        if (t0 >= 0) {
            if (t - t0 >= FLOOD_SECS)
                break;
        } else if (iters >= 400000) {
            break; /* no clock: iteration-bounded fallback */
        }
    }

    puts_(id == 0 ? "[flood0] DONE iters=" : "[flood1] DONE iters=");
    putdec_(iters);
    puts_(" fails=");
    putdec_(fails);
    sys3(__NR_exit, 0, 0, 0);
}

/* Direct console pressure through the OTHER TX path: userspace writes to
 * /dev/console go through the tty/console driver and console::putchar
 * (UART spinlock held across the byte loop — also now bounded). */
static void console_flooder(void)
{
    s64 fd = sys3(__NR_openat, AT_FDCWD, (s64)"/dev/console", O_WRONLY);
    if (fd < 0)
        fd = sys3(__NR_openat, AT_FDCWD, (s64)"/dev/ttyS0", O_WRONLY);
    if (fd < 0) {
        puts_("[conflood] no console device, skipping\n");
        sys3(__NR_exit, 0, 0, 0);
    }
    s64 t0 = now_s();
    u64 lines = 0;
    /* Initialized (not .bss): keeps the RW PT_LOAD's p_filesz > 0 — the
     * kernel's ELF loader bounds-checks p_offset+p_filesz even for
     * filesz==0 segments and a bare .bss segment lands past EOF. */
    static char buf[96] = "CONSOLE-FLOOD line=0 0123456789abcdef\n";
    for (;;) {
        /* fixed-width line: replace a decimal counter in-place */
        const char *pre = "CONSOLE-FLOOD line=";
        int i = 0, k = 0;
        while (pre[k])
            buf[i++] = pre[k++];
        u64 v = lines;
        char tmp[24];
        int n = 0;
        do {
            tmp[n++] = '0' + (v % 10);
            v /= 10;
        } while (v);
        while (n)
            buf[i++] = tmp[--n];
        const char *post = " 0123456789abcdef\n";
        k = 0;
        while (post[k])
            buf[i++] = post[k++];
        if (sys3(__NR_write, fd, (s64)buf, i) < 0)
            break;
        lines++;
        s64 t = now_s();
        if (t0 >= 0) {
            if (t - t0 >= FLOOD_SECS)
                break;
        } else if (lines >= 200000) {
            break;
        }
    }
    puts_("[conflood] DONE lines=");
    putdec_(lines);
    sys3(__NR_exit, 0, 0, 0);
}

static s64 fork_(void)
{
    /* clone(SIGCHLD, 0, 0, 0, 0) = classic fork. */
    return sys5(__NR_clone, SIGCHLD, 0, 0, 0, 0);
}

void _start(void)
{
    /* crt0's job, skipped by -nostartfiles: without gp the linker's
     * gp-relative relaxation addresses static data as gp+offset = garbage
     * (console_flooder's buf faulted at 0xfffffffffffff813 = -2029+0). */
    __asm__ volatile(
        ".option push\n"
        ".option norelax\n"
        "la gp, __global_pointer$\n"
        ".option pop\n"
        ::: "memory");

    puts_("printk_flood: starting\n");

    int nchildren = 0;
    for (int i = 0; i < 2; i++) {
        s64 p = fork_();
        if (p == 0)
            mremap_flooder(i);
        if (p > 0)
            nchildren++;
    }
    {
        s64 p = fork_();
        if (p == 0)
            console_flooder();
        if (p > 0)
            nchildren++;
    }
    puts_("printk_flood: forked children=");
    putdec_((unsigned long)nchildren);

    int fails = 0;
    for (int i = 0; i < nchildren; i++) {
        s64 st = 0;
        if (sys4(__NR_wait4, -1, (s64)&st, 0, 0) < 0)
            fails++;
        else if ((st & 0xff) != 0)
            fails++; /* exited nonzero or signaled */
    }

    if (fails == 0)
        puts_("FLOOD-DONE OK\n");
    else
        puts_("FLOOD-DONE FAIL\n");

    /* Idle forever: init exiting powers the machine down; the driver
     * kills QEMU at its own timeout while the serial keeps flowing. */
    u64 ts[2];
    for (;;) {
        ts[0] = 5;
        ts[1] = 0;
        sys3(__NR_nanosleep, (s64)ts, 0, 0);
    }
}
