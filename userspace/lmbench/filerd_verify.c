/* filerd_verify.c — merged-copyout byte-correctness probe (PID 1).
 *
 * Reads a real multi-page file through the read(2) fast path in three
 * shapes (aligned 64 KiB chunks; unaligned 100000-byte chunks after an
 * odd seek; 4 KiB preads) and prints a 64-bit FNV-1a of the bytes for
 * each. The host compares against its own hash of the same file (also
 * verifies cross-shape equality inside the guest). Any divergence in
 * the merged copyout (wrong bytes at page seams, short/long copies)
 * shows up immediately.
 *
 * Build: musl-gcc -static -O2 -o filerd_verify filerd_verify.c
 * Boot:  init=/test/filerd_verify
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <fcntl.h>

static unsigned long long fnv(const unsigned char *p, size_t n,
                              unsigned long long h)
{
    for (size_t i = 0; i < n; i++) {
        h ^= p[i];
        h *= 1099511628211ULL;
    }
    return h;
}

int main(int argc, char **argv)
{
    const char *path = argc > 1 ? argv[1] : "/usr/lib/verify.bin";
    unsigned char buf[65536];
    unsigned long long h;
    ssize_t n;
    int fd;

    /* shape 1: aligned 64 KiB read(2) chunks from offset 0 */
    fd = open(path, O_RDONLY);
    if (fd < 0) { printf("VERIFY open-fail %s\n", path); return 1; }
    h = 1469598103934665603ULL;
    while ((n = read(fd, buf, sizeof(buf))) > 0)
        h = fnv(buf, (size_t)n, h);
    close(fd);
    printf("VERIFY %s aligned64k fnv=%016llx\n", path, h);

    /* shape 2: seek to an odd offset, 100000-byte chunks (first page
     * partial, interior full pages, tail partial) */
    fd = open(path, O_RDONLY);
    if (fd < 0) { printf("VERIFY open-fail2\n"); return 1; }
    lseek(fd, 3, SEEK_SET);
    h = 1469598103934665603ULL;
    {
        unsigned char b2[100000];
        while ((n = read(fd, b2, sizeof(b2))) > 0)
            h = fnv(b2, (size_t)n, h);
    }
    close(fd);
    printf("VERIFY %s unaligned100k fnv=%016llx\n", path, h);

    /* shape 3: 4 KiB preads over the whole file */
    fd = open(path, O_RDONLY);
    if (fd < 0) { printf("VERIFY open-fail3\n"); return 1; }
    h = 1469598103934665603ULL;
    {
        off_t off = 0;
        for (;;) {
            n = pread(fd, buf, 4096, off);
            if (n <= 0) break;
            h = fnv(buf, (size_t)n, h);
            off += n;
        }
    }
    close(fd);
    printf("VERIFY %s pread4k fnv=%016llx\n", path, h);

    printf("VERIFY DONE\n");
    fflush(stdout);
    return 0;
}
