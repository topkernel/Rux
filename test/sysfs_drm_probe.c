/*
 * sysfs_drm_probe.c — validate the /sys DRM tree libdrm + Mesa walk.
 *
 * libdrm's drmGetDevice(fd) (string table of the image's
 * /system/lib64/chipset-sdk/libdrm.so) does exactly this walk:
 *
 *   readlink("/sys/dev/char/226:<minor>")          -> ../../devices/pci.../drm/<node>
 *   strip trailing /drm/<node>                     -> PCI function dir
 *   read <fn>/vendor <fn>/device <fn>/subsystem_vendor <fn>/subsystem_device
 *   readlink <fn>/subsystem                        -> .../bus/pci
 *   opendir <fn>/drm                               -> node names
 *
 * gbm_create_device() then maps the DRM_IOCTL_VERSION driver name onto
 * <name>_dri.so. On the reference Linux 6.6 virtio-gpu the name is
 * "virtio_gpu"; the image ships virtio_gpu_dri.so + kms_swrast_dri.so.
 *
 * Every step prints ok/FAIL; verdict line:
 *     SYSFS_DRM_PROBE RESULT: PASS <n>/<n>
 *
 * Build: same as drm_probe.c (static musl, toolchain/x86_64-rux-linux-musl).
 */
#include <fcntl.h>
#include <stdint.h>
#include <stddef.h>
#include <string.h>
#include <errno.h>
#include <unistd.h>
#include <dirent.h>
#include <sys/ioctl.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <stdio.h>

static int puts_(const char *s) { return write(1, s, strlen(s)); }
static void putu_(unsigned long v) {
    char b[24]; int i = 24;
    if (!v) { puts_("0"); return; }
    while (v && i) { b[--i] = '0' + (v % 10); v /= 10; }
    write(1, b + i, 24 - i);
}
static int passed, total;
static void ok(const char *what) { puts_("ok   "); puts_(what); puts_("\n"); passed++; total++; }
static void fail(const char *what, long detail) {
    puts_("FAIL "); puts_(what); puts_(" errno="); putu_((unsigned long)(-detail)); puts_("\n"); total++;
}
static void failm(const char *what, const char *msg) {
    puts_("FAIL "); puts_(what); puts_(" — "); puts_(msg); puts_("\n"); total++;
}

#define DRM_IOCTL_VERSION 0xc0406400u /* _IOWR(d, 0, 64) */
struct drm_version_ {
    int32_t major, minor, patch;
    size_t name_len; char *name;
    size_t date_len; char *date;
    size_t desc_len; char *desc;
};

static int read_file(const char *path, char *buf, size_t n) {
    int fd = open(path, O_RDONLY);
    if (fd < 0) return -1;
    ssize_t r = read(fd, buf, n - 1);
    close(fd);
    if (r < 0) return -1;
    buf[r] = 0;
    return (int)r;
}

/* Strip trailing whitespace/newline in place. */
static void chomp(char *s) { size_t n = strlen(s); while (n && (s[n-1]=='\n'||s[n-1]==' '||s[n-1]=='\r')) s[--n] = 0; }

int main(void)
{
    char buf[256], link[256], fn[256];
    int n;

    /* 1. card0 opens */
    int fd = open("/dev/dri/card0", O_RDWR | O_CLOEXEC);
    if (fd >= 0) ok("open /dev/dri/card0"); else fail("open /dev/dri/card0", -errno);

    /* 2. version name == virtio_gpu */
    {
        char name[32] = {0};
        struct drm_version_ v;
        memset(&v, 0, sizeof v);
        v.name_len = sizeof name - 1; v.name = name;
        long r = ioctl(fd, DRM_IOCTL_VERSION, &v);
        if (r == 0) {
            if (!strcmp(name, "virtio_gpu")) ok("drmGetVersion name == virtio_gpu");
            else { failm("drmGetVersion name", name[0] ? name : "(empty)"); }
        } else fail("DRM_IOCTL_VERSION", r ? r : -errno);
    }

    /* 3. /sys/dev/char/226:0 readlink */
    n = (int)syscall(SYS_readlink, "/sys/dev/char/226:0", link, sizeof link - 1);
    if (n > 0) {
        link[n] = 0;
        ok("/sys/dev/char/226:0 readlink");
    } else { fail("readlink /sys/dev/char/226:0", -errno); link[0] = 0; }

    /* 4. the link target names the node and a PCI function */
    if (strstr(link, "/drm/card0") && !strncmp(link, "../../devices/pci0000:00/", 25)) {
        ok("link target shape ../../devices/pci0000:00/<slot>/drm/card0");
    } else failm("link target shape", link[0] ? link : "(empty)");

    /* 5. resolve to an absolute /sys path: the link target is relative
     * to the link's directory (/sys/dev/char/) — normalize lexically
     * exactly like realpath() (libdrm does the same). */
    {
        const char *base = "/sys/dev/char";
        size_t o = 0;
        for (size_t i = 0; base[i]; i++) fn[o++] = base[i];
        char *comp = link;
        while (comp) {
            char *next = strchr(comp, '/');
            if (next) *next = 0;
            if (!strcmp(comp, "..")) {
                while (o > 1 && fn[o-1] != '/') fn[--o] = 0;
                if (o > 1) fn[--o] = 0;
            } else if (strcmp(comp, ".") && comp[0]) {
                fn[o++] = '/';
                for (size_t i = 0; comp[i]; i++) fn[o++] = comp[i];
            }
            comp = next ? next + 1 : NULL;
        }
        /* drop trailing "/card0" and "/drm" -> PCI function dir */
        char *slash = strrchr(fn, '/');
        if (slash) *slash = 0;
        slash = strrchr(fn, '/');
        if (slash && !strcmp(slash, "/drm")) *slash = 0;
    }
    struct stat st;
    if (!stat(fn, &st) && S_ISDIR(st.st_mode)) ok("PCI function dir exists");
    else failm("PCI function dir", fn);

    /* 6. vendor/device ids */
    static const char *ids[4][2] = {
        {"/vendor", "0x1af4"}, {"/device", "0x1050"},
        {"/subsystem_vendor", "0x1af4"}, {"/subsystem_device", "0x1100"},
    };
    for (int i = 0; i < 4; i++) {
        char p[300]; snprintf(p, sizeof p, "%s%s", fn, ids[i][0]);
        if (read_file(p, buf, sizeof buf) > 0) {
            chomp(buf);
            if (!strcmp(buf, ids[i][1])) { ok(ids[i][0] + 1); }
            else { failm(ids[i][0] + 1, buf); }
        } else fail(ids[i][0] + 1, -errno);
    }

    /* 7. subsystem -> bus/pci */
    {
        char p[300]; snprintf(p, sizeof p, "%s/subsystem", fn);
        n = (int)syscall(SYS_readlink, p, buf, sizeof buf - 1);
        if (n > 0) {
            buf[n] = 0;
            if (strstr(buf, "bus/pci")) ok("subsystem -> bus/pci");
            else failm("subsystem -> bus/pci", buf);
        } else fail("readlink subsystem", -errno);
    }

    /* 8. <fn>/drm enumerates both nodes */
    {
        char p[300]; snprintf(p, sizeof p, "%s/drm", fn);
        DIR *d = opendir(p);
        int card = 0, render = 0;
        if (d) {
            struct dirent *e;
            while ((e = readdir(d))) {
                if (!strcmp(e->d_name, "card0")) card = 1;
                if (!strcmp(e->d_name, "renderD128")) render = 1;
            }
            closedir(d);
        }
        if (card && render) ok("<fn>/drm lists card0 + renderD128");
        else failm("<fn>/drm listing", d ? "missing node" : "opendir failed");
    }

    /* 9. node uevent carries MAJOR/MINOR/DEVNAME */
    {
        char p[300]; snprintf(p, sizeof p, "%s/drm/card0/uevent", fn);
        if (read_file(p, buf, sizeof buf) > 0) {
            if (strstr(buf, "MAJOR=226") && strstr(buf, "MINOR=0") && strstr(buf, "DEVNAME=dri/card0"))
                ok("card0 uevent MAJOR/MINOR/DEVNAME");
            else failm("card0 uevent", buf);
        } else fail("card0 uevent", -errno);
    }

    /* 10. renderD128 char link + uevent */
    {
        n = (int)syscall(SYS_readlink, "/sys/dev/char/226:128", link, sizeof link - 1);
        if (n > 0) {
            link[n] = 0;
            if (strstr(link, "renderD128")) ok("/sys/dev/char/226:128 -> renderD128");
            else failm("/sys/dev/char/226:128", link);
        } else fail("readlink 226:128", -errno);
    }

    /* 11. /sys/class/drm/card0 symlink resolves + version file */
    {
        n = (int)syscall(SYS_readlink, "/sys/class/drm/card0", link, sizeof link - 1);
        if (n > 0) { link[n] = 0; ok("/sys/class/drm/card0 link"); }
        else fail("readlink /sys/class/drm/card0", -errno);
        if (read_file("/sys/class/drm/version", buf, sizeof buf) > 0) ok("/sys/class/drm/version");
        else fail("/sys/class/drm/version", -errno);
    }

    /* 12. /sys/bus/pci/devices/<slot> backlink */
    {
        char slot[64];
        const char *b = strstr(fn, "pci0000:00/");
        if (b) {
            snprintf(slot, sizeof slot, "/sys/bus/pci/devices/%s", b + 11);
            n = (int)syscall(SYS_readlink, slot, buf, sizeof buf - 1);
            if (n > 0) { buf[n] = 0; ok("/sys/bus/pci/devices/<slot> link"); }
            else fail("readlink bus/pci/devices", -errno);
        } else failm("slot derivation", fn);
    }

    puts_("SYSFS_DRM_PROBE RESULT: ");
    if (passed == total) { puts_("PASS "); } else { puts_("FAIL "); }
    putu_((unsigned long)passed); puts_("/"); putu_((unsigned long)total); puts_("\n");
    _exit(passed == total ? 0 : 1);
}
