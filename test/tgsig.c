/*
 * tgsig.c — rt_tgsigqueueinfo parity probe (hilogd EFAULT hunt).
 *
 * The kernel used to parse 3 args (tgid, tid, uinfo=args[2]); args[2]
 * is the signal number, so every call returned EFAULT and hilogd's
 * DFX stack-dump path died (hilogd.server SIGSEGV at 0x18 right after
 * `EFAULTTRAC: nr=240`). This probe verifies the 4-arg (tgid, tid,
 * sig, uinfo) Linux contract end to end:
 *
 *   1. self-delivery with si_code=SI_QUEUE(-1): syscall returns 0 and
 *      an installed SA_SIGINFO handler receives the signal with the
 *      exact sival_ptr payload;
 *   2. sig==0: returns 0, no delivery;
 *   3. si_code==0 (SI_USER, >= 0): EPERM (kernel-source forging);
 *   4. wrong tid: ESRCH;
 *   5. bad uinfo: EFAULT;
 *   6. out-of-range sig: EINVAL.
 *
 * Final line: TGSIG RESULT: PASS|FAIL (rc 0/1).
 */
#include <stdint.h>
#include <stddef.h>
#include <string.h>
#include <errno.h>
#include <unistd.h>
#include <signal.h>
#include <sys/syscall.h>

static int puts_(const char *s) { return write(1, s, strlen(s)); }
static void putu_(unsigned long v) {
    char b[24]; int i = 24;
    if (!v) { puts_("0"); return; }
    while (v && i) { b[--i] = '0' + (v % 10); v /= 10; }
    write(1, b + i, 24 - i);
}

static volatile sig_atomic_t got_signo;
static volatile long long got_val;

static void handler(int sig, siginfo_t *si, void *ctx) {
    (void)ctx;
    got_signo = sig;
    got_val = (long long)(intptr_t)si->si_value.sival_ptr;
}

int main(void) {
    int fail = 0;
    long r;
    pid_t pid = getpid();

    struct sigaction sa;
    memset(&sa, 0, sizeof(sa));
    sa.sa_sigaction = handler;
    sa.sa_flags = SA_SIGINFO | SA_RESTART;
    sigemptyset(&sa.sa_mask);
    sigaction(SIGUSR1, &sa, 0);

    /* 1. self-delivery, SI_QUEUE payload round-trip */
    siginfo_t si;
    memset(&si, 0, sizeof(si));
    si.si_signo = SIGUSR1;
    si.si_code = -1;                 /* SI_QUEUE */
    si.si_value.sival_ptr = (void *)(intptr_t)0x5a5a5a5aULL;
    errno = 0;
    r = syscall(SYS_rt_tgsigqueueinfo, pid, pid, SIGUSR1, &si);
    if (r != 0) {
        puts_("TGSIG 1 self-delivery FAILED ret=");
        putu_(r < 0 ? (unsigned long)-r : (unsigned long)r);
        puts_(" errno=");
        putu_(errno);
        puts_("\n");
        fail = 1;
    } else {
        for (volatile int i = 0; i < 20000000 && !got_signo; i++) {}
        if (got_signo != SIGUSR1 || got_val != 0x5a5a5a5aLL) {
            puts_("TGSIG 1 payload FAILED: handler did not run or value wrong\n");
            fail = 1;
        } else {
            puts_("TGSIG 1 self-delivery OK (sival round-trip)\n");
        }
    }

    /* 2. sig == 0: validate only */
    errno = 0;
    r = syscall(SYS_rt_tgsigqueueinfo, pid, pid, 0, &si);
    puts_("TGSIG 2 sig0 ret="); putu_(r < 0 ? (unsigned long)-r : 0);
    puts_(r == 0 ? " OK\n" : " FAILED\n");
    if (r != 0) fail = 1;

    /* 3. si_code >= 0: EPERM */
    siginfo_t bad;
    memset(&bad, 0, sizeof(bad));
    bad.si_signo = SIGUSR1;
    bad.si_code = 0;                 /* SI_USER */
    errno = 0;
    r = syscall(SYS_rt_tgsigqueueinfo, pid, pid, SIGUSR1, &bad);
    puts_("TGSIG 3 si_code0 errno=");
    putu_(errno);
    puts_((r == -1 && errno == EPERM) ? " OK\n" : " FAILED\n");
    if (!(r == -1 && errno == EPERM)) fail = 1;

    /* 4. wrong tid: ESRCH */
    errno = 0;
    r = syscall(SYS_rt_tgsigqueueinfo, pid, 999999, SIGUSR1, &si);
    puts_("TGSIG 4 badtid errno=");
    putu_(errno);
    puts_((r == -1 && errno == ESRCH) ? " OK\n" : " FAILED\n");
    if (!(r == -1 && errno == ESRCH)) fail = 1;

    /* 5. bad uinfo: EFAULT */
    errno = 0;
    r = syscall(SYS_rt_tgsigqueueinfo, pid, pid, SIGUSR1, (void *)16);
    puts_("TGSIG 5 badinfo errno=");
    putu_(errno);
    puts_((r == -1 && errno == EFAULT) ? " OK\n" : " FAILED\n");
    if (!(r == -1 && errno == EFAULT)) fail = 1;

    /* 6. out-of-range sig: EINVAL */
    errno = 0;
    r = syscall(SYS_rt_tgsigqueueinfo, pid, pid, 99, &si);
    puts_("TGSIG 6 sig99 errno=");
    putu_(errno);
    puts_((r == -1 && errno == EINVAL) ? " OK\n" : " FAILED\n");
    if (!(r == -1 && errno == EINVAL)) fail = 1;

    puts_(fail ? "TGSIG RESULT: FAIL\n" : "TGSIG RESULT: PASS\n");
    return fail;
}
