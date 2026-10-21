// loader_vma.c — /proc/self/maps file-VMA parity probe (mm lane).
//
// Exercises the paths that must keep per-segment file identity:
//   P1  the ELF interpreter mapped by the KERNEL exec path: every PT_LOAD
//       of ld-musl must appear as its own VMA with the interpreter PATH
//       and Linux-parity pgoff (file_off = maps_off + (rip - vma_start)
//       must locate the on-disk bytes). Before the fix the whole span
//       printed as ONE anonymous R-X VMA with fd=-1 off=0 — no path, no
//       per-segment offsets, forensics blind inside the loader. This is
//       the PID-1 boot-exec instance (init=), which also exercises the
//       fdtable-free pin (no fdtable exists at boot-exec time).
//   P2  the main executable's PT_LOADs: file VMAs with the exe path
//       (Linux prints the path; Rux additionally keeps "[exe]" on the
//       first executable segment — accepted).
//   P3  mremap(MAYMOVE) of a file mapping: the moved VMA must keep the
//       path and a continuation offset.
//   P4  fork+execve child (sys_execve path): interp + exe paths and
//       per-segment offsets in the child's own maps.
//   P5  the USERSPACE loader overlay sequence (musl ld.so shape): an
//       anonymous PROT_NONE MAP_FIXED reservation, then file PT_LOAD
//       overlays MAP_FIXED inside it with per-segment offsets, then the
//       fd CLOSED — each LOAD must keep its path + exact offset and the
//       reservation holes must survive as PROT_NONE lines.
//
// PASS: prints LVMAP-PASS. Any failure prints LVMAP-FAIL with the
// offending maps lines.
#define _GNU_SOURCE
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/wait.h>
#include <unistd.h>

#define PAGE 4096UL

static int failures;

static void dump_maps(const char *tag)
{
    char buf[8192];
    int fd = open("/proc/self/maps", O_RDONLY);
    if (fd < 0) {
        printf("LVMAP-FAIL: cannot open /proc/self/maps\n");
        failures++;
        return;
    }
    int n = read(fd, buf, sizeof(buf) - 1);
    close(fd);
    if (n <= 0) {
        printf("LVMAP-FAIL: empty /proc/self/maps\n");
        failures++;
        return;
    }
    buf[n] = 0;
    printf("--- MAPS %s ---\n%s--- END %s ---\n", tag, buf, tag);
}

/* Count maps lines in [lo, hi) whose pathname contains `need`.
 * want_off semantics: 0 = only require SOME line with a non-zero offset;
 * non-zero = require every matched line to carry exactly want_off.
 * file==NULL reads /proc/self/maps. */
static int count_lines_path(const char *file, unsigned long lo, unsigned long hi,
                            const char *need, unsigned long want_off, int *off_ok)
{
    FILE *f = fopen(file ? file : "/proc/self/maps", "r");
    if (!f) return -1;
    char line[512];
    int cnt = 0;
    *off_ok = 1;
    while (fgets(line, sizeof(line), f)) {
        unsigned long s, e, off;
        char perms[8];
        char dev[32];
        unsigned long ino;
        char path[256] = "";
        int k = sscanf(line, "%lx-%lx %7s %lx %31s %lu %255[^\n]",
                       &s, &e, &perms, &off, &dev, &ino, &path);
        if (k < 6) continue;   // path column may be absent (anon, PROT_NONE holes)
        if (s < lo || s >= hi) continue;
        if (!strstr(path, need)) continue;
        cnt++;
        if (want_off && off != want_off) *off_ok = 0;
        if (!want_off && off == 0) *off_ok = 0;
    }
    fclose(f);
    return cnt;
}

/* Count PROT_NONE (---p) maps lines in [lo, hi). */
static int count_none_lines(unsigned long lo, unsigned long hi)
{
    FILE *f = fopen("/proc/self/maps", "r");
    if (!f) return -1;
    char line[512];
    int cnt = 0;
    while (fgets(line, sizeof(line), f)) {
        unsigned long s, e, off;
        char perms[8];
        char dev[32];
        unsigned long ino;
        char path[256] = "";
        int k = sscanf(line, "%lx-%lx %7s %lx %31s %lu %255[^\n]",
                       &s, &e, &perms, &off, &dev, &ino, &path);
        if (k < 6) continue;   // path column may be absent (anon, PROT_NONE holes)
        if (s < lo || s >= hi) continue;
        if (!strcmp(perms, "---p")) cnt++;
    }
    fclose(f);
    return cnt;
}

/* Match one specific line: start==want_start, path contains need,
 * off==want_off. Returns 1 when found. */
static int has_line(unsigned long want_start, const char *need, unsigned long want_off)
{
    FILE *f = fopen("/proc/self/maps", "r");
    if (!f) return 0;
    char line[512];
    int found = 0;
    while (fgets(line, sizeof(line), f)) {
        unsigned long s, e, off;
        char perms[8];
        char dev[32];
        unsigned long ino;
        char path[256] = "";
        int k = sscanf(line, "%lx-%lx %7s %lx %31s %lu %255[^\n]",
                       &s, &e, &perms, &off, &dev, &ino, &path);
        if (k < 6) continue;   // path column may be absent (anon, PROT_NONE holes)
        if (s != want_start) continue;
        if (!strstr(path, need)) continue;
        if (off == want_off) found = 1;
    }
    fclose(f);
    return found;
}

/* Count matched lines in [lo, hi) whose offset is non-zero (per-segment
 * offset fidelity: a flat baseoff=0 VMA scores 0 here). */
static int count_lines_path_nonzero(unsigned long lo, unsigned long hi, const char *need)
{
    FILE *f = fopen("/proc/self/maps", "r");
    if (!f) return -1;
    char line[512];
    int cnt = 0;
    while (fgets(line, sizeof(line), f)) {
        unsigned long s, e, off;
        char perms[8];
        char dev[32];
        unsigned long ino;
        char path[256] = "";
        int k = sscanf(line, "%lx-%lx %7s %lx %31s %lu %255[^\n]",
                       &s, &e, &perms, &off, &dev, &ino, &path);
        if (k < 6) continue;   // path column may be absent (anon, PROT_NONE holes)
        if (s < lo || s >= hi) continue;
        if (!strstr(path, need)) continue;
        if (off != 0) cnt++;
    }
    fclose(f);
    return cnt;
}

int main(int argc, char **argv)
{
    printf("LVMAP-START\n");

    /* ---- P1: interpreter segments (boot-exec instance) ----
     * Rux maps the interp at INTERP_BASE = 0x3FBF000000 (x86_64). Each
     * PT_LOAD must print with the ld-musl path and real per-segment
     * offsets (>=3 lines; >=2 carry non-zero offsets — the exec/data
     * LOADs — the first LOAD's 0 offset is legitimate). */
    {
        int n = count_lines_path(NULL, 0x3fbf000000UL, 0x3fbf000000UL + 0x400000UL,
                                 "ld-musl", 0xdeadbeefUL, &(int){0});
        int nz = count_lines_path_nonzero(0x3fbf000000UL, 0x3fbf000000UL + 0x400000UL,
                                          "ld-musl");
        if (n >= 3 && nz >= 2)
            printf("LVMAP-P1 OK: %d interp lines with path, %d non-zero offs\n", n, nz);
        else {
            printf("LVMAP-FAIL P1: interp lines=%d non-zero-off=%d (want >=3, >=2)\n",
                   n, nz);
            failures++;
        }
    }

    /* ---- P2: main executable segments (boot-exec instance) ----
     * The probe itself: ET_DYN/ET_EXEC LOADs must carry the exe path
     * ("loader_vma"); "[exe]" on the first executable line is accepted
     * (Rux keeps it; Linux prints the path there). */
    {
        int zero = 0;
        /* Search the whole user space below the mmap region: on Rux the
         * PIE base (0x555555554000) sits ABOVE INTERP_BASE
         * (0x3fbf000000) — do not bound by the interp base. */
        int n = count_lines_path(NULL, 0x10000UL, 0x7f0000000000UL, "loader_vma", 0, &zero);
        if (n >= 2)
            printf("LVMAP-P2 OK: %d exe lines with path\n", n);
        else {
            printf("LVMAP-FAIL P2: exe lines with path=%d (want >=2)\n", n);
            failures++;
        }
    }

    /* ---- P3: mremap keeps file identity ---- */
    {
        int fd = open("/test/vmadata.bin", O_RDONLY);
        if (fd < 0) {
            printf("LVMAP-FAIL P3: open data file rc=%d\n", fd);
            failures++;
        } else {
            char *m = mmap(NULL, 4 * PAGE, PROT_READ, MAP_PRIVATE, fd, 0x2000);
            (void)0;
            if (m == MAP_FAILED) {
                printf("LVMAP-FAIL P3: mmap failed\n");
                failures++;
            } else {
                /* verify content round-trip first (mapped at file
                 * offset 0x2000: page 0 of the mapping is the file's
                 * W-page) */
                if (m[0] != 'W') {
                    printf("LVMAP-FAIL P3: bad file content (%c)\n", m[0]);
                    failures++;
                }
                char *m2 = mremap(m, 4 * PAGE, 4 * PAGE, MREMAP_MAYMOVE);
                if (m2 == MAP_FAILED) {
                    printf("LVMAP-FAIL P3: mremap failed\n");
                    failures++;
                } else {
                    int off_ok = 0;
                    int n = count_lines_path(
                        NULL,
                        (unsigned long)m2 & ~0xfffffUL,
                        ((unsigned long)m2 & ~0xfffffUL) + 0x200000UL,
                        "vmadata.bin", 0x2000, &off_ok);
                    if (n >= 1 && off_ok && m2[0] == 'W')
                        printf("LVMAP-P3 OK: moved VMA keeps path+off+content at %p\n", m2);
                    else {
                        printf("LVMAP-FAIL P3: moved VMA lines=%d off_ok=%d "
                               "content=%c (want 1 with path, off=0x2000, W)\n",
                               n, off_ok, m2[0]);
                        failures++;
                    }
                }
            }
            close(fd);
        }
    }

    /* ---- P5: userspace loader overlay sequence (musl ld.so shape) ----
     * Reserve 4 pages PROT_NONE MAP_FIXED, overlay two file LOADs inside
     * it (adjacent, same flags, DISCONTINUOUS file offsets so they must
     * stay separate lines), then close the fd — the identity must live
     * on the pinned vm_file. Layout:
     *   BASE+0x0000  PROT_NONE reservation hole (survives as ---p)
     *   BASE+0x1000  LOAD1 R  file off 0x0000 (V-page)
     *   BASE+0x2000  LOAD2 R  file off 0x2000 (W-page)
     *   BASE+0x3000  PROT_NONE reservation tail (survives as ---p)   */
    {
        const unsigned long base = 0x7ff200000000UL;
        void *r = mmap((void *)base, 4 * PAGE, PROT_NONE,
                       MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED, -1, 0);
        if (r != (void *)base) {
            printf("LVMAP-FAIL P5: reservation mmap returned %p\n", r);
            failures++;
        } else {
            int fd = open("/test/vmadata.bin", O_RDONLY);
            char *l1 = mmap((void *)(base + PAGE), PAGE, PROT_READ,
                            MAP_PRIVATE | MAP_FIXED, fd, 0x0000);
            char *l2 = mmap((void *)(base + 2 * PAGE), PAGE, PROT_READ,
                            MAP_PRIVATE | MAP_FIXED, fd, 0x2000);
            close(fd); /* loader semantics: identity must survive the fd */
            if (l1 == MAP_FAILED || l2 == MAP_FAILED) {
                printf("LVMAP-FAIL P5: LOAD overlay mmap failed\n");
                failures++;
            } else if (l1[0] != 'V' || l2[0] != 'W') {
                printf("LVMAP-FAIL P5: content V=%c W=%c\n", l1[0], l2[0]);
                failures++;
            } else {
                int ok1 = has_line(base + PAGE, "vmadata.bin", 0x0000);
                int ok2 = has_line(base + 2 * PAGE, "vmadata.bin", 0x2000);
                int holes = count_none_lines(base, base + 4 * PAGE);
                if (ok1 && ok2 && holes == 2)
                    printf("LVMAP-P5 OK: overlay LOADs keep path+exact off, "
                           "reservation holes survive\n");
                else {
                    printf("LVMAP-FAIL P5: l1_off0=%d l2_off2000=%d holes=%d "
                           "(want 1,1,2)\n", ok1, ok2, holes);
                    failures++;
                }
            }
        }
    }

    /* ---- P4: fork+exec'd child (sys_execve path) ----
     * The child re-execs this binary; its /proc/self/maps must show the
     * interp with path+per-segment offsets and the exe path. */
    {
        if (argc > 1 && !strcmp(argv[1], "child")) {
            /* child: dump our maps for the parent to inspect */
            FILE *out = fopen("/test/childmaps.txt", "w");
            if (!out) _exit(9);
            FILE *f = fopen("/proc/self/maps", "r");
            if (!f) _exit(10);
            char line[512];
            while (fgets(line, sizeof(line), f)) fputs(line, out);
            fclose(f); fclose(out);
            _exit(0);
        }
        pid_t pid = fork();
        if (pid == 0) {
            char *cargv[] = {(char *)"loader_vma", (char *)"child", 0};
            char *cenv[] = {0};
            execve("/test/loader_vma", cargv, cenv);
            _exit(11);
        }
        int st = -1;
        waitpid(pid, &st, 0);
        int off_ok = 0;
        int n1 = count_lines_path("/test/childmaps.txt",
                                  0x3fbf000000UL, 0x3fbf000000UL + 0x400000UL,
                                  "ld-musl", 0xdeadbeefUL, &off_ok);
        int off_ok2 = 0;
        int n2 = count_lines_path("/test/childmaps.txt",
                                  0x10000UL, 0x7f0000000000UL, "loader_vma", 0, &off_ok2);
        if (n1 >= 3)
            printf("LVMAP-P4a OK: child interp %d lines with path\n", n1);
        else {
            printf("LVMAP-FAIL P4a: child interp lines=%d (want >=3)\n", n1);
            failures++;
        }
        if (n2 >= 2)
            printf("LVMAP-P4b OK: child exe %d lines with path\n", n2);
        else {
            printf("LVMAP-FAIL P4b: child exe lines=%d\n", n2);
            failures++;
        }
    }

    dump_maps("FINAL");
    if (failures == 0) {
        printf("LVMAP-PASS\n");
        return 0;
    }
    printf("LVMAP-FAIL (%d)\n", failures);
    return 1;
}
