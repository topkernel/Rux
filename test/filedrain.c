// filedrain.c — file-mapping churn probe: the fake-OOM drainer's
// minimal reproducer (wildwrite session 24; riscv64 A/B for the
// filemap_fault port).
//
// Rounds of: mmap a large data file MAP_PRIVATE, read-touch EVERY page
// (post-port: pins the page-cache frames, one shared copy for all
// rounds; pre-port: allocates an EXCLUSIVE unflagged private copy per
// fault — the session-22 drain), keep the mapping alive, verify the
// content checksum. The final round forks children that read the
// mapping and exit (fork pin duplication + teardown under load).
//
// Expected: post-port kernel completes ALL rounds with matching
// checksums; the pre-port kernel hits the fake OOM (pagefault: Out of
// memory / oom-killer / MEMLEDGER OOM dump) partway through on -m 2G.

#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/wait.h>
#include <unistd.h>

#define PAGE 4096U
#define ROUNDS 10U

static unsigned long checksum(const uint8_t *p, size_t len) {
    unsigned long h = 1469598103934665603UL;
    for (size_t i = 0; i < len; i += PAGE) {
        h ^= p[i] | ((unsigned long)p[i + 7] << 8);
        h *= 1099511628211UL;
    }
    return h;
}

int main(void) {
    printf("filedrain: pid=%d\n", getpid());
    int fd = open("/data.bin", O_RDONLY);
    if (fd < 0) { printf("FDR-FAIL open\n"); return 1; }
    off_t sz = lseek(fd, 0, SEEK_END);
    if (sz < 0) { printf("FDR-FAIL lseek\n"); return 1; }
    printf("filedrain: data file %ld bytes\n", (long)sz);

    static uint8_t *maps[ROUNDS];
    unsigned long ref = 0;
    int ref_set = 0;

    for (unsigned r = 0; r < ROUNDS; r++) {
        uint8_t *p = mmap(NULL, sz, PROT_READ, MAP_PRIVATE, fd, 0);
        if (p == MAP_FAILED) {
            printf("FDR-FAIL mmap round %u\n", r);
            return 1;
        }
        maps[r] = p;
        volatile uint8_t sink = 0;
        for (off_t o = 0; o < sz; o += PAGE) sink ^= p[o];
        (void)sink;
        unsigned long c = checksum(p, (size_t)sz);
        if (!ref_set) { ref = c; ref_set = 1; }
        if (c != ref) {
            printf("FDR-FAIL checksum round %u\n", r);
            return 1;
        }
        printf("FDR-ROUND %u ok sum=%lx\n", r, c);
    }

    // fork leg: children read the first mapping and exit (pin
    // duplication in the fork walk + teardown of the copies).
    for (int k = 0; k < 3; k++) {
        pid_t pid = fork();
        if (pid == 0) {
            unsigned long c = checksum(maps[0], (size_t)sz);
            write(1, "FDR child read done\n", 20);
            _exit(c == ref ? 0 : 3);
        }
        int st = 0;
        waitpid(pid, &st, 0);
        if (!WIFEXITED(st) || WEXITSTATUS(st) != 0) {
            printf("FDR-FAIL child %d\n", k);
            return 1;
        }
    }
    printf("FDR-CHILDREN ok\n");

    for (unsigned r = 0; r < ROUNDS; r++) munmap(maps[r], sz);
    printf("FDR-PASS\n");
    return 0;
}
