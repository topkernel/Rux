/*
 * oomchurn.c — deterministic fake-OOM reproducer (oom-accounting lane).
 *
 * Touches anonymous memory in fine steps while printing progress, so the
 * serial log shows exactly HOW MUCH was faulted in when the allocator
 * gives up ("pagefault: Out of memory" / OOM-FORENSIC). With -m 2G the
 * kernel should be able to hand out (RAM - kernel reservations - cache)
 * before reclaim kicks in; dying at a small fraction of that is the
 * fake-OOM signature (GNOME: ~342MB of 2G).
 *
 * Modes:
 *   anon <MB>          touch MB of anonymous memory 1 page per 4, print
 *                      every 16MB, then munmap and report.
 *   fork <n> <MB>      n rounds of fork; child touches MB then exits.
 *   churn <MB> <rounds>  alloc/touch/release loop (leak detector).
 *   mapfile <MB>       mmap + read a file to fill page cache.
 *   wrstorm <MB> <rounds> [path]  write+read+fsync churn through the page
 *                      cache (OH /data first-boot storm shape): per round
 *                      create a fresh file, write MB in 64K chunks, fsync,
 *                      re-read MB (page-cache fill), close, unlink. Prints
 *                      MemFree per round so a monotonic drop across rounds
 *                      is the leak signature.
 *   mix <rounds> <MB>  fork churn where each child writes MB to a fresh
 *                      file, reads it back, and exits — fork COW + write
 *                      storm + full lifecycle teardown in one loop.
 *
 * Build: riscv64-linux-gnu-gcc -static -O2 -o oomchurn oomchurn.c
 * Any Linux-ABI static binary runs on the minimal rootfs (musl userland
 * does not matter for static binaries).
 */
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/wait.h>
#include <unistd.h>
#include <dirent.h>

#define PAGE 4096

static void setbufs(void)
{
    setvbuf(stdout, NULL, _IONBF, 0);
    setvbuf(stderr, NULL, _IONBF, 0);
}

static long touch_anon_mb(long mb, volatile unsigned char *p)
{
    long npages = mb * 1024 * 1024 / PAGE;
    long i;
    unsigned sum = 0;
    for (i = 0; i < npages; i++) {
        p[i * PAGE] = (unsigned char)i;
        sum += p[i * PAGE];
        if ((i % (16 * 1024 * 1024 / PAGE)) == (16 * 1024 * 1024 / PAGE) - 1) {
            long done = (i + 1) * PAGE / (1024 * 1024);
            printf("anon: %ld MB touched (sum=%u)\n", done, sum);
        }
    }
    return (long)sum;
}

static int mode_anon(long mb)
{
    void *p = mmap(NULL, mb * 1024 * 1024, PROT_READ | PROT_WRITE,
                   MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (p == MAP_FAILED) {
        printf("anon: mmap %ldMB failed errno=%d (%s)\n", mb, errno,
               strerror(errno));
        return 1;
    }
    printf("anon: mapped %ldMB at %p -- touching\n", mb, p);
    long s = touch_anon_mb(mb, p);
    printf("anon: ALL %ldMB touched ok (sum=%ld) -- munmap\n", mb, s);
    munmap(p, mb * 1024 * 1024);
    printf("anon: munmap ok\nOOMCHURN-ANON-PASS\n");
    return 0;
}

/*
 * fake <cacheMB> <anonMB> — the stress-fleet s17 fake-OOM shape,
 * deterministic (leg 49B): fill the page cache with CLEAN file pages
 * (write + fsync a file of cacheMB, then re-read it), then touch
 * anonMB. On a kernel whose pre-OOM reclaim is shallow, the anon touch
 * dies at a small fraction with the clean cache still resident (the
 * fake-OOM signature); on a kernel that drains before killing, the
 * touch converts to slow allocations and completes, or dies at the
 * true (RAM - kernel) ceiling.
 */
static void print_memfree(void);

static int mode_fake(long cache_mb, long anon_mb)
{
    const char *path = "/bigfile";
    long want = cache_mb * 1024 * 1024;
    int fd = open(path, O_RDWR | O_CREAT | O_TRUNC, 0644);
    if (fd < 0) {
        printf("fake: open %s failed errno=%d\n", path, errno);
        return 1;
    }
    static char buf[64 * 1024];
    memset(buf, 0x5a, sizeof(buf));
    long wrote = 0;
    while (wrote < want) {
        long n = sizeof(buf);
        if (want - wrote < n)
            n = want - wrote;
        if (write(fd, buf, n) != n) {
            printf("fake: write failed at %ldMB errno=%d\n",
                   wrote / (1024 * 1024), errno);
            close(fd);
            return 1;
        }
        wrote += n;
    }
    if (fsync(fd) != 0)
        printf("fake: fsync errno=%d (continuing)\n", errno);
    close(fd);
    printf("fake: wrote+fsynced %ldMB file -- re-reading for clean cache residency\n",
           cache_mb);

    /* Re-read the whole file: populates the page cache with clean
     * (fsynced) pages that no process holds mapped. */
    fd = open(path, O_RDONLY);
    if (fd < 0) {
        printf("fake: reopen failed errno=%d\n", errno);
        return 1;
    }
    long got = 0;
    long n;
    while ((n = read(fd, buf, sizeof(buf))) > 0)
        got += n;
    close(fd);
    printf("fake: re-read %ldMB through the cache\n", got / (1024 * 1024));
    print_memfree();

    printf("fake: touching %ldMB anon under clean-cache pressure\n", anon_mb);
    void *p = mmap(NULL, anon_mb * 1024 * 1024, PROT_READ | PROT_WRITE,
                   MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (p == MAP_FAILED) {
        printf("fake: mmap %ldMB failed errno=%d (%s)\n", anon_mb, errno,
               strerror(errno));
        return 1;
    }
    long s = touch_anon_mb(anon_mb, p);
    printf("fake: ALL %ldMB anon touched ok (sum=%ld) -- reclaim drained the clean cache\nOOMCHURN-FAKE-PASS\n",
           anon_mb, s);
    munmap(p, anon_mb * 1024 * 1024);
    return 0;
}

static void print_memfree(void)
{
    FILE *f = fopen("/proc/meminfo", "r");
    if (!f) {
        printf("meminfo: open errno=%d\n", errno);
        return;
    }
    static char ln[256];
    while (fgets(ln, sizeof(ln), f)) {
        if (strncmp(ln, "MemFree", 7) == 0 ||
            strncmp(ln, "MemTotal", 8) == 0 ||
            strncmp(ln, "HeapUsed", 8) == 0 ||
            strncmp(ln, "HeapFree", 8) == 0)
            printf("meminfo: %s", ln);
    }
    fclose(f);
}

static int mode_fork(int n, long mb)
{
    int r;
    print_memfree();
    for (r = 0; r < n; r++) {
        pid_t pid = fork();
        if (pid < 0) {
            printf("fork: round %d fork failed errno=%d (%s)\n", r, errno,
                   strerror(errno));
            return 1;
        }
        if (pid == 0) {
            void *p = mmap(NULL, mb * 1024 * 1024, PROT_READ | PROT_WRITE,
                           MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
            if (p == MAP_FAILED)
                _exit(10);
            touch_anon_mb(mb, p);
            _exit(0);
        }
        int st = 0;
        waitpid(pid, &st, 0);
        printf("fork: round %d/%d child done st=%d\n", r + 1, n, st);
        if ((r + 1) % 16 == 0)
            print_memfree();
        if (st != 0) {
            printf("OOMCHURN-FORK-FAIL round=%d status=%d\n", r + 1, st);
            return 2;
        }
    }
    printf("OOMCHURN-FORK-PASS\n");
    return 0;
}

/* fork+exec churn: each round forks; the child execve()s this same
 * binary with marker args ("exekid", mb) which touches mb anon and
 * exits. Exercises the full per-process lifecycle: fork COW, exec
 * address-space teardown/rebuild, page-table free, Task/stack free. */
static int mode_exec(int n, long mb)
{
    int r;
    print_memfree();
    for (r = 0; r < n; r++) {
        pid_t pid = fork();
        if (pid < 0) {
            printf("exec: round %d fork failed errno=%d (%s)\n", r, errno,
                   strerror(errno));
            printf("OOMCHURN-EXEC-FAIL round=%d errno=%d\n", r + 1, errno);
            return 1;
        }
        if (pid == 0) {
            char mbstr[16];
            snprintf(mbstr, sizeof(mbstr), "%ld", mb);
            char *av[] = { "oomchurn", "exekid", mbstr, NULL };
            execve("/test/oomchurn", av, NULL);
            printf("exec: round %d execve failed errno=%d (%s)\n", r, errno,
                   strerror(errno));
            _exit(20);
        }
        int st = 0;
        waitpid(pid, &st, 0);
        printf("exec: round %d/%d child st=%d\n", r + 1, n, st);
        if ((r + 1) % 16 == 0)
            print_memfree();
        if (st != 0) {
            printf("OOMCHURN-EXEC-FAIL round=%d status=%d\n", r + 1, st);
            return 2;
        }
    }
    printf("OOMCHURN-EXEC-PASS\n");
    return 0;
}

static int mode_churn(long mb, int rounds)
{
    int r;
    for (r = 0; r < rounds; r++) {
        void *p = mmap(NULL, mb * 1024 * 1024, PROT_READ | PROT_WRITE,
                       MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        if (p == MAP_FAILED) {
            printf("churn: round %d mmap failed errno=%d\n", r, errno);
            return 1;
        }
        touch_anon_mb(mb, p);
        munmap(p, mb * 1024 * 1024);
        printf("churn: round %d/%d done\n", r + 1, rounds);
    }
    printf("OOMCHURN-CHURN-PASS\n");
    return 0;
}

static int mode_mapfile(long mb)
{
    char buf[PAGE];
    const char *path = "/bigfile";
    int fd = open(path, O_RDONLY);
    if (fd < 0) {
        printf("mapfile: open %s failed errno=%d -- run dd first\n", path,
               errno);
        return 1;
    }
    long got = 0;
    long want = mb * 1024 * 1024;
    while (got < want) {
        ssize_t n = read(fd, buf, PAGE);
        if (n <= 0)
            break;
        got += n;
        buf[0] = (unsigned char)got;
        if ((got % (64L * 1024 * 1024)) == 0)
            printf("mapfile: %ld MB read\n", got / (1024 * 1024));
    }
    close(fd);
    printf("mapfile: read %ld MB total\nOOMCHURN-MAPFILE-PASS\n",
           got / (1024 * 1024));
    return 0;
}

/* One write-storm round: fresh file, write mb MB in 64K chunks, fsync,
 * re-read it back (page-cache fill + invalidate churn), close, unlink. */
static int wrstorm_round(long mb, int round, const char *path)
{
    char pathbuf[128];
    char buf[64 * 1024];
    snprintf(pathbuf, sizeof(pathbuf), "%s.%d", path, round);
    int fd = open(pathbuf, O_WRONLY | O_CREAT | O_TRUNC, 0644);
    if (fd < 0) {
        printf("wrstorm: round %d open(%s) failed errno=%d (%s)\n", round,
               pathbuf, errno, strerror(errno));
        return 1;
    }
    long want = mb * 1024 * 1024;
    long wrote = 0;
    unsigned seed = (unsigned)round * 2654435761u;
    while (wrote < want) {
        size_t chunk = sizeof(buf);
        if (want - wrote < (long)chunk)
            chunk = (size_t)(want - wrote);
        for (size_t i = 0; i < chunk; i += PAGE)
            buf[i] = (unsigned char)(seed + i);
        ssize_t n = write(fd, buf, chunk);
        if (n <= 0) {
            printf("wrstorm: round %d write failed at %ld errno=%d (%s)\n",
                   round, wrote, errno, strerror(errno));
            close(fd);
            unlink(pathbuf);
            return 1;
        }
        wrote += n;
    }
    if (fsync(fd) != 0)
        printf("wrstorm: round %d fsync errno=%d (continuing)\n", round,
               errno);
    close(fd);

    /* Read the file back through the page cache. */
    fd = open(pathbuf, O_RDONLY);
    if (fd < 0) {
        printf("wrstorm: round %d reopen failed errno=%d\n", round, errno);
        return 1;
    }
    long got = 0;
    while (got < want) {
        ssize_t n = read(fd, buf, sizeof(buf));
        if (n <= 0)
            break;
        got += n;
    }
    close(fd);
    unlink(pathbuf);
    return 0;
}

static int mode_wrstorm(long mb, int rounds, const char *path)
{
    print_memfree();
    for (int r = 0; r < rounds; r++) {
        int rc = wrstorm_round(mb, r, path);
        if (rc != 0) {
            printf("OOMCHURN-WRSTORM-FAIL round=%d\n", r + 1);
            return rc;
        }
        printf("wrstorm: round %d/%d done (%ldMB w+r)\n", r + 1, rounds, mb);
        print_memfree();
    }
    printf("OOMCHURN-WRSTORM-PASS\n");
    return 0;
}

static int mode_mix(int rounds, long mb)
{
    const char *base = "/storm/mixfile";
    print_memfree();
    for (int r = 0; r < rounds; r++) {
        pid_t pid = fork();
        if (pid < 0) {
            printf("mix: round %d fork failed errno=%d (%s)\n", r, errno,
                   strerror(errno));
            printf("OOMCHURN-MIX-FAIL round=%d errno=%d\n", r + 1, errno);
            return 1;
        }
        if (pid == 0) {
            /* child: small anon COW touch + write storm + exit */
            void *p = mmap(NULL, mb * 1024 * 1024 / 8, PROT_READ | PROT_WRITE,
                           MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
            if (p != MAP_FAILED) {
                touch_anon_mb(mb / 8, p);
                munmap(p, mb * 1024 * 1024 / 8);
            }
            int rc = wrstorm_round(mb, 1000 + r, base);
            _exit(rc);
        }
        int st = 0;
        waitpid(pid, &st, 0);
        printf("mix: round %d/%d child st=%d\n", r + 1, rounds, st);
        print_memfree();
        if (st != 0) {
            printf("OOMCHURN-MIX-FAIL round=%d status=%d\n", r + 1, st);
            return 2;
        }
    }
    printf("OOMCHURN-MIX-PASS\n");
    return 0;
}

int main(int argc, char **argv)
{
    setbufs();
    printf("oomchurn: pid=%d argc=%d\n", (int)getpid(), argc);
    /* The Rux init path gets no argv, so when run as PID 1 read the
     * argument line from /args (the boot image carries it). */
    if (argc < 2) {
        DIR *d = opendir("/test");
        if (d) {
            struct dirent *de;
            printf("oomchurn: /test contains:");
            while ((de = readdir(d)))
                printf(" %s", de->d_name);
            printf("\n");
            closedir(d);
        } else {
            printf("oomchurn: opendir(/test) errno=%d\n", errno);
        }
        FILE *f = fopen("/test/args", "r");
        static char line[256];
        if (f) {
            if (fgets(line, sizeof(line), f)) {
                char *nl = strchr(line, '\n');
                if (nl)
                    *nl = 0;
                fclose(f);
                printf("oomchurn: /test/args='%s'\n", line);
                /* re-enter main with split args */
                static char *av[8];
                int an = 1;
                static char prog[] = "oomchurn";
                av[0] = prog;
                char *tok = strtok(line, " ");
                while (tok && an < 8) {
                    av[an++] = tok;
                    tok = strtok(NULL, " ");
                }
                return main(an, av);
            }
            fclose(f);
        }
        printf("usage: oomchurn anon <MB> | fork <n> <MB> | "
               "churn <MB> <rounds> | mapfile <MB>\n");
        return 1;
    }
    if (strcmp(argv[1], "anon") == 0)
        return mode_anon(atol(argv[2]));
    if (strcmp(argv[1], "fake") == 0)
        return mode_fake(atol(argv[2]), atol(argv[3]));
    if (strcmp(argv[1], "fork") == 0)
        return mode_fork(atoi(argv[2]), atol(argv[3]));
    if (strcmp(argv[1], "churn") == 0)
        return mode_churn(atol(argv[2]), atoi(argv[3]));
    if (strcmp(argv[1], "mapfile") == 0)
        return mode_mapfile(atol(argv[2]));
    if (strcmp(argv[1], "wrstorm") == 0) {
        const char *path = argc > 4 ? argv[4] : "/storm/wrfile";
        return mode_wrstorm(atol(argv[2]), atoi(argv[3]), path);
    }
    if (strcmp(argv[1], "mix") == 0)
        return mode_mix(atoi(argv[2]), atol(argv[3]));
    if (strcmp(argv[1], "exekid") == 0)
        return mode_anon(atol(argv[2]));
    if (strcmp(argv[1], "exec") == 0)
        return mode_exec(atoi(argv[2]), atol(argv[3]));
    printf("oomchurn: unknown mode %s\n", argv[1]);
    return 1;
}
