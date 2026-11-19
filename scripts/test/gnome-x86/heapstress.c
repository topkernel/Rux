// heapstress.c — silent user-memory corruption reproducer for Rux x86_64.
//
// gnome-shell died at ~12min with `malloc.c:4302: _int_malloc: Assertion
// (size) >= (nb)` (glibc arena metadata corrupted). This program hammers
// the same kernel surfaces WITHOUT the desktop stack, so the corruption
// either reproduces in minutes or the desktop chain is exonerated:
//
//   - multithreaded malloc/free churn (per-thread arenas in TLS — the
//     corrupted structure class), every block filled with a position-
//     derived canary pattern and verified after each round
//   - interleaved file reads (page-cache pressure — fontconfig was
//     mid-load when the shell died)
//   - mmap/mprotect RW->RX JIT-style cycles (llvmpipe/SpiderMonkey
//     shape), including executing a ret from a re-protected page
//   - fork churn in parallel (page-table/COW pressure)
//
// Output goes to the serial console: run as init=/sbin/heapstress-init.
// Exit code 42 on canary mismatch (also printed with details).
#include <errno.h>
#include <stdarg.h>
#include <fcntl.h>
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

#define NTHREADS 4
#define NBLOCKS 96

static volatile int stop_flag;
static volatile int corrupt_detected;

// mode bits read from /etc/stress-mode (default full)
static int do_malloc = 1, do_files = 1, do_mprotect = 1, do_munmap = 1;
static int nthreads = NTHREADS;
static int small_mode;
static void load_mode(void) {
    char buf[32] = ""; int fd = open("/etc/stress-mode", O_RDONLY);
    if (fd >= 0) { read(fd, buf, sizeof buf - 1); close(fd); }
    if (strstr(buf, "t1"))       { nthreads = 1; }
    if (strstr(buf, "small"))    { small_mode = 1; }
    if (strstr(buf, "malloc"))   { do_files = do_mprotect = do_munmap = 0; }
    else if (strstr(buf, "mmaponly")) { do_files = do_mprotect = 0; }
    else if (strstr(buf, "mprot"))    { do_files = do_munmap = 0; }
    else if (strstr(buf, "nojit"))    { do_mprotect = do_munmap = 0; do_files = 1; }
    // "full" (or anything else): everything on
}

static uint32_t g_got, g_want;

static void conprint(const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    vprintf(fmt, ap);
    va_end(ap);
    fflush(stdout);
}

static uint32_t pat(uint32_t seed, size_t off) {
    uint32_t x = seed ^ (uint32_t)off ^ (uint32_t)(off >> 17);
    x ^= x >> 13; x *= 0x5bd1e995u; x ^= x >> 15;
    return x;
}

static void fill_block(unsigned char *p, size_t sz, uint32_t seed) {
    for (size_t off = 0; off + 4 <= sz; off += 4)
        memcpy(p + off, &(uint32_t){ pat(seed, off) }, 4);
}

static int verify_block(const unsigned char *p, size_t sz, uint32_t seed,
                        const char *who, int idx) {
    for (size_t off = 0; off + 4 <= sz; off += 4) {
        uint32_t got;
        memcpy(&got, p + off, 4);
        uint32_t want = pat(seed, off);
        if (got != want) {
            conprint("[heapstress] CORRUPTION %s blk=%d off=%zu want=%08x got=%08x p=%p\n",
                     who, idx, off, want, got, (void *)p);
            return -1;
        }
    }
    return 0;
}

static void *file_to_read(void) {
    static const char *cands[] = {
        "/usr/share/glib-2.0/schemas/gschemas.compiled",
        "/usr/lib/x86_64-linux-gnu/libX11.so.6.2.0",
        "/etc/machine-id",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        0,
    };
    static int rr;
    for (int i = 0; cands[i]; i++) {
        int fd = open(cands[(rr + i) % 4], O_RDONLY);
        if (fd >= 0) { close(fd); const char *r = cands[(rr + i) % 4]; rr++; return (void *)r; }
    }
    return 0;
}

// mmap'd JIT-style region: write code, flip RW->RX, execute, flip back,
// munmap. Breakage shows as SIGSEGV (NX left set) or canary corruption.
static int jit_cycle(int n) {
    if (!do_mprotect && !do_munmap) return 0; // nojit mode: skip entirely
    size_t sz = 0x2000;
    unsigned char *m = mmap(0, sz, PROT_READ | PROT_WRITE,
                            MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (m == MAP_FAILED) return 0;
    fill_block(m, sz / 2, 0xabcdef00u + (uint32_t)n); // data half
    if (do_mprotect) {
        // code half: a function that returns its argument + 1
        m[sz / 2 + 0] = 0x8d; m[sz / 2 + 1] = 0x47; m[sz / 2 + 2] = 0x01; // lea 1(%rdi),%eax
        m[sz / 2 + 3] = 0xc3;                                          // ret
        if (mprotect(m + sz / 2, sz / 2, PROT_READ | PROT_EXEC) != 0) {
            conprint("[heapstress] mprotect RX failed errno=%d\n", errno);
            munmap(m, sz);
            return -1;
        }
        int (*fn)(int) = (int (*)(int))(m + sz / 2);
        int r = fn(n);
        if (r != n + 1) {
            conprint("[heapstress] JIT exec WRONG r=%d n=%d\n", r, n);
            munmap(m, sz);
            return -1;
        }
        mprotect(m + sz / 2, sz / 2, PROT_READ | PROT_WRITE);
    }
    if (do_munmap) {
        int bad = verify_block(m, sz / 2, 0xabcdef00u + (uint32_t)n, "jit", n);
        munmap(m, sz);
        return bad;
    }
    return 0; // mprotect-only mode leaks the region on purpose
}

struct targ { int id; };

// per-worker live block tables, shared for cross-thread forensics
static unsigned char *g_blocks[NTHREADS][NBLOCKS];
static size_t g_sizes[NTHREADS][NBLOCKS];
static volatile unsigned char *g_corrupt_at;

static void forensic_dump(void) {
    conprint("[heapstress] FORENSIC: target=%p\n", (void *)g_corrupt_at);
    for (int t = 0; t < NTHREADS; t++) {
        for (int i = 0; i < NBLOCKS; i++) {
            unsigned char *p = g_blocks[t][i];
            if (!p) continue;
            long d = (long)p - (long)g_corrupt_at;
            if (d >= 0 && d < 0x10000) {
                conprint("[heapstress] FORENSIC: near t%d blk=%d p=%p sz=%zu (delta=%ld)\n",
                         t, i, (void *)p, g_sizes[t][i], d);
            }
        }
    }
    for (int t1 = 0; t1 < NTHREADS; t1++)
        for (int i1 = 0; i1 < NBLOCKS; i1++) {
            unsigned char *a = g_blocks[t1][i1];
            if (!a) continue;
            for (int t2 = t1 + 1; t2 < NTHREADS; t2++)
                for (int i2 = 0; i2 < NBLOCKS; i2++)
                    if (g_blocks[t2][i2] == a)
                        conprint("[heapstress] FORENSIC: DUPLICATE ptr %p t%d/b%d AND t%d/b%d\n",
                                 (void *)a, t1, i1, t2, i2);
        }
    int fd = open("/proc/self/maps", O_RDONLY);
    if (fd >= 0) {
        static char b[512]; ssize_t r;
        conprint("[heapstress] FORENSIC: /proc/self/maps:\n");
        while ((r = read(fd, b, sizeof b)) > 0) {
            ssize_t off = 0;
            while (off < r) {
                ssize_t w = write(1, b + off, r - off);
                if (w <= 0) break;
                off += w;
            }
        }
        close(fd);
    } else {
        conprint("[heapstress] FORENSIC: no /proc/self/maps (errno=%d)\n", errno);
    }
}

static void *worker(void *ap); // fwd

static void *worker_impl(void *ap, const char *tag) {
    struct targ *ta = ap;
    unsigned char **blocks = g_blocks[ta->id];
    size_t *sizes = g_sizes[ta->id];
    uint32_t seeds[NBLOCKS];
    unsigned rnd = 0x1234 + ta->id * 7919;
    int round = 0;

    (void)tag;
    char wtag[8]; snprintf(wtag, sizeof wtag, "w%d", ta->id);
    while (!stop_flag && !corrupt_detected) {
        // churn: alloc/fill
        for (int i = 0; i < NBLOCKS; i++) {
            rnd = rnd * 1103515245 + 12345;
            if (blocks[i]) continue;
            sizes[i] = small_mode ? 16 + (rnd % 8000) : 16 + (rnd % 48000);
            seeds[i] = (uint32_t)(ta->id << 28) ^ (uint32_t)round ^ rnd;
            blocks[i] = malloc(sizes[i]);
            if (!blocks[i]) { sizes[i] = 0; continue; }
            fill_block(blocks[i], sizes[i], seeds[i]);
        }
        // free a random half
        for (int i = 0; i < NBLOCKS; i++) {
            rnd = rnd * 1103515245 + 12345;
            if (blocks[i] && (rnd & 1)) { free(blocks[i]); blocks[i] = 0; }
        }
        // realloc survivors bigger (arena metadata churn)
        for (int i = 0; i < NBLOCKS; i++) {
            if (!blocks[i]) continue;
            if (verify_block(blocks[i], sizes[i], seeds[i], wtag, i)) {
                g_corrupt_at = blocks[i];
                corrupt_detected = 1;
                memcpy(&g_got, blocks[i], 4);
                g_want = pat(seeds[i], 0);
                conprint("[heapstress] victim bytes 0..31:");
                for (int b = 0; b < 32; b++) conprint(" %02x", blocks[i][b]);
                conprint("\n");
                return 0;
            }
            size_t nsz = small_mode ? sizes[i] : sizes[i] + 1024;
            unsigned char *nb = (nsz == sizes[i]) ? blocks[i] : realloc(blocks[i], nsz);
            if (nb) { blocks[i] = nb; sizes[i] = nsz; fill_block(nb, nsz, seeds[i] ^ 0x9999); seeds[i] ^= 0x9999; }
        }
        // file read through page cache
        const char *fn = do_files ? file_to_read() : 0;
        if (fn) {
            int fd = open(fn, O_RDONLY);
            if (fd >= 0) {
                static char buf[65536];
                ssize_t t = 0, r;
                while ((r = read(fd, buf, sizeof buf)) > 0) t += r;
                close(fd);
                (void)t;
            }
        }
        // JIT-style mprotect cycle
        if (jit_cycle(round)) corrupt_detected = 1;
        round++;
        if ((round % 8) == 0)
            conprint("[heapstress] t%d round %d ok\n", ta->id, round);
    }
    return 0;
}

static void *worker(void *ap) { return worker_impl(ap, "w"); }

static void fork_churn(void) {
    for (int i = 0; i < 30 && !stop_flag; i++) {
        pid_t p = fork();
        if (p == 0) {
            // child: small malloc churn + immediate file read + exit
            for (int j = 0; j < 64; j++) {
                size_t sz = 32 + (j * 997) % 8192;
                unsigned char *b = malloc(sz);
                if (b) { fill_block(b, sz, 0xdead0000 + j); if (verify_block(b, sz, 0xdead0000 + j, "c", j)) _exit(40); free(b); }
            }
            const char *fn = file_to_read();
            if (fn) { int fd = open(fn, O_RDONLY); if (fd >= 0) { char b[4096]; while (read(fd, b, sizeof b) > 0) {} close(fd); } }
            _exit(0);
        }
        int st;
        waitpid(p, &st, 0);
        if (WIFEXITED(st) && WEXITSTATUS(st) == 40) {
            conprint("[heapstress] CORRUPTION in fork child\n");
            corrupt_detected = 1;
        }
    }
}


int main(void) {
    int con = open("/dev/console", O_WRONLY);
    if (con >= 0) { dup2(con, STDOUT_FILENO); dup2(con, STDERR_FILENO); }

    load_mode();
    conprint("[heapstress] start threads=%d blocks=%d mode: malloc=%d files=%d mprotect=%d munmap=%d\n",
             nthreads, NBLOCKS, do_malloc, do_files, do_mprotect, do_munmap);

    unsigned long heartbeat = 0;
    pthread_t th[NTHREADS];
    struct targ tas[NTHREADS];
    for (int i = 0; i < nthreads; i++) {
        tas[i].id = i;
        pthread_create(&th[i], 0, worker, &tas[i]);
    }

    int phase = 0;
    while (!corrupt_detected) {
        sleep(10);
        heartbeat++;
        conprint("[heapstress] alive hb=%lu phase=%d\n", heartbeat, phase);
        if (phase % 3 == 1) fork_churn();
        phase++;
        if (phase >= 60) { conprint("[heapstress] 10 minutes clean, stopping\n"); break; }
    }

    stop_flag = 1;
    for (int i = 0; i < nthreads; i++) pthread_join(th[i], 0);
    if (corrupt_detected) forensic_dump();
    conprint("[heapstress] done corrupt=%d hb=%lu\n", corrupt_detected, heartbeat);
    return corrupt_detected ? 42 : 0;
}
