/*
 * ueventd-style netlink listener probe for the Rux initrd boot test
 * (OH Phase 1 prereq, R7): binds NETLINK_KOBJECT_UEVENT with
 * nl_groups=0xffffffff like OH's ueventd (ueventd_socket.c), sets
 * SO_PASSCRED, then triggers a uevent via /sys/bus/pci/rescan (hotplug
 * virtio-blk) and verifies:
 *   1. recvmsg returns the uevent payload ("add@...\0ACTION=...\0...")
 *   2. an SCM_CREDENTIALS cmsg is attached (pid=0/uid=0/gid=0 — kernel
 *      sender; without it OH ueventd DROPS the message)
 *   3. the payload carries DEVPATH/SUBSYSTEM/DEVNAME/MAJOR/MINOR/DEVTYPE
 *
 * OH Phase 1b (gap 3, /devices-shaped DEVPATH): the block uevent's
 * DEVPATH must start with "/devices" (ueventd
 * GetBlockDeviceSymbolLinks bails otherwise — no /dev/block/by-name
 * symlinks are ever created), and walking up from /sys<DEVPATH> must
 * reach a parent whose `subsystem` symlink resolves to /sys/bus/platform
 * (exactly ueventd's algorithm: FindPlatformDeviceName then
 * /dev/block/platform/<parent>/by-name/<PARTNAME>). The probe replicates
 * the walk: readlink each ancestor's subsystem, resolve the relative
 * target lexically, compare.
 *
 * Freestanding, no libc.
 */

#define AF_NETLINK     16
#define SOCK_DGRAM     2
#define NETLINK_KOBJECT_UEVENT 15
#define SOL_SOCKET     1
#define SO_PASSCRED    16
#define SCM_CREDENTIALS 2

struct sockaddr_nl { unsigned short nl_family; unsigned short nl_pad; unsigned int nl_pid; unsigned int nl_groups; };

static long sys3(long n, long a, long b, long c)
{
    register long a0 __asm__("a0") = a;
    register long a1 __asm__("a1") = b;
    register long a2 __asm__("a2") = c;
    register long a7 __asm__("a7") = n;
    __asm__ volatile("ecall" : "+r"(a0) : "r"(a1), "r"(a2), "r"(a7) : "memory");
    return a0;
}

static long sys5(long n, long a, long b, long c, long d, long e)
{
    register long a0 __asm__("a0") = a;
    register long a1 __asm__("a1") = b;
    register long a2 __asm__("a2") = c;
    register long a3 __asm__("a3") = d;
    register long a4 __asm__("a4") = e;
    register long a7 __asm__("a7") = n;
    __asm__ volatile("ecall" : "+r"(a0)
                     : "r"(a1), "r"(a2), "r"(a3), "r"(a4), "r"(a7)
                     : "memory");
    return a0;
}

static long sys6(long n, long a, long b, long c, long d, long e, long f)
{
    register long a0 __asm__("a0") = a;
    register long a1 __asm__("a1") = b;
    register long a2 __asm__("a2") = c;
    register long a3 __asm__("a3") = d;
    register long a4 __asm__("a4") = e;
    register long a5 __asm__("a5") = f;
    register long a7 __asm__("a7") = n;
    __asm__ volatile("ecall" : "+r"(a0)
                     : "r"(a1), "r"(a2), "r"(a3), "r"(a4), "r"(a5), "r"(a7)
                     : "memory");
    return a0;
}

static unsigned slen(const char *s) { unsigned n = 0; while (s[n]) n++; return n; }

/* gcc may emit memcpy for the struct/loop copies; freestanding has none. */
void *memcpy(void *dst, const void *src, unsigned long n)
{
    unsigned char *d = dst;
    const unsigned char *s = src;
    while (n--) *d++ = *s++;
    return dst;
}
static void puts_(const char *s) { sys3(64, 1, (long)s, slen(s)); }

static void put_dec_(long v)
{
    char b[24];
    int i = 23;
    if (v == 0) b[--i] = '0';
    while (v > 0) { b[--i] = '0' + (v % 10); v /= 10; }
    sys3(64, 1, (long)(b + i), 24 - i);
}

static int has_field(const char *payload, long len, const char *key)
{
    unsigned klen = slen(key);
    long pos = 0;
    while (pos < len) {
        const char *e = payload + pos;
        unsigned l = slen(e);
        if (l > klen) {
            int eq = 1;
            for (unsigned i = 0; i < klen; i++)
                if (e[i] != key[i]) { eq = 0; break; }
            if (eq && e[klen] == '=') return 1;
        }
        if (l == 0) break;
        pos += l + 1;
    }
    return 0;
}

/*
 * ---- OH Phase 1b: ueventd by-name walk replication ----
 */

static long sys4_78_readlinkat(const char *path, char *buf, unsigned sz);

/* Copy the value of KEY= from the uevent payload into out. 1 on hit. */
static int copy_field(const char *payload, long len, const char *key, char *out, unsigned outsz)
{
    unsigned klen = slen(key);
    long pos = 0;
    while (pos < len) {
        const char *e = payload + pos;
        unsigned l = slen(e);
        if (l > klen) {
            int eq = 1;
            for (unsigned i = 0; i < klen; i++)
                if (e[i] != key[i]) { eq = 0; break; }
            if (eq && e[klen] == '=') {
                unsigned v = 0;
                while (v < outsz - 1 && klen + 1 + v < l) {
                    out[v] = e[klen + 1 + v];
                    v++;
                }
                out[v] = 0;
                return 1;
            }
        }
        if (l == 0) break;
        pos += l + 1;
    }
    return 0;
}

/* dirname(3): strip the last component of a path in place-ish (dst). */
static void dirname_of(const char *path, char *dst, unsigned dstsz)
{
    unsigned n = slen(path);
    if (n == 0 || (n == 1 && path[0] == '/')) { dst[0] = '/'; dst[1] = 0; return; }
    while (n > 1 && path[n - 1] == '/') n--;  /* trailing slashes */
    while (n > 0 && path[n - 1] != '/') n--;  /* last component */
    if (n == 0) { dst[0] = '/'; dst[1] = 0; return; }
    unsigned m = n > dstsz - 1 ? dstsz - 1 : n;
    for (unsigned i = 0; i < m; i++) dst[i] = path[i];
    if (m > 1 && dst[m - 1] == '/') m--;
    dst[m] = 0;
}

/* Lexically resolve `target` (may be relative) against directory `base`
 * into `out`. Handles "../" and "./" segments. */
static void resolve_rel(const char *base, const char *target, char *out, unsigned outsz)
{
    unsigned o = 0;
    /* copy base */
    for (unsigned i = 0; base[i] && o < outsz - 1; i++) out[o++] = base[i];
    out[o] = 0;

    unsigned t = 0;
    if (target[0] == '/') { o = 0; out[o] = 0; t = 1; }
    while (target[t]) {
        /* extract one segment */
        unsigned seglen = 0;
        while (target[t + seglen] && target[t + seglen] != '/') seglen++;
        if (seglen == 1 && target[t] == '.') {
            /* skip */
        } else if (seglen == 2 && target[t] == '.' && target[t + 1] == '.') {
            /* drop the last out segment */
            while (o > 0 && out[o - 1] != '/') o--;
            if (o > 0) o--; /* the slash */
        } else if (seglen > 0) {
            if (o > 0 && out[o - 1] != '/' && o < outsz - 1) out[o++] = '/';
            for (unsigned i = 0; i < seglen && o < outsz - 1; i++) out[o++] = target[t + i];
        }
        t += seglen;
        while (target[t] == '/') t++;
    }
    if (o == 0) { out[0] = '/'; o = 1; }
    out[o] = 0;
}

static int streq(const char *a, const char *b)
{
    unsigned i = 0;
    while (a[i] && a[i] == b[i]) i++;
    return a[i] == 0 && b[i] == 0;
}

/*
 * ueventd GetBlockDeviceSymbolLinks walk: from /sys<DEVPATH> climb the
 * parents; a parent whose `subsystem` readlink resolves to
 * /sys/bus/platform yields the platform device name (the component after
 * /sys/devices/platform/) — the <parent> of
 * /dev/block/platform/<parent>/by-name/<partition>.
 * Returns 1 and fills plat[] when found.
 */
static int ueventd_platform_walk(const char *devpath, char *plat, unsigned platsz)
{
    char syspath[256];
    char parent[256];
    char linkpath[280];
    char target[128];
    char resolved[256];

    /* syspath = "/sys" + devpath */
    syspath[0] = '/'; syspath[1] = 's'; syspath[2] = 'y'; syspath[3] = 's';
    unsigned dl = slen(devpath);
    if (dl > 250) return 0;
    for (unsigned i = 0; i < dl; i++) syspath[4 + i] = devpath[i];
    syspath[4 + dl] = 0;

    dirname_of(syspath, parent, sizeof(parent));
    while (slen(parent) > 4) { /* stop at "/sys" */
        /* linkpath = parent + "/subsystem" */
        unsigned pl = slen(parent);
        if (pl + 11 >= sizeof(linkpath)) return 0;
        for (unsigned i = 0; i < pl; i++) linkpath[i] = parent[i];
        linkpath[pl] = '/';
        const char *sub = "subsystem";
        for (unsigned i = 0; i <= slen(sub); i++) linkpath[pl + 1 + i] = sub[i];

        long n = sys4_78_readlinkat(linkpath, target, sizeof(target) - 1);
        if (n > 0) {
            target[n] = 0;
            resolve_rel(parent, target, resolved, sizeof(resolved));
            if (streq(resolved, "/sys/bus/platform")) {
                /* parent == /sys/devices/platform/<name>[...] */
                const char *pfx = "/sys/devices/platform/";
                unsigned i = 0;
                while (pfx[i] && parent[i] == pfx[i]) i++;
                if (pfx[i] == 0 && parent[i] && parent[i] != '/') {
                    unsigned j = 0;
                    while (parent[i + j] && parent[i + j] != '/' && j < platsz - 1) {
                        plat[j] = parent[i + j];
                        j++;
                    }
                    plat[j] = 0;
                    return 1;
                }
                /* deeper path under platform/ (not the platform device
                 * itself) — ueventd FindPlatformDeviceName returns the
                 * whole remainder; treat the first component as the
                 * device name, same as the /dev/block/platform/<parent>
                 * link shape. */
                return 0;
            }
        }
        char next[256];
        dirname_of(parent, next, sizeof(next));
        for (unsigned i = 0; i < sizeof(next); i++) parent[i] = next[i];
    }
    return 0;
}

/* readlinkat(AT_FDCWD, path, buf, sz) wrapper. */
static long sys4_78_readlinkat(const char *path, char *buf, unsigned sz)
{
    register long a0 __asm__("a0") = -100;
    register long a1 __asm__("a1") = (long)path;
    register long a2 __asm__("a2") = (long)buf;
    register long a3 __asm__("a3") = (long)sz;
    register long a7 __asm__("a7") = 78;
    __asm__ volatile("ecall" : "+r"(a0) : "r"(a1), "r"(a2), "r"(a3), "r"(a7) : "memory");
    return a0;
}

void _start(void)
{
    puts_("UEV: start\n");

    long fd = sys3(198 /*socket*/, AF_NETLINK, SOCK_DGRAM, NETLINK_KOBJECT_UEVENT);
    if (fd < 0) { puts_("UEV: socket FAILED\n"); sys3(93, 1, 0, 0); }

    int one = 1;
    long r = sys5(208 /*setsockopt*/, fd, SOL_SOCKET, SO_PASSCRED, (long)&one, 4);
    if (r != 0) { puts_("UEV: SO_PASSCRED FAILED\n"); sys3(93, 1, 0, 0); }
    puts_("UEV: SO_PASSCRED ok\n");

    struct sockaddr_nl addr;
    addr.nl_family = AF_NETLINK;
    addr.nl_pad = 0;
    addr.nl_pid = 1;            /* getpid() for PID 1 */
    addr.nl_groups = 0xffffffff;
    r = sys3(200 /*bind*/, fd, (long)&addr, sizeof(addr));
    if (r != 0) { puts_("UEV: bind FAILED\n"); sys3(93, 1, 0, 0); }
    puts_("UEV: bind ok\n");

    /* Trigger: PCI rescan creates a hotplug virtio-blk uevent. */
    long tfd = sys3(56 /*openat*/, -100, (long)"/sys/bus/pci/rescan", 1 /*O_WRONLY*/);
    if (tfd < 0) {
        puts_("UEV: open rescan FAILED\n");
    } else {
        sys3(64, tfd, (long)"1\n", 2);
        sys3(57, tfd, 0, 0);
        puts_("UEV: rescan triggered\n");
    }

    /* recvmsg loop: collect up to 8 uevents, stop at the block one. */
    for (int i = 0; i < 8; i++) {
        char buf[2048];
        char control[128];
        /* struct msghdr (lp64): name(8) namelen(4+4) iov(8) iovlen(8)
         * control(8) controllen(8) flags(4+4) = 56 bytes.
         * struct iovec: base(8) len(8). */
        long iov[2] = { (long)buf, (long)sizeof(buf) };
        unsigned char mh[56];
        for (int k = 0; k < 56; k++) mh[k] = 0;
        *(long *)(mh + 16) = (long)iov;     /* msg_iov */
        *(long *)(mh + 24) = 1;             /* msg_iovlen */
        *(long *)(mh + 32) = (long)control; /* msg_control */
        *(long *)(mh + 40) = sizeof(control);
        long n = sys6(212 /*recvmsg*/, fd, (long)mh, 0, 0, 0, 0);
        if (n <= 0) { puts_("UEV: recvmsg end\n"); break; }

        long controllen = *(long *)(mh + 40);
        puts_("UEV: got uevent len=");
        put_dec_(n);
        puts_(" cmsg_bytes=");
        put_dec_(controllen);
        puts_("\nUEV: payload: ");
        sys3(64, 1, (long)buf, n);
        puts_("\n");

        /* Walk the cmsg chain: expect SCM_CREDENTIALS with {0,0,0}. */
        int creds_ok = 0;
        long off = 0;
        while (off + 16 <= controllen) {
            long cmsg_len = *(long *)(control + off);
            if (cmsg_len < 16 || off + cmsg_len > controllen) break;
            int level = *(int *)(control + off + 8);
            int type = *(int *)(control + off + 12);
            if (level == SOL_SOCKET && type == SCM_CREDENTIALS && cmsg_len >= 28) {
                int pid = *(int *)(control + off + 16);
                unsigned uid = *(unsigned *)(control + off + 20);
                unsigned gid = *(unsigned *)(control + off + 24);
                if (pid == 0 && uid == 0 && gid == 0) creds_ok = 1;
            }
            off += (cmsg_len + 7) & ~7;
        }
        puts_(creds_ok ? "UEV: SCM_CREDENTIALS {0,0,0} PRESENT\n"
                       : "UEV: SCM_CREDENTIALS MISSING\n");

        if (has_field(buf, n, "SUBSYSTEM") && has_field(buf, n, "DEVNAME")
            && has_field(buf, n, "DEVTYPE") && has_field(buf, n, "MAJOR")) {
            puts_("UEV: block payload fields ok\n");

            /* ---- Phase 1b checks: /devices-shaped DEVPATH + walk ---- */
            char devpath[192];
            if (!copy_field(buf, n, "DEVPATH", devpath, sizeof(devpath))) {
                puts_("UEV: no DEVPATH field\n");
                continue;
            }
            puts_("UEV: DEVPATH=");
            puts_(devpath);
            puts_("\n");
            if (devpath[0] == '/' && devpath[1] == 'd' && devpath[2] == 'e'
                && devpath[3] == 'v' && devpath[4] == 'i' && devpath[5] == 'c'
                && devpath[6] == 'e' && devpath[7] == 's' && devpath[8] == '/') {
                puts_("UEV: DEVPATH /devices-shaped ok\n");
            } else {
                puts_("UEV: DEVPATH NOT /devices-shaped (by-name walk dead)\n");
            }
            char plat[64];
            if (ueventd_platform_walk(devpath, plat, sizeof(plat))) {
                puts_("UEV: platform ancestor found: ");
                puts_(plat);
                puts_(" (by-name symlink creatable)\n");
                puts_("UEV: PASS\n");
                sys3(93, 0, 0, 0);
            } else {
                puts_("UEV: no platform ancestor via subsystem links\n");
            }
        }
    }
    puts_("UEV: done\n");
    sys3(93, 0, 0, 0);
    __builtin_unreachable();
}
