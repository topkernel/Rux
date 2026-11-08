/* mprotprobe — mprotect semantics probe on the current kernel.
 *
 * Tests the exact patterns the mprotect coverage walk must accept:
 *   1. mprotect the MIDDLE of an anonymous mmap (not at its start)
 *   2. mprotect a sub-range spanning two adjacent VMAs
 *   3. mprotect the middle of the /dev/fb0 device mapping (Xorg's own
 *      pattern for DBE/render buffers)
 * Exit code = number of failures.
 */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>

static int fails;

static void t(const char *name, int rc)
{
    if (rc != 0) {
        printf("MPROT FAIL %s: %s (errno=%d)\n", name, strerror(errno), errno);
        fails++;
    } else {
        printf("MPROT ok   %s\n", name);
    }
}

int main(void)
{
    setvbuf(stdout, NULL, _IONBF, 0);

    /* 1. middle of an anon mapping: 16 pages, mprotect pages 4..8. */
    char *m = mmap(NULL, 16 * 4096, PROT_READ | PROT_WRITE,
                   MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (m == MAP_FAILED) { printf("MPROT mmap1 failed\n"); return 9; }
    m[0] = 1; m[15 * 4096] = 2;
    errno = 0;
    t("middle-of-anon RW->RO", mprotect(m + 4 * 4096, 4 * 4096, PROT_READ) == 0 ? 0 : -1);
    errno = 0;
    t("middle-of-anon RO->RW", mprotect(m + 4 * 4096, 4 * 4096, PROT_READ | PROT_WRITE) == 0 ? 0 : -1);

    /* 2. two adjacent mappings mprotected as one span. */
    char *a = mmap(NULL, 4 * 4096, PROT_READ | PROT_WRITE,
                   MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    char *b = mmap(a + 4 * 4096, 4 * 4096, PROT_READ | PROT_WRITE,
                   MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED, -1, 0);
    if (a == MAP_FAILED || b == MAP_FAILED) { printf("MPROT mmap2 failed\n"); return 9; }
    a[0] = 3; b[0] = 4;
    errno = 0;
    t("span-of-two-adjacent", mprotect(a, 8 * 4096, PROT_READ) == 0 ? 0 : -1);
    errno = 0;
    t("span-of-two-adjacent-back", mprotect(a, 8 * 4096, PROT_READ | PROT_WRITE) == 0 ? 0 : -1);

    /* 3. middle of the fb device mapping (Xorg's pattern). */
    int fd = open("/dev/fb0", O_RDWR);
    if (fd < 0) {
        printf("MPROT open fb0 failed (skip)\n");
    } else {
        char *fb = mmap(NULL, 0x3e8000, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
        if (fb == MAP_FAILED) {
            printf("MPROT mmap fb failed (skip)\n");
        } else {
            errno = 0;
            t("middle-of-fb RW->RW(same)", mprotect(fb + 0x100000, 0x100000, PROT_READ | PROT_WRITE) == 0 ? 0 : -1);
            errno = 0;
            t("middle-of-fb RW->RO", mprotect(fb + 0x100000, 0x80000, PROT_READ) == 0 ? 0 : -1);
            errno = 0;
            t("middle-of-fb RO->RW", mprotect(fb + 0x100000, 0x80000, PROT_READ | PROT_WRITE) == 0 ? 0 : -1);
        }
    }

    printf("MPROT-DONE fails=%d\n", fails);
    return fails;
}
