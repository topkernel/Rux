/*
 * OH-lite service stand-ins — one binary, dispatched by argv[0] (installed
 * as /bin/svc_binder, /bin/svc_ashmem, /bin/svc_tokenid via symlinks, so
 * ext4 symlink-exec is exercised too).
 *
 * svc_binder   — samgr stand-in: open /dev/binder, mmap the binder zone,
 *                BINDER_VERSION==8, BINDER_SET_MAX_THREADS,
 *                BINDER_SET_CONTEXT_MGR, BC_ENTER_LOOPER via one
 *                write-only BINDER_WRITE_READ, then heartbeat holding the
 *                fd+map (a parked samgr listener loop).
 * svc_ashmem   — open /dev/ashmem, SET_NAME/SET_SIZE/GET_SIZE, mmap rw,
 *                write/read pattern, PIN, GET_PIN_STATUS, heartbeat.
 * svc_tokenid  — open /dev/access_token_id, GET/SET/GET_FTOKENID ioctls,
 *                heartbeat.
 *
 * All ioctls report failures loudly but never abort the service: the gate
 * is that services exec and stay up (samgr-class exec = next milestone);
 * per-ioctl bugs become kernel fix tasks from the log lines.
 */

#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <unistd.h>

/* ---- binder UAPI (linux/android/binder.h subset, hand-rolled) ---- */
struct binder_write_read {
    int32_t write_size, write_consumed;
    uint8_t write_buffer[0]; /* userspace pointer in real UAPI */
};
/* The real struct uses pointers; use the canonical layout: */
struct bwr_real {
    int32_t write_size;
    int32_t write_consumed;
    uint64_t write_buffer;
    int32_t read_size;
    int32_t read_consumed;
    uint64_t read_buffer;
};
#define BINDER_WRITE_READ _IOWR('b', 1, struct bwr_real)
#define BINDER_SET_MAX_THREADS _IOW('b', 5, int32_t)
#define BINDER_SET_CONTEXT_MGR _IOW('b', 7, int32_t)
#define BINDER_VERSION _IOWR('b', 9, struct { int32_t protocol_version; })
#define BC_ENTER_LOOPER 0x46104606u
#define BR_NOOP 0x00004c12u

/* ---- ashmem UAPI ---- */
#define ASHMEM_SET_NAME 0x41007701
#define ASHMEM_GET_NAME 0x81007702
#define ASHMEM_SET_SIZE 0x40087703
#define ASHMEM_GET_SIZE 0x00007704
#define ASHMEM_PIN 0x40087707
#define ASHMEM_UNPIN 0x40087708
#define ASHMEM_GET_PIN_STATUS 0x00007709

/* ---- access_tokenid UAPI ---- */
#define ACCESS_TOKENID_GET_TOKENID 0x80084101
#define ACCESS_TOKENID_SET_TOKENID 0x40084102
#define ACCESS_TOKENID_GET_FTOKENID 0x80084103
#define ACCESS_TOKENID_SET_FTOKENID 0x40084104

static void slog(const char *fmt, ...)
{
    char buf[512];
    va_list ap;
    va_start(ap, fmt);
    int n = vsnprintf(buf, sizeof(buf), fmt, ap);
    va_end(ap);
    if (n > 0) {
        buf[n] = '\0';
        write(1, "OHLITE SVC: ", 12);
        write(1, buf, n);
        write(1, "\n", 1);
    }
}

static const char *myname(void)
{
    const char *a0 = program_invocation_short_name;
    /* glibc sets it from argv[0]; fall back to /proc/self/exe-ish guess */
    return a0 ? a0 : "svc";
}

static void heartbeat(const char *tag, int fd, void *map)
{
    for (int i = 0;; i++) {
        sleep(10);
        slog("%s alive beat %d fd %d map %p errno-last %d", tag, i, fd, map,
             errno);
    }
}

static int run_binder(void)
{
    int fd = open("/dev/binder", O_RDWR | O_CLOEXEC);
    if (fd < 0) {
        slog("binder open /dev/binder FAILED: %s", strerror(errno));
        heartbeat("binder(open-failed)", -1, NULL);
        return 1;
    }
    size_t sz = 1u << 20;
    void *m = mmap(NULL, sz, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    if (m == MAP_FAILED)
        slog("binder mmap FAILED: %s (continuing)", strerror(errno));
    else
        slog("binder mmap 1MiB ok at %p", m);

    int32_t ver = -1;
    if (ioctl(fd, BINDER_VERSION, &ver) == 0)
        slog("binder BINDER_VERSION=%d (%s)", ver, ver == 8 ? "ok" : "BAD");
    else
        slog("binder BINDER_VERSION FAILED: %s", strerror(errno));

    int32_t maxt = 4;
    if (ioctl(fd, BINDER_SET_MAX_THREADS, &maxt) == 0)
        slog("binder SET_MAX_THREADS ok");
    else
        slog("binder SET_MAX_THREADS FAILED: %s", strerror(errno));

    int32_t zero = 0;
    if (ioctl(fd, BINDER_SET_CONTEXT_MGR, &zero) == 0)
        slog("binder SET_CONTEXT_MGR ok (samgr role)");
    else
        slog("binder SET_CONTEXT_MGR FAILED: %s (may be taken)", strerror(errno));

    /* BC_ENTER_LOOPER via one write-only BWR */
    uint32_t bc = BC_ENTER_LOOPER;
    struct bwr_real bwr;
    memset(&bwr, 0, sizeof(bwr));
    bwr.write_size = 4;
    bwr.write_buffer = (uint64_t)&bc;
    if (ioctl(fd, BINDER_WRITE_READ, &bwr) == 0)
        slog("binder ENTER_LOOPER ok (samgr-lite parked in listen)");
    else
        slog("binder ENTER_LOOPER FAILED: %s", strerror(errno));

    heartbeat("binder(samgr-lite)", fd, m == MAP_FAILED ? NULL : m);
    return 0;
}

static int run_ashmem(void)
{
    int fd = open("/dev/ashmem", O_RDWR | O_CLOEXEC);
    if (fd < 0) {
        slog("ashmem open FAILED: %s", strerror(errno));
        heartbeat("ashmem(open-failed)", -1, NULL);
        return 1;
    }
    char name[256] = "oh-lite-shm";
    if (ioctl(fd, ASHMEM_SET_NAME, name) == 0)
        slog("ashmem SET_NAME ok");
    else
        slog("ashmem SET_NAME FAILED: %s", strerror(errno));

    size_t sz = 1u << 20;
    if (ioctl(fd, ASHMEM_SET_SIZE, &sz) == 0)
        slog("ashmem SET_SIZE 1MiB ok");
    else
        slog("ashmem SET_SIZE FAILED: %s", strerror(errno));

    long g = ioctl(fd, ASHMEM_GET_SIZE, 0);
    slog("ashmem GET_SIZE -> %ld (expect %u)", g, 1u << 20);

    void *m = mmap(NULL, sz, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    if (m == MAP_FAILED)
        slog("ashmem mmap FAILED: %s (continuing)", strerror(errno));
    else {
        memset(m, 0xA5, 4096);
        int ok = ((unsigned char *)m)[0] == 0xA5 && ((unsigned char *)m)[4095] == 0xA5;
        slog("ashmem mmap+r/w %s", ok ? "ok" : "DATA MISMATCH");
    }

    struct { uint32_t offset, len; } pin = {0, 4096};
    long pr = ioctl(fd, ASHMEM_PIN, &pin);
    slog("ashmem PIN -> %ld (0=NOT_PURGED)", pr);
    long ps = ioctl(fd, ASHMEM_GET_PIN_STATUS, 0);
    slog("ashmem GET_PIN_STATUS -> %ld", ps);

    heartbeat("ashmem", fd, m == MAP_FAILED ? NULL : m);
    return 0;
}

static int run_tokenid(void)
{
    int fd = open("/dev/access_token_id", O_RDWR | O_CLOEXEC);
    if (fd < 0) {
        slog("tokenid open FAILED: %s", strerror(errno));
        heartbeat("tokenid(open-failed)", -1, NULL);
        return 1;
    }
    uint64_t tok = 0;
    if (ioctl(fd, ACCESS_TOKENID_GET_TOKENID, &tok) == 0)
        slog("tokenid GET_TOKENID -> 0x%llx",
             (unsigned long long)tok);
    else
        slog("tokenid GET_TOKENID FAILED: %s", strerror(errno));

    uint64_t nt = tok ? tok + 1 : 0x10001;
    if (ioctl(fd, ACCESS_TOKENID_SET_TOKENID, &nt) == 0)
        slog("tokenid SET_TOKENID 0x%llx ok", (unsigned long long)nt);
    else
        slog("tokenid SET_TOKENID FAILED: %s", strerror(errno));

    uint64_t ftok = 0;
    if (ioctl(fd, ACCESS_TOKENID_GET_FTOKENID, &ftok) == 0)
        slog("tokenid GET_FTOKENID -> 0x%llx", (unsigned long long)ftok);
    else
        slog("tokenid GET_FTOKENID FAILED: %s", strerror(errno));

    heartbeat("tokenid", fd, NULL);
    return 0;
}

int main(int argc, char **argv)
{
    const char *n = argc > 0 && argv[0] ? argv[0] : "svc";
    const char *base = strrchr(n, '/');
    base = base ? base + 1 : n;
    slog("service exec ok (%s, pid %d)", base, (int)getpid());
    if (!strcmp(base, "svc_binder"))
        return run_binder();
    if (!strcmp(base, "svc_ashmem"))
        return run_ashmem();
    if (!strcmp(base, "svc_tokenid"))
        return run_tokenid();
    slog("unknown service name %s — parking", base);
    heartbeat(base, -1, NULL);
    return 0;
}
