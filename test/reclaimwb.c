// reclaimwb.c — reclaim-time dirty writeback probe (wildwrite lane
// session 28; the shrink_reclaim v2 validation).
//
// Scenario: a large writable MAP_SHARED mapping stores into hole pages
// and is abandoned WITHOUT msync/fsync — every page of it is a dirty,
// unreferenced cache entry that plain shrink() must skip (evicting it
// would lose the only copy). A sacrificial child then eats the rest of
// RAM with anonymous mappings, driving alloc_pages into direct reclaim.
//
//   RWB-DIRTY-SET        the dirty region is faulted, stored, unmapped
//   RWB-PRESSURE-COMPLETE  the child touched its full anon target — the
//                          dirty frames were flushed by reclaim and the
//                          pages became evictable (the v2 signature)
//   RWB-PRESSURE-OOMKILL   the child hit the wall and was OOM-killed —
//                          the PRE-fix signature (dirty stuck forever);
//                          the boot itself must still survive cleanly
//   RWB-DONE             the parent finished; kernel never panicked
//
// Build defines (from the driver, per platform):
//   RWB_DIRTY_MB  size of the abandoned dirty shared file
//   RWB_ANON_MB   the child's anonymous pressure target
//
// Children print via raw write(2) only (faultaround probe lesson).

#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/wait.h>
#include <unistd.h>

#define PAGE 4096U

#ifndef RWB_DIRTY_MB
#define RWB_DIRTY_MB 256
#endif
#ifndef RWB_ANON_MB
#define RWB_ANON_MB 1400
#endif

#define DIRTY_BYTES ((size_t)RWB_DIRTY_MB * 1024 * 1024)
#define ANON_BYTES ((size_t)RWB_ANON_MB * 1024 * 1024)
#define DIRTY_PAGES (DIRTY_BYTES / PAGE)
#define ANON_PAGES (ANON_BYTES / PAGE)
/* One stored word per stride keeps the page dirty (dirty tracking is
 * page-granular) while staying fast under TCG; the same stride would be
 * verified on a cold-boot read if a persist leg is ever wanted. */
#define WORD_STRIDE 64

static void raw_write(const char *s) {
    write(1, s, strlen(s));
}

int main(void) {
    printf("reclaimwb: pid=%d dirty=%dMB anon=%dMB\n",
           getpid(), RWB_DIRTY_MB, RWB_ANON_MB);

    // ---- make the abandoned dirty region ----
    unlink("/reclaimwb.dat");
    int fd = open("/reclaimwb.dat", O_RDWR | O_CREAT | O_TRUNC, 0644);
    if (fd < 0) { printf("RWB-FAIL create\n"); return 1; }
    if (ftruncate(fd, (off_t)DIRTY_BYTES) != 0) {
        printf("RWB-FAIL ftruncate\n");
        return 1;
    }
    uint32_t *p = mmap(NULL, DIRTY_BYTES, PROT_READ | PROT_WRITE,
                       MAP_SHARED, fd, 0);
    if (p == MAP_FAILED) { printf("RWB-FAIL mmap\n"); return 1; }
    for (uint32_t pg = 0; pg < DIRTY_PAGES; pg++) {
        uint32_t *base = p + (size_t)pg * (PAGE / sizeof(uint32_t));
        for (uint32_t i = 0; i < PAGE / sizeof(uint32_t); i += WORD_STRIDE)
            base[i] = 0x3C3C0000u | pg;
    }
    printf("RWB-DIRTY-SET\n");
    // Abandon with NO msync/fsync: every page is now a dirty,
    // unreferenced cache entry (pin dropped at munmap).
    munmap(p, DIRTY_BYTES);
    close(fd);

    // ---- sacrificial pressure child ----
    // The child touches anonymous memory until it completes the target
    // or dies at the wall (pagefault-OOM kill — survivable by design).
    // Progress markers every PROGRESS_MB let the driver read HOW FAR it
    // got: with the dirty region flushed by reclaim (v2) the child
    // reaches ~zone-ceiling MB; with the dirty frames stuck (v1) it
    // stops RWB_DIRTY_MB earlier. The reached-amount delta across an
    // A/B of the two kernels is the discrimination — no knife-edge RAM
    // arithmetic required.
    pid_t pid = fork();
    if (pid == 0) {
        char *a = mmap(NULL, ANON_BYTES, PROT_READ | PROT_WRITE,
                       MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        if (a == MAP_FAILED) { raw_write("RWB child: anon mmap failed\n"); _exit(3); }
        size_t done_mb = 0;
        for (size_t pg = 0; pg < ANON_PAGES; pg++) {
            *(volatile char *)(a + pg * PAGE) = (char)pg;
            size_t mb = (pg + 1) / (1024 * 1024 / PAGE);
            if (mb / 64 > done_mb / 64) {
                done_mb = mb;
                char msg[48];
                int n = snprintf(msg, sizeof msg, "RWB child: %zuMB\n", mb);
                write(1, msg, n);
            }
        }
        raw_write("RWB child: anon target complete\n");
        _exit(0);
    }
    int st = 0;
    waitpid(pid, &st, 0);
    if (WIFEXITED(st) && WEXITSTATUS(st) == 0)
        printf("RWB-PRESSURE-COMPLETE\n");
    else if (WIFSIGNALED(st))
        printf("RWB-PRESSURE-OOMKILL sig=%d\n", WTERMSIG(st));
    else
        printf("RWB-PRESSURE-OTHER st=%d\n", st);

    // The file is deliberately kept dirty-in-cache at poweroff: this
    // probe validates the MEMORY behavior (flush + evict + survive),
    // not cross-boot persistence.
    printf("RWB-DONE\n");
    return 0;
}
