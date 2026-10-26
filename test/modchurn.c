/* modchurn.c — module-load/unload churn member driver (PID 1, never exits).
 *
 * The bssslack2 hunt (session 9; OH lane leg-22 handoff): a dlopen'd
 * module's never-written bss-slack page arrived carrying foreign data.
 * dirtyscan covers plain anon surfaces; THIS probe reproduces the
 * module shape — dlopen/dlclose churn of libmodchurn.so whose ctor
 * writes only page 0 of a 256KiB bss — and reads every byte nobody
 * wrote. Foreign content there = the recycler handed out a dirty (or
 * double-mapped) frame, the missing wild-writer class.
 *
 * Phases (2-3 random per cycle):
 *   modscan   dlopen -> scan untouched bss + maps-derived slack tail ->
 *             dlclose, R iterations. Any nonzero byte = MODDIRTYHIT.
 *   donor     fork children that memset large regions with per-cycle
 *             ASCII tags and exit — recognizable frames land in the
 *             recycler pool, so a later dirty re-hand IDENTIFIES the
 *             donor.
 *   modonor   donor waves interleaved with short modscan bursts — the
 *             tightest race window (freed donor frame re-handed as a
 *             module bss page within the same second).
 *   swaprt    fill a 24MiB region with the cycle pattern, fork hog
 *             children touching ~700MiB total so vmscan swaps the
 *             parent region out, wait, then re-read: any byte != the
 *             pattern = SWAPHIT (the swap-pending fix's negative test;
 *             foreign bytes = another slot's content read back).
 *   dnt       fill pages, MADV_DONTNEED them, re-read: demand-zap
 *             pages must come back zero.
 *
 * Heartbeat "MODCHURN-BEAT <cycle> <phase> ..." (wedge rule 300s).
 * MODDIRTYHIT / SWAPHIT print hex+ascii context and are promoted to
 * CRITICAL by fleet-classify.
 *
 * Build (dynamic — musl dlopen needs a dynamic main):
 *   run/fleet/bin/riscv64-musl-gcc -O1 -o modchurn modchurn.c -ldl
 */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

#define PAGE 4096UL
#define MiB (1024UL * 1024UL)
#define LIBPATH "/test/libmodchurn.so"

static unsigned long seed = 0x9E3779B97F4A7C15UL;
static inline unsigned long rnd(void)
{
    seed = seed * 6364136223846793005UL + 1442695040888963407UL;
    return seed >> 17;
}

static unsigned long cycle, loads, dirty_hits, swap_hits, dnt_hits;
static unsigned long donor_tags_fed, swap_roundtrips;

static void dump_ctx(const char *tag, const void *p, unsigned long expect)
{
    const uintptr_t a = ((uintptr_t)p) & ~7UL;
    const unsigned char *q = (const unsigned char *)(a - 16);
    printf("%s va=%p expect=%02lx\n  hex: ", tag, p, expect);
    for (int i = 0; i < 64; i++) {
        printf("%02x", q[i]);
        if (i % 16 == 15) putchar(' ');
    }
    printf("\n  ascii: ");
    for (int i = 0; i < 64; i++) {
        unsigned char c = q[i];
        putchar((c >= 0x20 && c < 0x7f) ? c : '.');
    }
    printf("\n");
    fflush(stdout);
}

/* ---- module surface -------------------------------------------------- */

/* Scan [from, end): first nonzero byte wins. Returns pointer or NULL. */
static const unsigned char *scan_nonzero(const unsigned char *from,
                                         const unsigned char *end)
{
    for (; from < end; from++)
        if (*from)
            return from;
    return NULL;
}

/* End of the anon region containing `v` per /proc/self/maps (0 on any
 * trouble — caller then scans only the in-array slack). */
static uintptr_t anon_vma_end(const void *v)
{
    uintptr_t va = (uintptr_t)v, lo = 0, hi = 0;
    FILE *f = fopen("/proc/self/maps", "r");
    if (!f)
        return 0;
    char line[256];
    while (fgets(line, sizeof line, f)) {
        uintptr_t slo, shi;
        if (sscanf(line, "%lx-%lx", &slo, &shi) == 2 && va >= slo && va < shi) {
            /* module bss is the private-anon tail; take the mapping as
             *-is whether file-backed head or anon — the untouched range
             * we scan starts inside the array anyway. */
            lo = slo; hi = shi;
            break;
        }
    }
    fclose(f);
    return hi;
}

/* dlopen + full untouched-region scan + dlclose. Returns hits found. */
static int mod_scan_once(int idx)
{
    void *h = dlopen(LIBPATH, RTLD_NOW);
    if (!h) {
        printf("MODCHURN-ERR dlopen #%d: %s\n", idx, dlerror());
        fflush(stdout);
        return 0;
    }
    char *bss = (char *)dlsym(h, "churn_bss");
    if (!bss) {
        printf("MODCHURN-ERR dlsym churn_bss #%d: %s\n", idx, dlerror());
        fflush(stdout);
        dlclose(h);
        return 0;
    }
    int hits = 0;
    /* untouched interior: page 1 .. end of the 256KiB array */
    const unsigned char *bssu = (const unsigned char *)bss;
    const unsigned char *nz = scan_nonzero(bssu + PAGE, bssu + 0x40000);
    if (nz) {
        printf("MODDIRTYHIT #%d interior bss+0x%lx\n", idx,
               (unsigned long)(nz - bssu));
        dump_ctx("  ctx-interior", nz, 0);
        hits++;
    }
    /* slack past the array end, up to the VMA end (maps-derived). The
     * ctor never writes there and the module never addresses it — the
     * exact bssslack2 shape. */
    uintptr_t vend = anon_vma_end(bss + 0x40000 - 1);
    if (vend) {
        uintptr_t aend = (uintptr_t)bss + 0x40000;
        if (vend > aend) {
            nz = scan_nonzero((unsigned char *)aend, (unsigned char *)vend);
            if (nz) {
                printf("MODDIRTYHIT #%d SLACK bss+0x%lx (past memsz end)\n",
                       idx, (unsigned long)((uintptr_t)nz - (uintptr_t)bss));
                dump_ctx("  ctx-slack", nz, 0);
                hits++;
            }
        }
    }
    /* the second bss blob: ctor writes 1 byte of 64KiB */
    char *bss2 = (char *)dlsym(h, "churn_bss2");
    if (bss2) {
        const unsigned char *b2u = (const unsigned char *)bss2;
        nz = scan_nonzero(b2u + 1, b2u + 0x10000);
        if (nz) {
            printf("MODDIRTYHIT #%d bss2+0x%lx\n", idx,
                   (unsigned long)(nz - b2u));
            dump_ctx("  ctx-bss2", nz, 0);
            hits++;
        }
    }
    dlclose(h);
    return hits;
}

static void phase_modscan(void)
{
    for (int i = 0; i < 30; i++) {
        loads++;
        dirty_hits += mod_scan_once(i);
        usleep(2000 + (rnd() % 4000));
    }
}

/* ---- donor / pressure surfaces --------------------------------------- */

/* reap children, keeping the serial warm (wedge rule is 300s of
 * silence; a swap-pressured hog wave can run minutes) */
static void wait_beating(const char *why)
{
    int quiet = 0;
    for (;;) {
        pid_t r = waitpid(-1, NULL, WNOHANG);
        if (r > 0) {
            quiet = 0;
            continue;
        }
        if (r < 0)
            break; /* ECHILD: everything reaped */
        if (++quiet >= 15) {
            printf("MODCHURN-BEAT %lu waiting-%s loads=%lu dh=%lu sh=%lu\n",
                   cycle, why, loads, dirty_hits, swap_hits);
            quiet = 0;
        }
        sleep(1);
    }
}

static void donor_wave(int waveno)
{
    for (int c = 0; c < 6; c++) {
        pid_t p = fork();
        if (p == 0) {
            unsigned long sz = (4 + rnd() % 12) * MiB;
            char *m = mmap(NULL, sz, PROT_READ | PROT_WRITE,
                           MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
            if (m == MAP_FAILED)
                _exit(0);
            for (unsigned long off = 0; off < sz; off += 16) {
                int n = snprintf(m + off, 16, "D%luW%d", cycle % 100000, waveno);
                m[off + n] = 'D';
            }
            _exit(0); /* frames full of tags go to the recycler */
        }
    }
    wait_beating("donor");
    donor_tags_fed += 6;
}

static void phase_donor(void)
{
    for (int w = 0; w < 4; w++)
        donor_wave(w);
}

static void phase_modonor(void)
{
    for (int w = 0; w < 6; w++) {
        donor_wave(w);
        for (int i = 0; i < 6; i++) {
            loads++;
            dirty_hits += mod_scan_once(1000 + w * 10 + i);
        }
    }
}

/* Swap roundtrip: pattern region swapped out by hog children, read
 * back, every byte must still be the pattern. */
static void phase_swaprt(void)
{
    const unsigned long sz = 24UL * MiB;
    unsigned char *m = mmap(NULL, sz, PROT_READ | PROT_WRITE,
                            MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (m == MAP_FAILED)
        return;
    unsigned char pat = (unsigned char)(0x21 + rnd() % 0x5E);
    memset(m, pat, sz); /* fault everything in first */

    /* hogs: ~700MiB of touched anon in a 1G guest forces the parent
     * region (clean of recent access) out to swap */
    for (int c = 0; c < 4; c++) {
        pid_t p = fork();
        if (p == 0) {
            unsigned long hs = 175UL * MiB;
            unsigned char *hm = mmap(NULL, hs, PROT_READ | PROT_WRITE,
                                     MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
            if (hm == MAP_FAILED)
                _exit(0);
            for (unsigned long off = 0; off < hs; off += PAGE)
                hm[off] = (unsigned char)(c + 1);
            _exit(0);
        }
    }
    wait_beating("swap-hog");
    swap_roundtrips++;

    for (unsigned long off = 0; off < sz; off++) {
        if (m[off] != pat) {
            printf("SWAPHIT off=0x%lx pattern=0x%02x got=0x%02x\n",
                   off, pat, m[off]);
            dump_ctx("  ctx-swap", m + off, pat);
            swap_hits++;
            break; /* one full-context hit per roundtrip is enough */
        }
    }
    /* post-swap module load: freshly faulted bss pages under an
     * active swap device — the f87b0f29 window's exact shape */
    loads++;
    dirty_hits += mod_scan_once(5000);

    munmap(m, sz);
}

/* MADV_DONTNEED round-trip: zapped pages must read back zero. */
static void phase_dnt(void)
{
    const unsigned long sz = 8UL * MiB;
    unsigned char *m = mmap(NULL, sz, PROT_READ | PROT_WRITE,
                            MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (m == MAP_FAILED)
        return;
    memset(m, 0x5C, sz);
    if (madvise(m, sz, MADV_DONTNEED) != 0)
        printf("MODCHURN-ERR madvise: %s\n", strerror(errno));
    for (unsigned long off = 0; off < sz; off++) {
        if (m[off]) {
            printf("MODDIRTYHIT dnt off=0x%lx got=0x%02x (post-zap nonzero)\n",
                   off, m[off]);
            dump_ctx("  ctx-dnt", m + off, 0);
            dnt_hits++;
            break;
        }
    }
    munmap(m, sz);
}

int main(void)
{
    setvbuf(stdout, NULL, _IOLBF, 4096);
    printf("modchurn start pid=%d lib=%s\n", getpid(), LIBPATH);
    /* one cold load to shake out loader trouble before the loop */
    if (mod_scan_once(-1) == 0 && loads == 1)
        printf("modchurn cold-load ok\n");

    static const char *phases[] = { "modscan", "donor", "modonor", "dnt" };
    for (;;) {
        cycle++;
        int nph = 2 + rnd() % 2;
        for (int i = 0; i < nph; i++) {
            const char *ph = phases[rnd() % 4];
            printf("MODCHURN-BEAT %lu %s loads=%lu dh=%lu sh=%lu dnh=%lu donors=%lu srts=%lu\n",
                   cycle, ph, loads, dirty_hits, swap_hits, dnt_hits,
                   donor_tags_fed, swap_roundtrips);
            if (!strcmp(ph, "modscan")) phase_modscan();
            else if (!strcmp(ph, "donor")) phase_donor();
            else if (!strcmp(ph, "modonor")) phase_modonor();
            else phase_dnt();
        }
        if (cycle % 3 == 0)
            phase_swaprt();
        printf("MODCHURN-BEAT %lu cyclesum loads=%lu dh=%lu sh=%lu dnh=%lu donors=%lu srts=%lu\n",
               cycle, loads, dirty_hits, swap_hits, dnt_hits,
               donor_tags_fed, swap_roundtrips);
    }
}
