/*
 * OH-lite init_early — the ramdisk /init stand-in for the OpenHarmony
 * Phase 1 gate, built from Ubuntu glibc instead of OH musl.
 *
 * Replicates what OH's init_early (base/startup/init/services/init/
 * standard/) does before switch_root, in the same order, so the kernel
 * side of the OH boot chain is exercised without the OH images:
 *
 *   1. mounts tmpfs/devpts/proc(hidepid=2)/sysfs/selinuxfs
 *   2. mknods /dev/{null,random,urandom,kmsg,console,ttyS0}
 *   3. ramfs read/exec stress of a large dynamic binary (the m3 frontier:
 *      OH's 538KB bin/toybox intermittently failed to read from the
 *      initramfs; here bin/bash is 1.4MB and we read+exec it repeatedly)
 *   4. netlink ueventd: NETLINK_KOBJECT_UEVENT + SO_PASSCRED, coldplug
 *      trigger of the block disks, wait for vdb/vdc uevents, mknod
 *      /dev/block/vdX from MAJOR/MINOR (ueventd_socket.c's algorithm)
 *   5. parse ohos.boot.required_mount.* tokens from /proc/cmdline (the
 *      kernel must expose them verbatim) and mount vdb->/usr vdc->/vendor
 *   6. MS_MOVE /proc /sys /dev /vendor under /usr, then switch_root:
 *      chdir("/usr"); mount("/usr","/",MS_MOVE); chroot("."); exec
 *      /bin/init --second-stage
 *
 * Every step prints an "OHLITE:" milestone line to stdout (the kernel
 * gives init the UART console) and to /dev/kmsg once it exists.
 *
 * Build: gcc -O2 -o init init_early.c  (dynamic — glibc's ld.so doing
 * open/mmap of libc.so.6 from the ramfs is exactly the kernel path OH's
 * musl ld.so takes for libsec_shared.z.so et al.)
 */

#define _GNU_SOURCE
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <linux/netlink.h>
#include <signal.h>
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mount.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/sysmacros.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

/* ---------------------------------------------------------------- logging */

static int kmsg_fd = -1;

static void ohlog(const char *fmt, ...)
{
    char buf[512];
    va_list ap;
    va_start(ap, fmt);
    int n = vsnprintf(buf, sizeof(buf), fmt, ap);
    va_end(ap);
    if (n > 0) {
        buf[n] = '\0';
        write(1, "OHLITE: ", 8);
        write(1, buf, n);
        write(1, "\n", 1);
        if (kmsg_fd >= 0)
            write(kmsg_fd, buf, n);
    }
}

static void pfatal(const char *what)
{
    ohlog("FATAL %s: %s (errno %d)", what, strerror(errno), errno);
}

/* wall time may be unset (epoch 0) in a fresh VM — use the monotonic
 * clock for all deadlines (the vdso fast path serves it) */
static long long mono_ms(void)
{
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (long long)ts.tv_sec * 1000 + ts.tv_nsec / 1000000;
}

/* probe the sysfs surface the block coldplug needs */
static void probe_sysfs(void)
{
    static const char *dirs[] = {"/sys/block", "/sys/class/block",
                                 "/sys/devices", "/sys/dev/block"};
    for (size_t i = 0; i < sizeof(dirs) / sizeof(dirs[0]); i++) {
        DIR *d = opendir(dirs[i]);
        if (!d) {
            ohlog("sysfs-probe opendir %s: %s", dirs[i], strerror(errno));
            continue;
        }
        int n = 0;
        char names[256] = "";
        struct dirent *de;
        while ((de = readdir(d)) != NULL && n < 12) {
            if (!strcmp(de->d_name, ".") || !strcmp(de->d_name, ".."))
                continue;
            strncat(names, de->d_name, sizeof(names) - strlen(names) - 2);
            strncat(names, " ", 1);
            n++;
        }
        closedir(d);
        ohlog("sysfs-probe %s: %d entries [%s]", dirs[i], n, names);
    }
    /* symlink resolution */
    static const char *links[] = {"/sys/block/vdb", "/sys/class/block/vdb"};
    for (size_t i = 0; i < sizeof(links) / sizeof(links[0]); i++) {
        char tgt[256];
        ssize_t n = readlink(links[i], tgt, sizeof(tgt) - 1);
        if (n < 0) {
            ohlog("sysfs-probe readlink %s: %s", links[i], strerror(errno));
            continue;
        }
        tgt[n] = '\0';
        ohlog("sysfs-probe readlink %s -> %s", links[i], tgt);
        /* and a stat THROUGH the link (what open depends on) */
        struct stat st;
        if (stat(links[i], &st) == 0)
            ohlog("sysfs-probe stat %s: mode %o size %lld", links[i],
                  st.st_mode, (long long)st.st_size);
        else
            ohlog("sysfs-probe stat %s: %s", links[i], strerror(errno));
    }
    static const char *files[] = {"/sys/block/vdb/dev", "/sys/block/vdb/uevent",
                                  "/sys/class/block/vdb/dev", "/proc/partitions"};
    for (size_t i = 0; i < sizeof(files) / sizeof(files[0]); i++) {
        int fd = open(files[i], O_RDONLY);
        if (fd < 0) {
            ohlog("sysfs-probe %s: %s", files[i], strerror(errno));
            continue;
        }
        char b[512];
        ssize_t n = read(fd, b, sizeof(b) - 1);
        close(fd);
        if (n > 0) {
            b[n] = '\0';
            while (n > 0 && (b[n - 1] == '\n'))
                b[--n] = '\0';
            ohlog("sysfs-probe %s: [%s]", files[i], b);
        } else {
            ohlog("sysfs-probe %s: empty", files[i]);
        }
    }
}

/* ------------------------------------------------------- step 1: mounts */

static int try_mount(const char *src, const char *dst, const char *fstype,
                     unsigned long flags, const char *opts, int required)
{
    if (mount(src, dst, fstype, flags, opts ? opts : "") == 0)
        return 0;
    if (required) {
        pfatal(dst);
        return -1;
    }
    ohlog("mount %s->%s (%s) failed: %s (tolerated)", src, dst,
          fstype ? fstype : "?", strerror(errno));
    return -1;
}

/* --------------------------------------------- step 3: ramfs rd/exec stress */

#define STRESS_ROUNDS 16

static long long stress_read(const char *path)
{
    /* full-file read + simple checksum; any short/failed read is the m3
     * frontier symptom and must be loud */
    int fd = open(path, O_RDONLY);
    if (fd < 0)
        return -1;
    long long sum = 0, total = 0;
    static char buf[65536];
    for (;;) {
        ssize_t n = read(fd, buf, sizeof(buf));
        if (n < 0) {
            close(fd);
            return -(total + 1);
        }
        if (n == 0)
            break;
        total += n;
        for (ssize_t i = 0; i < n; i += 997)
            sum += (unsigned char)buf[i];
    }
    close(fd);
    return sum * 1000003 + total;
}

/* ---------------------------------------------------------------- main */

int main(int argc, char **argv)
{
    ohlog("init_early running (pid %d, argc %d, argv0 %s)", getpid(), argc,
          argv[0]);

    /* ---- 1. mounts (OH init_early order) ---- */
    mkdir("/dev", 0755);
    mkdir("/proc", 0755);
    mkdir("/sys", 0755);
    mkdir("/usr", 0755);
    mkdir("/vendor", 0755);
    mkdir("/mnt", 0755);

    if (try_mount("proc", "/proc", "proc", MS_NOSUID | MS_NODEV, "hidepid=2", 1))
        return 1;
    ohlog("M1 proc mounted");
    if (try_mount("sysfs", "/sys", "sysfs", MS_NOSUID | MS_NODEV, NULL, 1))
        return 1;
    ohlog("M1 sysfs mounted");
    if (try_mount("tmpfs", "/dev", "tmpfs", MS_NOSUID, "mode=0755", 1))
        return 1;
    ohlog("M1 tmpfs on /dev");
    mkdir("/dev/pts", 0755);
    try_mount("devpts", "/dev/pts", "devpts", MS_NOSUID, "gid=5,mode=620", 0);
    ohlog("M1 devpts on /dev/pts");
    mkdir("/sys/fs", 0755);
    mkdir("/sys/fs/selinux", 0755);
    try_mount("selinuxfs", "/sys/fs/selinux", "selinuxfs", 0, NULL, 0);
    ohlog("M1 selinuxfs attempted (tolerated)");

    /* ---- 2. device nodes OH's init_early mknods first ---- */
    mkdir("/dev/block", 0755);
    mknod("/dev/null", S_IFCHR | 0666, makedev(1, 3));
    mknod("/dev/random", S_IFCHR | 0666, makedev(1, 8));
    mknod("/dev/urandom", S_IFCHR | 0666, makedev(1, 9));
    mknod("/dev/kmsg", S_IFCHR | 0600, makedev(1, 11));
    mknod("/dev/console", S_IFCHR | 0600, makedev(5, 1));
    mknod("/dev/ttyS0", S_IFCHR | 0620, makedev(4, 64));
    kmsg_fd = open("/dev/kmsg", O_WRONLY);
    ohlog("M1 dev nodes ok (kmsg fd %d)", kmsg_fd);

    /* dump /proc/cmdline — the kernel must relay ohos.* tokens verbatim */
    {
        char cmdbuf[2048];
        int fd = open("/proc/cmdline", O_RDONLY);
        if (fd >= 0) {
            ssize_t n = read(fd, cmdbuf, sizeof(cmdbuf) - 1);
            close(fd);
            if (n > 0) {
                cmdbuf[n] = '\0';
                ohlog("cmdline: %s", cmdbuf);
            }
        } else {
            ohlog("/proc/cmdline unreadable: %s", strerror(errno));
        }
    }

    /* ---- 3. ramfs read/exec stress (the m3 frontier) ---- */
    {
        long long want = stress_read("/bin/bash");
        if (want < 0) {
            pfatal("stress_read first /bin/bash");
            return 1;
        }
        for (int i = 1; i < STRESS_ROUNDS; i++) {
            long long got = stress_read("/bin/bash");
            if (got != want) {
                ohlog("RAMFS-READ-DETECT round %d: got %lld want %lld (errno %d)",
                      i, got, want, errno);
            }
        }
        ohlog("M3.5 ramfs read stress x%d ok (bash sum %lld bytes %lld)",
              STRESS_ROUNDS, want, (want - 1000003 * (want / 1000003)));
        /* exec it: dynamic ELF + ld.so + libtinfo all from the ramfs */
        pid_t p = fork();
        if (p == 0) {
            execl("/bin/bash", "/bin/bash", "-c",
                  "echo OHLITE: M3.5 bash exec from ramfs ok", (char *)NULL);
            _exit(127);
        }
        int st = 0;
        waitpid(p, &st, 0);
        if (!WIFEXITED(st) || WEXITSTATUS(st) != 0)
            ohlog("bash exec FAILED status %d errno %d", st, errno);
        else
            ohlog("M3.5 bash exec status 0");
    }

    /* ---- 4. ueventd: netlink listener + coldplug of block disks ---- */
    int nl = socket(AF_NETLINK, SOCK_DGRAM | SOCK_CLOEXEC, NETLINK_KOBJECT_UEVENT);
    if (nl < 0) {
        pfatal("netlink socket");
        return 1;
    }
    struct sockaddr_nl sa = {0};
    sa.nl_family = AF_NETLINK;
    sa.nl_groups = 0xffffffff; /* ueventd_socket.c */
    int one = 1;
    setsockopt(nl, SOL_SOCKET, SO_PASSCRED, &one, sizeof(one));
    if (bind(nl, (struct sockaddr *)&sa, sizeof(sa)) < 0) {
        pfatal("netlink bind");
        return 1;
    }
    ohlog("M3.7 netlink uevent socket bound");

    /* parse required mounts from /proc/cmdline like OH init does */
    char sys_dev[64] = "vdb", vend_dev[64] = "vdc";
    char sys_mp[64] = "/usr", vend_mp[64] = "/vendor";
    {
        char cmdbuf[4096];
        int fd = open("/proc/cmdline", O_RDONLY);
        if (fd >= 0) {
            ssize_t n = read(fd, cmdbuf, sizeof(cmdbuf) - 1);
            close(fd);
            if (n > 0) {
                cmdbuf[n] = '\0';
                char *tok = strtok(cmdbuf, " \n");
                while (tok) {
                    if (!strncmp(tok, "ohos.boot.required_mount.system=", 33) ||
                        !strncmp(tok, "ohos.required_mount.system=", 27)) {
                        char *v = strchr(tok, '=') + 1;
                        /* /dev/block/vdb@/usr@ext4@ro,barrier=1 */
                        char *a = strchr(v, '@');
                        if (a) {
                            *a = '\0';
                            const char *base = strrchr(v, '/');
                            snprintf(sys_dev, sizeof(sys_dev), "%s",
                                     base ? base + 1 : v);
                            char *b = strchr(a + 1, '@');
                            if (b) {
                                *b = '\0';
                                snprintf(sys_mp, sizeof(sys_mp), "%s", a + 1);
                            }
                        }
                    } else if (!strncmp(tok, "ohos.boot.required_mount.vendor=", 32) ||
                               !strncmp(tok, "ohos.required_mount.vendor=", 26)) {
                        char *v = strchr(tok, '=') + 1;
                        char *a = strchr(v, '@');
                        if (a) {
                            *a = '\0';
                            const char *base = strrchr(v, '/');
                            snprintf(vend_dev, sizeof(vend_dev), "%s",
                                     base ? base + 1 : v);
                            char *b = strchr(a + 1, '@');
                            if (b) {
                                *b = '\0';
                                snprintf(vend_mp, sizeof(vend_mp), "%s", a + 1);
                            }
                        }
                    }
                    tok = strtok(NULL, " \n");
                }
            }
        }
    }
    ohlog("required mounts parsed: %s->%s %s->%s", sys_dev, sys_mp, vend_dev,
          vend_mp);

    probe_sysfs();

    /* coldplug trigger: re-emit "add" for each block disk, then listen */
    const char *disks[2] = {sys_dev, vend_dev};
    int have_node[2] = {0, 0};
    for (int i = 0; i < 2; i++) {
        char p[128];
        snprintf(p, sizeof(p), "/sys/block/%s/uevent", disks[i]);
        int fd = open(p, O_WRONLY);
        if (fd >= 0) {
            if (write(fd, "add", 3) < 0)
                ohlog("uevent trigger %s write failed: %s", disks[i],
                      strerror(errno));
            else
                ohlog("uevent trigger %s ok", disks[i]);
            close(fd);
        } else {
            ohlog("uevent trigger %s open failed: %s (will try sysfs dev)",
                  disks[i], strerror(errno));
        }
    }

    /* listen with a monotonic 8s deadline; fall back to /sys/block/<d>/dev */
    struct timeval tv = {.tv_sec = 2, .tv_usec = 0};
    setsockopt(nl, SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof(tv));
    long long deadline = mono_ms() + 8000;
    int rounds = 0;
    while ((!have_node[0] || !have_node[1]) && mono_ms() < deadline) {
        rounds++;
        char buf[4096];
        struct iovec iov = {.iov_base = buf, .iov_len = sizeof(buf)};
        char cbuf[CMSG_SPACE(sizeof(struct ucred))];
        struct msghdr msg = {0};
        msg.msg_iov = &iov;
        msg.msg_iovlen = 1;
        msg.msg_control = cbuf;
        msg.msg_controllen = sizeof(cbuf);
        ssize_t n = recvmsg(nl, &msg, 0);
        if (n <= 0) {
            if (errno == EAGAIN || errno == EINTR || errno == EWOULDBLOCK) {
                ohlog("uevent recvmsg round %d: EAGAIN (waiting...)", rounds);
                continue;
            }
            ohlog("recvmsg failed: %s", strerror(errno));
            break;
        }
        /* SO_PASSCRED cmsg must be present or real ueventd drops it */
        int has_cred = 0;
        for (struct cmsghdr *c = CMSG_FIRSTHDR(&msg); c; c = CMSG_NXTHDR(&msg, c)) {
            if (c->cmsg_level == SOL_SOCKET && c->cmsg_type == SCM_CREDENTIALS)
                has_cred = 1;
        }
        buf[n < (ssize_t)sizeof(buf) ? n : (ssize_t)sizeof(buf) - 1] = '\0';
        /* parse ACTION/DEVNAME/MAJOR/MINOR out of the null-separated env */
        char action[16] = "", devname[64] = "";
        int major = -1, minor = -1;
        char *p = buf, *end = buf + n;
        while (p < end) {
            char *e = strchr(p, '=');
            char *term = p + strlen(p);
            if (!e) { p = term + 1; continue; }
            *e = '\0';
            if (!strcmp(p, "ACTION")) snprintf(action, sizeof(action), "%s", e + 1);
            else if (!strcmp(p, "DEVNAME")) snprintf(devname, sizeof(devname), "%s", e + 1);
            else if (!strcmp(p, "MAJOR")) major = atoi(e + 1);
            else if (!strcmp(p, "MINOR")) minor = atoi(e + 1);
            *e = '=';
            p = term + 1;
        }
        if (!has_cred)
            ohlog("uevent for %s MISSING SCM_CREDENTIALS (ueventd would drop)",
                  devname[0] ? devname : "?");
        if (devname[0] && !strcmp(action, "add") && major >= 0 && minor >= 0) {
            for (int i = 0; i < 2; i++) {
                if (!strcmp(devname, disks[i])) {
                    char path[128];
                    snprintf(path, sizeof(path), "/dev/block/%s", devname);
                    unlink(path);
                    if (mknod(path, S_IFBLK | 0660, makedev(major, minor)) == 0) {
                        have_node[i] = 1;
                        ohlog("M4 uevent add %s %d:%d -> %s (cred %d)", devname,
                              major, minor, path, has_cred);
                    } else {
                        pfatal(path);
                    }
                }
            }
        }
    }

    /* fallback: cold-read /sys/block/<d>/dev */
    for (int i = 0; i < 2; i++) {
        if (have_node[i])
            continue;
        char p[128], mm[32];
        snprintf(p, sizeof(p), "/sys/block/%s/dev", disks[i]);
        int fd = open(p, O_RDONLY);
        if (fd < 0)
            continue;
        ssize_t n = read(fd, mm, sizeof(mm) - 1);
        close(fd);
        if (n <= 0)
            continue;
        mm[n] = '\0';
        int major, minor;
        if (sscanf(mm, "%d:%d", &major, &minor) == 2) {
            char path[128];
            snprintf(path, sizeof(path), "/dev/block/%s", disks[i]);
            unlink(path);
            if (mknod(path, S_IFBLK | 0660, makedev(major, minor)) == 0) {
                have_node[i] = 1;
                ohlog("M4 sysfs coldplug %s %d:%d -> %s", disks[i], major,
                      minor, path);
            }
        }
    }

    /* ---- 5. mount system+vendor (the required_mount tokens) ---- */
    char syspath[128], vendpath[128];
    snprintf(syspath, sizeof(syspath), "/dev/block/%s", sys_dev);
    snprintf(vendpath, sizeof(vendpath), "/dev/block/%s", vend_dev);
    int sys_ok = 0, vend_ok = 0;
    for (int attempt = 0; attempt < 20 && !(sys_ok && vend_ok); attempt++) {
        if (!sys_ok && have_node[0] &&
            mount(syspath, sys_mp, "ext4", MS_RDONLY | MS_NOSUID, "") == 0)
            sys_ok = 1;
        if (!vend_ok && have_node[1] &&
            mount(vendpath, vend_mp, "ext4", MS_RDONLY | MS_NOSUID, "") == 0)
            vend_ok = 1;
        if (sys_ok && vend_ok)
            break;
        usleep(500000); /* devices may still be probing */
    }
    if (!sys_ok) {
        pfatal("mount system image");
        ohlog("BOOT STOPPED: no /usr — cannot switch_root");
        /* park so the serial log keeps the evidence */
        for (;;) { sleep(60); ohlog("parked (no system mount)"); }
    }
    ohlog("M4 system %s mounted on %s", sys_dev, sys_mp);
    ohlog("M4 vendor %s mounted on %s (%s)", vend_dev, vend_mp,
          vend_ok ? "ok" : "ABSENT — tolerated");

    /* ---- 6. move mounts under the new root, switch_root, exec 2nd stage */
    char mp_proc[96], mp_sys[96], mp_dev[96], mp_vend[96];
    snprintf(mp_proc, sizeof(mp_proc), "%s/proc", sys_mp);
    snprintf(mp_sys, sizeof(mp_sys), "%s/sys", sys_mp);
    snprintf(mp_dev, sizeof(mp_dev), "%s/dev", sys_mp);
    snprintf(mp_vend, sizeof(mp_vend), "%s/vendor", sys_mp);
    if (mount("/proc", mp_proc, NULL, MS_MOVE, NULL) == 0)
        ohlog("M4.5 moved /proc under new root");
    else
        ohlog("move /proc failed: %s (tolerated)", strerror(errno));
    if (mount("/sys", mp_sys, NULL, MS_MOVE, NULL) == 0)
        ohlog("M4.5 moved /sys under new root");
    else
        ohlog("move /sys failed: %s (tolerated)", strerror(errno));
    if (mount("/dev", mp_dev, NULL, MS_MOVE, NULL) == 0)
        ohlog("M4.5 moved /dev under new root");
    else
        ohlog("move /dev failed: %s (tolerated)", strerror(errno));
    if (vend_ok && mount(vend_mp, mp_vend, NULL, MS_MOVE, NULL) == 0)
        ohlog("M4.5 moved /vendor under new root");
    else if (vend_ok)
        ohlog("move /vendor failed: %s", strerror(errno));

    if (chdir(sys_mp) != 0) {
        pfatal("chdir newroot");
        return 1;
    }
    if (mount(".", "/", NULL, MS_MOVE, NULL) != 0) {
        pfatal("MS_MOVE newroot to /");
        return 1;
    }
    ohlog("M4.7 root moved — switch_root chroot");
    if (chroot(".") != 0) {
        pfatal("chroot");
        return 1;
    }
    chdir("/");
    ohlog("M5 switch_root complete — exec /bin/init --second-stage");
    close(nl);

    char *nargv[] = {"/bin/init", "--second-stage", NULL};
    char *nenv[] = {"PATH=/bin:/usr/bin:/sbin:/usr/sbin", "HOME=/root",
                    "TERM=linux", NULL};
    execve("/bin/init", nargv, nenv);
    pfatal("execve /bin/init second-stage");
    return 1;
}
