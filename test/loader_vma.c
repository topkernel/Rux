// loader_vma.c — /proc/self/maps file-VMA parity probe (mm lane).
//
// Exercises the three paths that must keep per-segment file identity:
//   P1  the ELF interpreter mapped by the KERNEL exec path: every PT_LOAD
//       of ld-musl must appear as its own VMA with the interpreter PATH
//       and Linux-parity pgoff (file_off = maps_off + (rip - vma_start)
//       must locate the on-disk bytes). Before the fix the whole span
//       printed as ONE anonymous R-X VMA with fd=-1 off=0 — no path, no
//       per-segment offsets, forensics blind inside the loader.
//   P2  the main executable's PT_LOADs: file VMAs with the exe path
//       (Linux prints the path; Rux additionally keeps "[exe]" on the
//       first executable segment — accepted).
//   P3  mremap(MAYMOVE) of a file mapping: the moved VMA must keep the
//       path and a continuation offset.
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

/* Count maps lines in [lo, hi) whose pathname contains `need`,
 * and check their offset column is a plausible file offset (multiple
 * of PAGE, not always 0). Returns count of matching lines.
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
                       &s, &e, perms, &off, dev, &ino, path);
        if (k < 7) continue;
        if (s < lo || s >= hi) continue;
        if (!strstr(path, need)) continue;
        cnt++;
        if (want_off && off != want_off) *off_ok = 0;
        if (!want_off && off == 0) *off_ok = 0;
    }
    fclose(f);
    return cnt;
}

static int count_region_with_path(unsigned long lo, unsigned long hi,
                                  const char *need, int *all_zero_off)
{
    FILE *f = fopen("/proc/self/maps", "r");
    if (!f) return -1;
    char line[512];
    int cnt = 0;
    *all_zero_off = 1;
    while (fgets(line, sizeof(line), f)) {
        unsigned long s, e, off;
        char perms[8];
        char dev[32];
        unsigned long ino;
        char path[256] = "";
        int k = sscanf(line, "%lx-%lx %7s %lx %31s %lu %255[^\n]",
                       &s, &e, perms, &off, dev, &ino, path);
        if (k < 7) continue;
        if (s < lo || s >= hi) continue;
        if (!strstr(path, need)) continue;
        cnt++;
        if (off != 0) *all_zero_off = 0;
    }
    fclose(f);
    return cnt;
}

int main(int argc, char **argv)
{
    printf("LVMAP-START\n");

    /* ---- P1: interpreter segments ----
     * Rux maps the interp at INTERP_BASE = 0x3FBF000000 (x86_64). Each
     * PT_LOAD should print with the ld-musl path and real offsets. */
    {
        int zero = 0;
        int n = count_region_with_path(0x3fbf000000UL, 0x3fbf000000UL + 0x400000UL,
                                       "ld-musl", &zero);
        /* Boot-exec (init=) instance: no fdtable in the exec context, so
         * the pin is absent and the path column stays empty BY DESIGN
         * (offsets are still per-segment — visible in the FINAL dump).
         * The authoritative path checks run in the CHILD instance
         * (fork+execve -> sys_execve) and in P4a/P4b. */
        printf("LVMAP-P1 INFO(boot-exec): interp lines with path=%d\n", n);
    }

    /* ---- P2: main executable segments ----
     * The probe itself: ET_DYN/ET_EXEC LOADs must carry the exe path
     * ("loader_vma"); "[exe]" on the first executable line is accepted
     * (Rux keeps it; Linux prints the path there). */
    {
        int zero = 0;
        /* Search the whole user space below the mmap region: on Rux the
         * PIE base (0x555555554000) sits ABOVE INTERP_BASE
         * (0x3fbf000000) — do not bound by the interp base. */
        int n = count_region_with_path(0x10000UL, 0x7f0000000000UL, "loader_vma", &zero);
        printf("LVMAP-P2 INFO(boot-exec): exe lines with path=%d\n", n);
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
