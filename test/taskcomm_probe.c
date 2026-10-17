/*
 * taskcomm_probe.c — /proc/<pid>/task/<tid>/* resolution probe for the
 * OpenHarmony boot (hdf_devmgr death).
 *
 * hdf_devmgr (and every process that spawns a thread and then reads
 * /proc/self/task/<tid>/comm) dies with exit(1) on our kernel:
 *
 *     fork: parent=329, child=361          (CLONE_THREAD child spawned)
 *     open ENOENT: '/proc/self/task/361/comm' (pid 329)
 *     exit: pid=329 ... exit_code=256      (devmgr exits 1 -> HDF chain dies)
 *
 * Linux serves /proc/<tgid>/task/<tid>/{comm,status,...} for every live
 * thread of the group, including the leader (task/<tgid> == the group
 * leader's dir). This probe walks the exact lookup chain component by
 * component so the failing hop is named, then exercises the read path.
 *
 * Cases:
 *   T1  open /proc                     (procfs up)
 *   T2  open /proc/self                (self alias)
 *   T3  open /proc/self/task           (task dir)
 *   T4  getdents /proc/self/task       (>=1 entry: the leader)
 *   T5  open /proc/self/task/<tid-self>/comm   (leader via self alias)
 *   T6  clone(CLONE_THREAD) -> tid T
 *       open /proc/self/task/<T>/comm  (THE hdf_devmgr death path)
 *   T7  open /proc/<pid>/task/<T>/comm (numeric tgid path)
 *   T8  openat chain                   (dirfd-per-component walk)
 *   T9  read the comm of T             (contents == prctl name)
 *
 * Prints one ok/FAIL line per case and a final verdict:
 *     TASKCOMM RESULT: PASS <n>/<n>
 *
 * Build (static musl, toolchain/x86_64-rux-linux-musl):
 *   gcc -static -nostdlib -O2 -I toolchain/x86_64-rux-linux-musl/include \
 *       -o taskcomm_probe taskcomm_probe.c \
 *       toolchain/x86_64-rux-linux-musl/lib/crt1.o \
 *       toolchain/x86_64-rux-linux-musl/lib/libc.a -lgcc
 *
 * Runs as init (init=/test/taskcomm) on a tiny ext4 rootfs, q35 console.
 */
#define _GNU_SOURCE
#include <fcntl.h>
#include <stdint.h>
#include <stddef.h>
#include <stdio.h>
#include <string.h>
#include <errno.h>
#include <time.h>
#include <unistd.h>
#include <sys/mman.h>
#include <sys/mount.h>
#include <sys/prctl.h>
#include <sys/signal.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <sys/types.h>
#include <sched.h>
#include <signal.h>

/* ---- raw console output (write(1)) ---- */
static int puts_(const char *s) { return write(1, s, strlen(s)); }
static void putu_(unsigned long v) {
    char b[24]; int i = 24;
    if (!v) { puts_("0"); return; }
    while (v && i) { b[--i] = '0' + (v % 10); v /= 10; }
    write(1, b + i, 24 - i);
}
static void putsdec_(long v) {
    if (v < 0) { puts_("-"); putu_((unsigned long)(-v)); }
    else putu_((unsigned long)v);
}

static int g_pass, g_total;
static void report(int ok, const char *name, long detail) {
    g_total++;
    if (ok) { g_pass++; puts_("ok   "); puts_(name); puts_("\n"); }
    else {
        puts_("FAIL "); puts_(name);
        puts_(" errno="); putsdec_(detail); puts_("\n");
    }
}

/* ---- struct for getdents64 (kernel layout, x86_64) ---- */
struct kdent64 {
    uint64_t d_ino;
    int64_t  d_off;
    uint16_t d_reclen;
    uint8_t  d_type;
    char     d_name[];
};

static char dents_buf[8192];

/* count numeric entries of a directory fd (0 on error, -errno via *err) */
static int count_numeric_entries(int fd, int *err) {
    int n = 0;
    *err = 0;
    for (;;) {
        long nread = syscall(SYS_getdents64, fd, dents_buf, sizeof(dents_buf));
        if (nread < 0) { *err = (int)-nread; return -1; }
        if (nread == 0) break;
        for (long off = 0; off < nread;) {
            struct kdent64 *d = (struct kdent64 *)(dents_buf + off);
            if (d->d_name[0] >= '0' && d->d_name[0] <= '9') n++;
            off += d->d_reclen;
        }
    }
    return n;
}

static volatile long g_child_tid = -1;

/* musl's public clone() rejects CLONE_THREAD with EINVAL by design
 * (src/linux/clone.c badflags); pthread_create uses this hidden
 * internal entry with the full (ptid, tls, ctid) tail. */
extern int __clone(int (*func)(void *), void *stack, int flags, void *arg, ...);

static int child_fn(void *arg) {
    (void)arg;
    /* Name ourselves so T9 can compare; park forever (exit_group in the
     * parent reaps the group). */
    prctl(PR_SET_NAME, (unsigned long)"probethrd", 0, 0, 0);
    g_child_tid = syscall(SYS_gettid);
    for (;;) syscall(SYS_pause);
    return 0;
}

int main(void) {
    mkdir("/proc", 0755);
    if (mount("proc", "/proc", "proc", 0, "")) {
        /* already mounted? then open test will tell */
        puts_("note: mount proc errno="); putsdec_(errno); puts_("\n");
    }

    long pid = syscall(SYS_getpid);
    long tid = syscall(SYS_gettid);
    puts_("taskcomm_probe pid="); putu_(pid);
    puts_(" tid=");             putu_(tid);
    puts_("\n");

    /* T1: /proc up */
    int fd = open("/proc", O_RDONLY | O_DIRECTORY);
    report(fd >= 0, "T1 open /proc", fd >= 0 ? 0 : errno);
    if (fd >= 0) close(fd);

    /* T2: /proc/self */
    fd = open("/proc/self", O_RDONLY | O_DIRECTORY);
    report(fd >= 0, "T2 open /proc/self", fd >= 0 ? 0 : errno);
    int self_dir = fd;  /* keep for T8 */

    /* T3: /proc/self/task */
    fd = open("/proc/self/task", O_RDONLY | O_DIRECTORY);
    report(fd >= 0, "T3 open /proc/self/task", fd >= 0 ? 0 : errno);
    int task_dir = fd;  /* keep for T8 */

    /* T4: readdir sees at least the leader */
    if (task_dir >= 0) {
        int err = 0;
        int n = count_numeric_entries(dup(task_dir), &err);
        report(n >= 1, "T4 getdents /proc/self/task >=1 tid", n >= 1 ? 0 : (err ? err : -n));
        close(task_dir);
    } else {
        report(0, "T4 getdents /proc/self/task >=1 tid", -1);
    }

    /* T5: leader's comm through the self alias */
    char path[128];
    char comm[64];
    snprintf(path, sizeof(path), "/proc/self/task/%ld/comm", pid);
    fd = open(path, O_RDONLY);
    report(fd >= 0, "T5 open /proc/self/task/<tgid>/comm", fd >= 0 ? 0 : errno);
    if (fd >= 0) {
        long r = read(fd, comm, sizeof(comm) - 1);
        if (r > 0) comm[r] = 0; else comm[0] = 0;
        close(fd);
    }

    /* spawn a real CLONE_THREAD child with its own stack */
    const size_t stk_sz = 8 * 4096;
    char *stk = mmap(NULL, stk_sz, PROT_READ | PROT_WRITE,
                     MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (stk == MAP_FAILED) {
        puts_("FATAL mmap stack errno="); putsdec_(errno); puts_("\n");
        puts_("TASKCOMM RESULT: FAIL 0/"); putu_(g_total); puts_("\n");
        return 1;
    }
    long ctid = __clone(child_fn, stk + stk_sz,
                      CLONE_VM | CLONE_FS | CLONE_FILES | CLONE_SIGHAND |
                      CLONE_THREAD | CLONE_SYSVSEM,
                      NULL, NULL, NULL, NULL);
    if (ctid < 0) {
        puts_("FATAL clone errno="); putsdec_(errno); puts_("\n");
        puts_("TASKCOMM RESULT: FAIL 0/"); putu_(g_total); puts_("\n");
        return 1;
    }
    /* wait for the child to publish its tid */
    for (int i = 0; i < 1000 && g_child_tid < 0; i++) {
        struct timespec ts = { 0, 2000000 };
        nanosleep(&ts, NULL);
    }
    long ttid = g_child_tid;
    puts_("child tid="); putu_(ttid); puts_("\n");

    /* T6: THE hdf_devmgr death path */
    snprintf(path, sizeof(path), "/proc/self/task/%ld/comm", ttid);
    fd = open(path, O_RDONLY);
    report(fd >= 0, "T6 open /proc/self/task/<tid>/comm (devmgr path)", fd >= 0 ? 0 : errno);

    /* T9: read the child's comm and compare with the prctl name */
    if (fd >= 0) {
        long r = read(fd, comm, sizeof(comm) - 1);
        if (r > 0) comm[r] = 0; else comm[0] = 0;
        close(fd);
        report(strncmp(comm, "probethrd", 9) == 0, "T9 read comm == probethrd", 0);
    } else {
        report(0, "T9 read comm == probethrd", errno);
    }

    /* T7: numeric tgid path */
    snprintf(path, sizeof(path), "/proc/%ld/task/%ld/comm", pid, ttid);
    fd = open(path, O_RDONLY);
    report(fd >= 0, "T7 open /proc/<tgid>/task/<tid>/comm", fd >= 0 ? 0 : errno);
    if (fd >= 0) close(fd);

    /* T8: openat chain from /proc/self — name the failing hop */
    {
        int hop1 = self_dir;  /* already open */
        /* What does the kernel think this fd's path is? (readlink shows
         * the string the dirfd branch would join onto.) */
        char lk[128], lpath[96];
        snprintf(lpath, sizeof(lpath), "/proc/self/fd/%d", hop1);
        long lr = readlink(lpath, lk, sizeof(lk) - 1);
        puts_("     fdlink of hop1: ");
        if (lr > 0) { lk[lr] = 0; puts_(lk); } else puts_("(readlink failed)");
        puts_("\n");
        errno = 0;
        int hop2 = hop1 >= 0 ? syscall(SYS_openat, hop1, "task", O_RDONLY | O_DIRECTORY, 0) : -1;
        int e2 = errno;
        char tidstr[24];
        snprintf(tidstr, sizeof(tidstr), "%ld", ttid);
        errno = 0;
        int hop3 = hop2 >= 0 ? syscall(SYS_openat, hop2, tidstr, O_RDONLY | O_DIRECTORY, 0) : -1;
        int e3 = errno;
        errno = 0;
        int hop4 = hop3 >= 0 ? syscall(SYS_openat, hop3, "comm", O_RDONLY, 0) : -1;
        int e4 = errno;
        report(hop4 >= 0, "T8 openat self->task->tid->comm chain", 0);
        puts_("     chain: self=");
        putsdec_(hop1); puts_("/");
        putsdec_(e2); puts_(" task=");
        putsdec_(hop2); puts_("/");
        putsdec_(e2 ? e2 : 0); puts_(" tid=");
        putsdec_(hop3); puts_("/");
        putsdec_(e3); puts_(" comm=");
        putsdec_(hop4); puts_("/");
        putsdec_(e4); puts_(" (fd/errno)\n");
        if (hop4 >= 0) close(hop4);
        if (hop3 >= 0) close(hop3);
        if (hop2 >= 0) close(hop2);
        if (hop1 >= 0) close(hop1);
    }

    puts_("TASKCOMM RESULT: ");
    if (g_pass == g_total) { puts_("PASS "); putu_(g_pass); puts_("/"); putu_(g_total); puts_("\n"); }
    else { putu_(g_pass); puts_("/"); putu_(g_total); puts_("\n"); }
    return g_pass == g_total ? 0 : 1;
}
