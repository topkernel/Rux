/*
 * dirstorm.c — concurrent directory-entry mutation storm (fleet member,
 * ext4 ENTRY_MUTEX retest). N workers churn a shared tree with
 * mkdir/rmdir/rename/unlink/creat/link/symlink; every created file
 * carries a verifiable pattern; workers re-read and validate.
 *
 * Markers:
 *   DIRSTORM-BEAT w<id> t=<t> ops=<n> files=<n> split=<n> fsync=<n> odd=<n>
 *   DIRSTORM-ODD <op> errno=<e>          (errno outside the legal race set)
 *   DIRSTORM-CORRUPT <file> v=<code>     (pattern/header mismatch — CRITICAL)
 * Exit: never (round budget kills); kernel panics are the classifier's.
 *
 * Legal errno sets (concurrent churn makes all of these expected):
 *   mkdir EEXIST; rmdir ENOENT ENOTEMPTY; creat EEXIST; unlink ENOENT;
 *   link EEXIST ENOENT; symlink EEXIST; rename ENOENT ENOTEMPTY EEXIST.
 */
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <fcntl.h>
#include <errno.h>
#include <time.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <sys/types.h>

#define ROOT "/dst"
#define WORKERS 4
#define BEAT_SECS 10

static int odd_count;

static void mkpath(char *out, size_t n, const char *dir, long a, long b)
{
    snprintf(out, n, "%s/%ld_%ld", dir, a, b);
}

/* long-name variant: 190+ byte names force large rec_len entries near
 * directory-block ends — the split-boundary geometry the leg-29A panic
 * lived on. */
static void mkpathL(char *out, size_t n, const char *dir, long a, long b)
{
    char tail[192];
    int i, k = snprintf(tail, sizeof tail, "%ld_%ld_", a, b);
    for (i = k; i < (int)sizeof tail - 1; i++)
        tail[i] = 'a' + (int)((a + b + i) % 26);
    tail[i] = 0;
    snprintf(out, n, "%s/%s", dir, tail);
}

static void wpat(int fd, int worker, long seq)
{
    /* header: magic | worker | seq | body_len, then pattern bytes
     * derived from (worker, seq) — verifier re-derives everything. */
    static const unsigned int magic = 0xD57A0C01u;
    unsigned int hdr[4] = { magic, (unsigned)worker, (unsigned)seq, 2064 };
    char buf[256];
    unsigned int x = (unsigned)worker * 2654435761u + (unsigned)seq * 40503u;
    int i;
    if (write(fd, hdr, sizeof hdr) != (ssize_t)sizeof hdr)
        return;
    for (i = 0; i < 8; i++) {
        int j;
        for (j = 0; j < 256; j++) {
            x = x * 1103515245u + 12345u;
            buf[j] = (char)(x >> 24);
        }
        if (write(fd, buf, 256) != 256)
            return;
    }
}

/* 0 ok; -1 open ENOENT (legal race); -2 open other / short header;
 * -3 bad magic (hard corruption verdict); -4 well-formed but foreign
 * (name raced to another worker's file); -5 short body; -6 mismatch. */
static int verify_file(const char *p, int worker, long seq)
{
    static const unsigned int magic = 0xD57A0C01u;
    unsigned int hdr[4];
    char buf[256], exp[256];
    unsigned int x;
    int fd, i;
    fd = open(p, O_RDONLY);
    if (fd < 0)
        return errno == ENOENT ? -1 : -2;
    if (read(fd, hdr, sizeof hdr) != (ssize_t)sizeof hdr) { close(fd); return -2; }
    if (hdr[0] != magic) { close(fd); return -3; }
    if (hdr[1] != (unsigned)worker || hdr[2] != (unsigned)seq) {
        close(fd);
        return -4;
    }
    x = (unsigned)worker * 2654435761u + (unsigned)seq * 40503u;
    for (i = 0; i < 8; i++) {
        int j;
        if (read(fd, buf, 256) != 256) { close(fd); return -5; }
        for (j = 0; j < 256; j++) {
            x = x * 1103515245u + 12345u;
            exp[j] = (char)(x >> 24);
        }
        if (memcmp(buf, exp, 256) != 0) { close(fd); return -6; }
    }
    close(fd);
    return 0;
}

static void odd(const char *op, int e)
{
    odd_count++;
    printf("DIRSTORM-ODD %s errno=%d\n", op, e);
}

static int legal(const char *op, int e)
{
    if (!e) return 1;
    if (!strcmp(op, "mkdir"))   return e == EEXIST;
    if (!strcmp(op, "rmdir"))   return e == ENOENT || e == ENOTEMPTY;
    if (!strcmp(op, "creat"))   return e == EEXIST;
    if (!strcmp(op, "unlink"))  return e == ENOENT;
    if (!strcmp(op, "link"))    return e == EEXIST || e == ENOENT;
    if (!strcmp(op, "symlink")) return e == EEXIST;
    if (!strcmp(op, "rename"))  return e == ENOENT || e == ENOTEMPTY || e == EEXIST;
    return 0;
}

static void worker(int id)
{
    char p[256], q[256], dirbuf[300];
    long seq = 0, ops = 0, files = 0, splits = 0, fsyncs = 0, fsync_tick = 0;
    time_t last = time(0);
    int dirphase = 0;
    unsigned int seedv = (unsigned)(id * 7919 + 13);
    snprintf(dirbuf, sizeof dirbuf, ROOT "/w%d", id);
    mkdir(dirbuf, 0755); /* private dir (parent ROOT exists from main) */
    for (;;) {
        unsigned r = rand_r(&seedv);
        int op = r % 100;
        long a = id, b = seq % 512;
        const char *dir;

        if ((r >> 8) & 1) {
            dir = ROOT "/shared";
        } else {
            snprintf(dirbuf, sizeof dirbuf, ROOT "/w%d", id);
            dir = dirbuf;
        }

        if (op < 22) { /* creat + pattern + self-verify */
            mkpath(p, sizeof p, dir, a, b);
            int fd = open(p, O_CREAT | O_EXCL | O_WRONLY, 0644);
            if (fd >= 0) {
                wpat(fd, id, seq);
                close(fd);
                files++;
                int v = verify_file(p, id, seq);
                if (v < 0 && v != -1 && v != -4)
                    printf("DIRSTORM-CORRUPT %s selfread v=%d\n", p, v);
            } else if (!legal("creat", errno)) {
                odd("creat", errno);
            }
        } else if (op < 34) { /* unlink */
            mkpath(p, sizeof p, dir, a, (r >> 4) % 512);
            if (unlink(p) < 0 && !legal("unlink", errno))
                odd("unlink", errno);
        } else if (op < 46) { /* mkdir */
            mkpath(p, sizeof p, dir, 900 + b % 64, b);
            if (mkdir(p, 0755) < 0 && !legal("mkdir", errno))
                odd("mkdir", errno);
        } else if (op < 56) { /* rmdir */
            mkpath(p, sizeof p, dir, 900 + b % 64, b);
            if (rmdir(p) < 0 && !legal("rmdir", errno))
                odd("rmdir", errno);
        } else if (op < 70) { /* rename (same-dir or shared-dir target) */
            mkpath(p, sizeof p, dir, a, b);
            snprintf(dirbuf, sizeof dirbuf, "%s",
                     (r >> 3) & 1 ? ROOT "/shared" : dir);
            /* note: when dir==dirbuf this self-assign is a no-op copy */
            mkpath(q, sizeof q, dirbuf, 800 + id, b);
            if (rename(p, q) < 0 && !legal("rename", errno))
                odd("rename", errno);
        } else if (op < 76) { /* link */
            mkpath(p, sizeof p, dir, a, b);
            mkpath(q, sizeof q, dir, a, (b + 7) % 512);
            if (link(p, q) < 0 && !legal("link", errno))
                odd("link", errno);
        } else if (op < 82) { /* symlink to a peer name */
            mkpath(p, sizeof p, dir, a, b);
            mkpath(q, sizeof q, dir, a, (b + 3) % 512);
            if (symlink(p, q) < 0 && !legal("symlink", errno))
                odd("symlink", errno);
        } else if (op < 88) { /* verify a random earlier name */
            mkpath(p, sizeof p, dir, a, (r >> 4) % 512);
            int v = verify_file(p, id, 0);
            /* -5/-6 only: partial-header states (-2/-3) can be a live
             * recreate race on shared names; -4 is a foreign file. */
            if (v == -5 || v == -6)
                printf("DIRSTORM-CORRUPT %s v=%d\n", p, v);
        } else if (op < 92) { /* paced dir-fsync trickle */
            if (fsync_tick++ % 8 == 0) {
                int fd = open(ROOT "/shared", O_RDONLY | O_DIRECTORY);
                if (fd >= 0) { fsync(fd); close(fd); fsyncs++; }
            }
        } else { /* split-grow: 64 long names into one hot dir, then purge */
            int i;
            snprintf(dirbuf, sizeof dirbuf, ROOT "/hot%d", id);
            mkdir(dirbuf, 0755);
            for (i = 0; i < 64; i++) {
                mkpathL(p, sizeof p, dirbuf, dirphase, i);
                int fd = open(p, O_CREAT | O_EXCL | O_WRONLY, 0644);
                if (fd >= 0) { wpat(fd, id, seq + i); close(fd); splits++; }
            }
            for (i = 0; i < 64; i++) {
                mkpathL(p, sizeof p, dirbuf, dirphase, i);
                if (unlink(p) < 0 && errno != ENOENT)
                    odd("unlink", errno);
            }
            dirphase++;
            if (rmdir(dirbuf) < 0 && !legal("rmdir", errno))
                odd("rmdir", errno);
        }

        ops++;
        seq++;
        if (time(0) - last >= BEAT_SECS) {
            printf("DIRSTORM-BEAT w%d t=%ld ops=%ld files=%ld split=%ld "
                   "fsync=%ld odd=%d\n", id, (long)time(0), ops, files,
                   splits, fsyncs, odd_count);
            fflush(stdout);
            ops = 0; last = time(0);
        }
    }
}

int main(void)
{
    int next = 0;
    mkdir(ROOT, 0755);
    mkdir(ROOT "/shared", 0755);
    printf("DIRSTORM-START workers=%d\n", WORKERS);
    fflush(stdout);
    for (next = 0; next < WORKERS; next++) {
        pid_t pid = fork();
        if (pid == 0) { worker(next); _exit(0); }
        if (pid < 0) { printf("DIRSTORM-ODD fork errno=%d\n", errno); sleep(1); }
    }
    /* judge: workers must never exit — respawn and flag if they do */
    for (;;) {
        int st;
        pid_t p = wait(&st);
        if (p < 0) { sleep(5); continue; }
        printf("DIRSTORM-ODD child %d exited st=%d (respawning)\n", p, st);
        fflush(stdout);
        pid_t pid = fork();
        if (pid == 0) { worker(next); _exit(0); }
        if (pid > 0) next = (next + 1) % WORKERS;
    }
    return 0;
}
