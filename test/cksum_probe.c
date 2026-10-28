/* cksum_probe.c — read a file block-by-block, FNV-1a each 4KB, print
 * per-chunk sums and the first 16 bytes of blocks whose content looks
 * suspicious (we print everything; host side diffs against the image).
 * Keep the binary tiny so its own text stays in the first pages.
 */
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <unistd.h>

static uint64_t fnv1a(const unsigned char *p, unsigned n, uint64_t h)
{
    for (unsigned i = 0; i < n; i++) {
        h ^= p[i];
        h *= 1099511628211ULL;
    }
    return h;
}

int main(int argc, char **argv)
{
    const char *path = argc > 1 ? argv[1] : "/bin/mrsh";
    int fd = open(path, O_RDONLY);
    if (fd < 0) {
        printf("open %s failed %d\n", path, fd);
        return 1;
    }
    unsigned char buf[4096];
    long blk = 0;
    ssize_t n;
    uint64_t h = 1469598103934665603ULL;
    printf("CKSUM %s start\n", path);
    while ((n = read(fd, buf, sizeof(buf))) > 0) {
        uint64_t bh = fnv1a(buf, (unsigned)n, 1469598103934665603ULL);
        printf("B%ld:%016llx\n", blk, (unsigned long long)bh);
        /* dump first 16 bytes every 16 blocks for spot checks */
        if (blk % 16 == 0) {
            printf("D%ld:", blk);
            for (int i = 0; i < 16; i++)
                printf("%02x", buf[i]);
            printf("\n");
        }
        h = fnv1a(buf, (unsigned)n, h);
        blk++;
    }
    printf("CKSUM %s blocks=%ld total=%016llx end\n", path, blk,
           (unsigned long long)h);
    return 0;
}
