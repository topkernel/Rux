// filemap.c — MAP_PRIVATE file-fault cache-mapping probe (wildwrite lane
// session 24; riscv64 twin of the x86_64 faultaround FA legs).
//
// Exercises the filemap_fault port: read/exec faults on private file
// VMAs map the PAGE-CACHE frame directly (read-only; writable VMAs carry
// the COW bit — first write COWs a private copy), fork duplicates the
// mapping's cache pin, and the pin is released at unmap/teardown/COW.
//
//   FC1  read faults serve correct content through the cache frames
//   FC2  write-after-read on a private mapping COWs: the local write is
//        visible, a FRESH mapping of the same file still sees the
//        original bytes (the cache frame was never scribbled)
//   FC3  fork over a read-faulted window: child write isolated, the
//        untouched pages stay shared and coherent
//   FC4  past-EOF fault raises SIGBUS in a forked child; the partial
//        tail page serves file bytes + zero fill
//   FC5  mprotect W-upgrade on a read-faulted page writes through COW
//        (writenotify parity); a fresh mapping keeps the original bytes
//   FC6  PROT_NONE round trip: marker faults, SIGSEGV while NONE,
//        content preserved after re-present
//   FC7  write(2) after a mapping invalidates the cache: a FRESH
//        mapping sees the new bytes (no stale Discard-warm fill)
//
// Forked children print via raw write(2) only (console printf from
// short-lived children is lossy — faultaround probe lesson).

#include <fcntl.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>

#define PAGE 4096U
#define NPAGES 6U

static int fails;

static void ok(const char *name) {
    printf("%s ok\n", name);
}

static void fail(const char *name, const char *why) {
    printf("%s FAIL: %s\n", name, why);
    fails++;
}

static void raw_write(const char *s) {
    // SAFETY-like: write(2) to stdout is the reliable console channel.
    write(1, s, strlen(s));
}

static void fill_pattern(uint32_t *p, unsigned idx) {
    for (unsigned i = 0; i < PAGE / sizeof(uint32_t); i++)
        p[i] = 0xA5A50000u | idx;
}

static int check_pattern(const uint32_t *p, unsigned idx) {
    for (unsigned i = 0; i < PAGE / sizeof(uint32_t); i++)
        if (p[i] != (0xA5A50000u | idx))
            return 0;
    return 1;
}

int main(void) {
    printf("filemap: pid=%d\n", getpid());

    // ---- fixture: NPAGES pages, page i fully stamped with i ----
    unlink("/filemap.dat");
    int fd = open("/filemap.dat", O_RDWR | O_CREAT | O_TRUNC, 0644);
    if (fd < 0) { fail("FC0", "create /filemap.dat"); return 1; }
    static uint32_t buf[PAGE / sizeof(uint32_t)];
    for (unsigned i = 0; i < NPAGES; i++) {
        fill_pattern(buf, i);
        if (write(fd, buf, PAGE) != PAGE) { fail("FC0", "write fixture"); return 1; }
    }

    // ---- FC1: read faults through the cache frames ----
    uint32_t *p1 = mmap(NULL, NPAGES * PAGE, PROT_READ, MAP_PRIVATE, fd, 0);
    if (p1 == MAP_FAILED) { fail("FC1", "mmap"); } else {
        unsigned bad = NPAGES;
        for (unsigned i = 0; i < NPAGES; i++)
            if (check_pattern(p1 + i * PAGE / 4, i)) bad--;
        if (bad == 0) ok("FC1"); else fail("FC1", "content wrong through cache");
        munmap(p1, NPAGES * PAGE);
    }

    // ---- FC2: write-after-read COWs; cache frame never scribbled ----
    uint32_t *p2 = mmap(NULL, 4 * PAGE, PROT_READ | PROT_WRITE, MAP_PRIVATE, fd, 0);
    if (p2 == MAP_FAILED) { fail("FC2", "mmap"); } else {
        if (!check_pattern(p2, 0)) fail("FC2", "pre-write read wrong");
        else {
            p2[0] = 0xDEADBEEFu;              // COW break on the cache leaf
            if (p2[0] != 0xDEADBEEFu) fail("FC2", "own write invisible");
            else {
                uint32_t *fresh = mmap(NULL, PAGE, PROT_READ, MAP_PRIVATE, fd, 0);
                if (fresh == MAP_FAILED) fail("FC2", "fresh mmap");
                else {
                    if (check_pattern(fresh, 0)) ok("FC2");
                    else fail("FC2", "write leaked into the cache frame");
                    munmap(fresh, PAGE);
                }
            }
        }
        munmap(p2, 4 * PAGE);
    }

    // ---- FC3: fork over a read-faulted cache window ----
    uint32_t *p3 = mmap(NULL, 4 * PAGE, PROT_READ | PROT_WRITE, MAP_PRIVATE, fd, 0);
    if (p3 == MAP_FAILED) { fail("FC3", "mmap"); } else {
        for (unsigned i = 0; i < 4; i++)
            if (!check_pattern(p3 + i * PAGE / 4, i)) fail("FC3", "pre-fault wrong");
        pid_t pid = fork();
        if (pid == 0) {
            p3[2 * PAGE / 4] = 0x0C0FFEE0u;   // child write: COW, never parent
            if (p3[2 * PAGE / 4] != 0x0C0FFEE0u) { raw_write("FC3 child: own write lost\n"); _exit(1); }
            if (!check_pattern(p3 + 3 * PAGE / 4, 3)) { raw_write("FC3 child: shared page 3 wrong\n"); _exit(2); }
            raw_write("FC3 child ok\n");
            _exit(0);
        }
        int st = 0;
        waitpid(pid, &st, 0);
        if (!WIFEXITED(st) || WEXITSTATUS(st) != 0) fail("FC3", "child failed");
        else if (!check_pattern(p3 + 2 * PAGE / 4, 2)) fail("FC3", "child write leaked into parent");
        else if (!check_pattern(p3 + 3 * PAGE / 4, 3)) fail("FC3", "parent shared page wrong");
        else ok("FC3");
        munmap(p3, 4 * PAGE);
    }

    // ---- FC4: SIGBUS past EOF + partial-tail zero fill ----
    unlink("/filemap2.dat");
    int fd2 = open("/filemap2.dat", O_RDWR | O_CREAT | O_TRUNC, 0644);
    if (fd2 < 0) { fail("FC4", "create /filemap2.dat"); } else {
        // 2 full pages + 2048 bytes of a third
        fill_pattern(buf, 0);
        write(fd2, buf, PAGE);
        fill_pattern(buf, 1);
        write(fd2, buf, PAGE);
        fill_pattern(buf, 2);
        write(fd2, buf, PAGE / 2);

        uint32_t *pt = mmap(NULL, 4 * PAGE, PROT_READ, MAP_PRIVATE, fd2, 0);
        if (pt == MAP_FAILED) fail("FC4", "mmap partial");
        else {
            // partial tail: bytes [0,2048) pattern, [2048,4096) zeros
            unsigned i = 0;
            while (i < PAGE / 8 && pt[2 * PAGE / 4 + i] == (0xA5A50002u)) i++;
            int tail_ok = (i == PAGE / 8) && (pt[2 * PAGE / 4 + i] == 0);
            if (tail_ok) ok("FC4a"); else fail("FC4a", "partial tail fill wrong");
        }
        pid_t pid = fork();
        if (pid == 0) {
            volatile uint32_t *x = &pt[3 * PAGE / 4];  // INSIDE the mapping,
            raw_write("FC4 child: touching past EOF\n");
            *x;                                        // but offset >= size: SIGBUS
            raw_write("FC4 child: NO SIGBUS (bug)\n");
            _exit(9);
        }
        int st = 0;
        waitpid(pid, &st, 0);
        if (WIFSIGNALED(st) && WTERMSIG(st) == SIGBUS) ok("FC4b");
        else fail("FC4b", "past-EOF touch did not SIGBUS");
        if (pt != MAP_FAILED) munmap(pt, 4 * PAGE);
        close(fd2);
    }

    // ---- FC5: mprotect W-upgrade diverts through COW ----
    uint32_t *p5 = mmap(NULL, 2 * PAGE, PROT_READ, MAP_PRIVATE, fd, 0);
    if (p5 == MAP_FAILED) fail("FC5", "mmap");
    else {
        if (!check_pattern(p5 + PAGE / 4, 1)) fail("FC5", "pre read wrong");
        else if (mprotect((char *)p5 + PAGE, PAGE, PROT_READ | PROT_WRITE) != 0)
            fail("FC5", "mprotect");
        else {
            p5[PAGE / 4 + 8] = 0x11223344u;  // must COW, not W-in-place
            uint32_t *fresh = mmap(NULL, PAGE, PROT_READ, MAP_PRIVATE, fd, PAGE);
            if (fresh == MAP_FAILED) fail("FC5", "fresh mmap");
            else {
                if (p5[PAGE / 4 + 8] == 0x11223344u && check_pattern(fresh, 1)) ok("FC5");
                else fail("FC5", "W-upgrade wrote the shared cache frame");
                munmap(fresh, PAGE);
            }
        }
        munmap(p5, 2 * PAGE);
    }

    // ---- FC6: PROT_NONE round trip keeps the pinned content ----
    uint32_t *p6 = mmap(NULL, 2 * PAGE, PROT_READ | PROT_WRITE, MAP_PRIVATE, fd, 0);
    if (p6 == MAP_FAILED) fail("FC6", "mmap");
    else {
        if (!check_pattern(p6, 0)) fail("FC6", "pre read wrong");
        else if (mprotect(p6, 2 * PAGE, PROT_NONE) != 0) fail("FC6", "mprotect NONE");
        else {
            pid_t pid = fork();
            if (pid == 0) {
                raw_write("FC6 child: touching PROT_NONE\n");
                volatile uint32_t *x = &p6[0];
                *x;                                   // SIGSEGV expected
                raw_write("FC6 child: NO SIGSEGV (bug)\n");
                _exit(9);
            }
            int st = 0;
            waitpid(pid, &st, 0);
            if (!(WIFSIGNALED(st) && WTERMSIG(st) == SIGSEGV))
                fail("FC6", "PROT_NONE touch did not fault");
            else if (mprotect(p6, 2 * PAGE, PROT_READ) != 0)
                fail("FC6", "mprotect back");
            else if (!check_pattern(p6, 0)) fail("FC6", "content lost after round trip");
            else ok("FC6");
        }
        munmap(p6, 2 * PAGE);
    }

    // ---- FC7: write(2) invalidates; a fresh mapping sees new bytes ----
    fill_pattern(buf, 0x77);
    if (pwrite(fd, buf, PAGE, 0) != PAGE) fail("FC7", "pwrite");
    else {
        uint32_t *fresh = mmap(NULL, PAGE, PROT_READ, MAP_PRIVATE, fd, 0);
        if (fresh == MAP_FAILED) fail("FC7", "mmap");
        else {
            if (check_pattern(fresh, 0x77u & 0xFFFFu)) ok("FC7");
            else fail("FC7", "fresh mapping served stale cache");
            munmap(fresh, PAGE);
        }
    }

    close(fd);
    unlink("/filemap.dat");
    unlink("/filemap2.dat");
    if (fails == 0) { printf("FMAP-PASS\n"); return 0; }
    printf("FMAP-FAIL (%d)\n", fails);
    return 1;
}
