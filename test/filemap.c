// filemap.c — file-fault cache-mapping probe (wildwrite lane sessions
// 24/25).
//
// Exercises the filemap_fault port: read/exec faults on private file
// VMAs map the PAGE-CACHE frame directly (read-only; writable VMAs carry
// the COW bit — first write COWs a private copy), fork duplicates the
// mapping's cache pin, and the pin is released at unmap/teardown/COW.
// Session 25 adds the MAP_SHARED writeback-coherence legs (FS*): SHARED
// ext4 mappings alias the cache frame WRITABLE (PCACHE + PCACHE_SHARED),
// stores are visible to every mapper, write(2) writes THROUGH pinned
// frames, msync/fsync flush dirty frames to disk, and truncate discards
// beyond-EOF state.
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
//   FS1  two concurrent SHARED mappers see each other's stores at once
//   FS2  write(2) lands in the LIVE mapping (write-through, not
//        invalidate) — both the live mapper and a fresh one see it
//   FS3  a mapping store is visible to pread(2) (cache coherence)
//   FS4  fork over a SHARED mapping: child store visible to the parent
//   FS5  msync(MS_SYNC) and fsync flush mapping stores (pread verifies)
//   FS6  truncate: size cut, kept range readable, past-EOF touch SIGBUS
//   FS7  munmap releases the pin; a fresh SHARED map sees retained data
//   FS8  hole pages: read as zeros, store+msync persists into them
//   FS-PERSIST  second-boot check: the pattern written+msynced in the
//        previous boot of this image reads back from cold disk
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

    // ---- second-boot persistence check (session 25): the previous boot
    // of this image wrote /filemap-persist.dat through a MAP_SHARED
    // mapping + msync(MS_SYNC) + fsync. A cold boot reading the pattern
    // back proves the writeback reached the DISK, not just the cache.
    {
        int pfd = open("/filemap-persist.dat", O_RDONLY);
        if (pfd >= 0) {
            static uint32_t pbuf[PAGE / sizeof(uint32_t)];
            int good = 1;
            for (unsigned i = 0; i < 4 && good; i++) {
                ssize_t r = pread(pfd, pbuf, PAGE, (off_t)i * PAGE);
                if (r != (ssize_t)PAGE) { good = 0; break; }
                for (unsigned j = 0; j < PAGE / sizeof(uint32_t); j++)
                    if (pbuf[j] != (0x5A5A0000u | i)) { good = 0; break; }
            }
            if (good) printf("FMAP-PERSIST-PASS\n");
            else printf("FMAP-PERSIST-FAIL\n");
            close(pfd);
        }
    }

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

    // ================= MAP_SHARED writeback legs (session 25) =================

    // ---- fixture: /filemaps.dat, 4 pages stamped 0x50..0x53 ----
    unlink("/filemaps.dat");
    int sfd = open("/filemaps.dat", O_RDWR | O_CREAT | O_TRUNC, 0644);
    if (sfd < 0) fail("FS0", "create /filemaps.dat");
    else {
        for (unsigned i = 0; i < 4; i++) {
            fill_pattern(buf, 0x50u + i);
            if (write(sfd, buf, PAGE) != PAGE) { fail("FS0", "write fixture"); break; }
        }
    }

    if (sfd >= 0) {
    // fixture sanity: what does pread see BEFORE any mapping faults?
    {
        uint32_t w0[4] = {1, 1, 1, 1}, w1[4] = {2, 2, 2, 2};
        pread(sfd, w0, 16, 0);
        pread(sfd, w1, 16, (off_t)PAGE);
        printf("FS0 dbg: pread p0=%08x p1=%08x\n", w0[0], w1[0]);
    }

    // ---- FS1: two concurrent SHARED mappers see stores at once ----
    uint32_t *a1 = mmap(NULL, 4 * PAGE, PROT_READ | PROT_WRITE, MAP_SHARED, sfd, 0);
    uint32_t *b1 = mmap(NULL, 4 * PAGE, PROT_READ | PROT_WRITE, MAP_SHARED, sfd, 0);
    if (a1 == MAP_FAILED || b1 == MAP_FAILED) fail("FS1", "mmap");
    else {
        if (!check_pattern(a1, 0x50u) || !check_pattern(b1 + PAGE / 4, 0x51u)) {
            printf("FS1 dbg: a1[0]=%08x a1[PAGE/4]=%08x b1[0]=%08x b1[PAGE/4+1]=%08x\n",
                   a1[0], a1[PAGE / 4], b1[0], b1[PAGE / 4 + 1]);
            fail("FS1", "initial content wrong");
        }
        else {
            a1[7] = 0x13572468u;                    // store through A
            if (b1[7] != 0x13572468u) fail("FS1", "B missed A's store");
            else {
                b1[PAGE / 4 + 3] = 0x2468ace0u;     // store through B
                if (a1[PAGE / 4 + 3] != 0x2468ace0u) fail("FS1", "A missed B's store");
                else ok("FS1");
            }
        }
    }

    // ---- FS2: write(2) lands in the LIVE mapping (write-through) ----
    if (a1 != MAP_FAILED) {
        fill_pattern(buf, 0xB1u);
        if (pwrite(sfd, buf, PAGE, 2 * PAGE) != (ssize_t)PAGE) fail("FS2", "pwrite");
        else if (!check_pattern(a1 + 2 * PAGE / 4, 0xB1u))
            fail("FS2", "live mapping missed write(2)");
        else {
            uint32_t *fr = mmap(NULL, PAGE, PROT_READ, MAP_SHARED, sfd, 2 * PAGE);
            if (fr == MAP_FAILED) fail("FS2", "fresh mmap");
            else {
                if (check_pattern(fr, 0xB1u)) ok("FS2");
                else fail("FS2", "fresh mapping served stale bytes");
                munmap(fr, PAGE);
            }
        }
    }

    // ---- FS3: mapping store visible to pread(2) ----
    if (b1 != MAP_FAILED) {
        for (unsigned i = 0; i < PAGE / sizeof(uint32_t); i++)
            b1[3 * PAGE / 4 + i] = 0xC3000000u | i;
        if (pread(sfd, buf, PAGE, 3 * PAGE) != (ssize_t)PAGE)
            fail("FS3", "pread");
        else {
            unsigned j;
            for (j = 0; j < PAGE / sizeof(uint32_t); j++)
                if (buf[j] != (0xC3000000u | j)) break;
            if (j == PAGE / sizeof(uint32_t)) ok("FS3");
            else fail("FS3", "pread missed mapping stores");
        }
    }

    // ---- FS4: fork coherence — child store visible to the parent ----
    if (a1 != MAP_FAILED) {
        pid_t pid = fork();
        if (pid == 0) {
            a1[2] = 0x0D4D4D4Du;                    // through the shared frame
            raw_write("FS4 child: stored marker\n");
            _exit(0);
        }
        int st = 0;
        waitpid(pid, &st, 0);
        if (a1[2] == 0x0D4D4D4Du) ok("FS4");
        else fail("FS4", "parent missed child's store");
    }

    // ---- FS5: msync(MS_SYNC) and fsync flush mapping stores ----
    if (a1 != MAP_FAILED) {
        for (unsigned p = 0; p < 4; p++)
            for (unsigned i = 0; i < PAGE / sizeof(uint32_t); i++)
                a1[p * PAGE / 4 + i] = 0xE5000000u | (p << 8) | i;
        a1[PAGE / 4 + 1] = 0x0E6E6E6Eu;             // one divergent word, p1
        if (msync(a1, 4 * PAGE, MS_SYNC) != 0) fail("FS5", "msync");
        else if (pread(sfd, buf, PAGE, 0) != (ssize_t)PAGE
                 || buf[5] != (0xE5000000u | 5u))
            fail("FS5", "pread after msync wrong");
        else {
            // fsync path: store WITHOUT msync, then fsync(fd)
            a1[2] = 0x0F5F5F5Fu;
            if (fsync(sfd) != 0) fail("FS5", "fsync");
            else if (pread(sfd, buf, PAGE, 0) != (ssize_t)PAGE
                     || buf[2] != 0x0F5F5F5Fu)
                fail("FS5", "pread after fsync wrong");
            else ok("FS5");
        }
    }

    // ---- FS6: truncate cuts the file; past-EOF touch SIGBUSes ----
    if (ftruncate(sfd, (off_t)2 * PAGE) != 0) fail("FS6", "ftruncate");
    else {
        struct stat st;
        if (fstat(sfd, &st) != 0 || st.st_size != (off_t)2 * PAGE)
            fail("FS6", "size not cut");
        else if (pread(sfd, buf, PAGE, 0) != (ssize_t)PAGE
                 || buf[2] != 0x0F5F5F5Fu)
            fail("FS6", "kept range lost the msynced data");
        else if (pread(sfd, buf, PAGE, 2 * PAGE) != 0)
            fail("FS6", "pread past EOF returned data");
        else {
            uint32_t *pm = mmap(NULL, PAGE, PROT_READ | PROT_WRITE,
                                MAP_SHARED, sfd, 2 * PAGE);
            if (pm == MAP_FAILED) fail("FS6", "mmap past EOF");
            else {
                pid_t pid = fork();
                if (pid == 0) {
                    raw_write("FS6 child: touching past-EOF page\n");
                    volatile uint32_t *x = &pm[0];
                    *x;                              // SIGBUS expected
                    raw_write("FS6 child: NO SIGBUS (bug)\n");
                    _exit(9);
                }
                int wst = 0;
                waitpid(pid, &wst, 0);
                if (WIFSIGNALED(wst) && WTERMSIG(wst) == SIGBUS) ok("FS6");
                else fail("FS6", "past-EOF touch did not SIGBUS");
                munmap(pm, PAGE);
            }
        }
    }

    // ---- FS7: unmap releases the pin; a fresh map sees retained data ----
    if (a1 != MAP_FAILED && b1 != MAP_FAILED) {
        munmap(a1, 4 * PAGE);
        munmap(b1, 4 * PAGE);
        a1 = b1 = MAP_FAILED;
        uint32_t *f7 = mmap(NULL, PAGE, PROT_READ | PROT_WRITE, MAP_SHARED, sfd, 0);
        if (f7 == MAP_FAILED) fail("FS7", "fresh mmap after unmap");
        else {
            if (f7[2] != 0x0F5F5F5Fu || (f7[5] >> 24) != 0xE5u)
                fail("FS7", "data lost across unmap/remap");
            else {
                f7[9] = 0x0F7F7F7Fu;
                if (msync(f7, PAGE, MS_SYNC) != 0) fail("FS7", "msync");
                else {
                    munmap(f7, PAGE);
                    uint32_t *f7b = mmap(NULL, PAGE, PROT_READ, MAP_SHARED, sfd, 0);
                    if (f7b == MAP_FAILED) fail("FS7", "second remap");
                    else {
                        if (f7b[9] == 0x0F7F7F7Fu) ok("FS7");
                        else fail("FS7", "store lost across unmap/remap");
                        munmap(f7b, PAGE);
                    }
                }
            }
        }
    }

    // ---- FS8: hole pages — zeros on read, store+msync persists ----
    if (ftruncate(sfd, (off_t)6 * PAGE) != 0) fail("FS8", "ftruncate grow");
    else {
        uint32_t *h8 = mmap(NULL, 6 * PAGE, PROT_READ | PROT_WRITE,
                            MAP_SHARED, sfd, 0);
        if (h8 == MAP_FAILED) fail("FS8", "mmap with holes");
        else {
            unsigned z;
            for (z = 0; z < PAGE / sizeof(uint32_t); z++)
                if (h8[4 * PAGE / 4 + z] != 0) break;
            if (z != PAGE / sizeof(uint32_t)) fail("FS8", "hole not zeros");
            else {
                for (unsigned i = 0; i < PAGE / sizeof(uint32_t); i++)
                    h8[4 * PAGE / 4 + i] = 0xA8000000u | i;
                if (msync(h8, 6 * PAGE, MS_SYNC) != 0) fail("FS8", "msync");
                else if (pread(sfd, buf, PAGE, 4 * PAGE) != (ssize_t)PAGE
                         || buf[3] != (0xA8000000u | 3u))
                    fail("FS8", "hole store not persisted");
                else ok("FS8");
            }
            munmap(h8, 6 * PAGE);
        }
    }

    close(sfd);
    unlink("/filemaps.dat");
    } // sfd >= 0

    // ---- persistence artifact for the second-boot check ----
    {
        unlink("/filemap-persist.dat");
        int pfd = open("/filemap-persist.dat", O_RDWR | O_CREAT | O_TRUNC, 0644);
        if (pfd < 0) fail("FSP", "create persist file");
        else {
            if (ftruncate(pfd, (off_t)4 * PAGE) != 0) fail("FSP", "ftruncate");
            else {
                uint32_t *pp = mmap(NULL, 4 * PAGE, PROT_READ | PROT_WRITE,
                                    MAP_SHARED, pfd, 0);
                if (pp == MAP_FAILED) fail("FSP", "mmap");
                else {
                    for (unsigned pgi = 0; pgi < 4; pgi++)
                        for (unsigned i = 0; i < PAGE / sizeof(uint32_t); i++)
                            pp[pgi * PAGE / 4 + i] = 0x5A5A0000u | pgi;
                    if (msync(pp, 4 * PAGE, MS_SYNC) != 0 || fsync(pfd) != 0)
                        fail("FSP", "msync/fsync");
                    else
                        ok("FSP");
                    munmap(pp, 4 * PAGE);
                }
            }
            // DELIBERATELY not unlinked: the second boot of this image
            // verifies it from cold disk.
            close(pfd);
        }
    }

    close(fd);
    unlink("/filemap.dat");
    unlink("/filemap2.dat");
    if (fails == 0) { printf("FMAP-PASS\n"); return 0; }
    printf("FMAP-FAIL (%d)\n", fails);
    return 1;
}
