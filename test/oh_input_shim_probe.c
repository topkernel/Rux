/*
 * oh_input_shim_probe — replicates OpenHarmony multimodalinput's device
 * discovery algorithm against this kernel, step by step, so every syscall
 * the real stack issues can be verified without booting the OH image:
 *
 *   1. HotplugDetector: opendir("/dev/input") + readdir
 *   2. udev shim: stat node -> readlink /sys/dev/char/<maj:min>
 *      -> require /sys/devices/ prefix -> stat + read <syspath>/uevent
 *      -> walk one parent dir, stat + read its uevent
 *   3. tag computation from the parent uevent's EV=/KEY=/ABS=/REL=/PROP=
 *      lines (ID_INPUT_KEYBOARD / ID_INPUT_MOUSE / ID_INPUT_TOUCHSCREEN)
 *   4. libinput path mode: open(O_RDWR|O_NONBLOCK|O_CLOEXEC),
 *      EVIOCGVERSION/EVIOCGID/EVIOCGBIT/EVIOCGABS, blocking read of one
 *      input_event (proves events flow to userspace)
 *
 * Exit code: number of failed checks.
 */
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <stdarg.h>
#include <string.h>
#include <unistd.h>
#include <fcntl.h>
#include <dirent.h>
#include <errno.h>
#include <poll.h>
#include <sys/stat.h>
#include <sys/sysmacros.h>
#include <sys/ioctl.h>
#include <sys/wait.h>
#include <sys/inotify.h>
#include <linux/input.h>

#ifndef IN_CLOEXEC
#define IN_CLOEXEC O_CLOEXEC
#endif

static int failures;

static void check(int cond, const char *fmt, ...)
{
    va_list ap;
    va_start(ap, fmt);
    if (cond) {
        printf("PASS: ");
    } else {
        failures++;
        printf("FAIL: ");
    }
    vprintf(fmt, ap);
    printf("\n");
    va_end(ap);
}

/* Read a whole sysfs attribute into a buffer. */
static int read_file(const char *path, char *buf, size_t bufsz)
{
    int fd = open(path, O_RDONLY);
    if (fd < 0)
        return -1;
    ssize_t n = read(fd, buf, bufsz - 1);
    close(fd);
    if (n < 0)
        return -1;
    buf[n] = 0;
    /* trim trailing newline */
    while (n > 0 && (buf[n - 1] == '\n' || buf[n - 1] == '\r'))
        buf[--n] = 0;
    return 0;
}

/* OH shim ResolveSymLink: resolve "../"-prefixed relative targets. */
static int resolve_symlink(const char *syspath, char *out, size_t outsz)
{
    char target[1024];
    ssize_t len = readlink(syspath, target, sizeof(target));
    if (len <= 0 || (size_t)len >= sizeof(target))
        return -1;
    target[len] = 0;

    int back = 0;
    while (strncmp(target + back * 3, "../", 3) == 0)
        back++;
    const char *tail = target + back * 3;

    /* chop back+1 components off syspath */
    char base[1024];
    snprintf(base, sizeof(base), "%s", syspath);
    for (int i = 0; i <= back; i++) {
        char *slash = strrchr(base, '/');
        if (!slash)
            return -1;
        *slash = 0;
    }
    snprintf(out, outsz, "%s/%s", base, tail);
    return 0;
}

/* Get one KEY= style value from a uevent env block. */
static int env_get(const char *env, const char *key, char *out, size_t outsz)
{
    size_t klen = strlen(key);
    const char *p = env;
    while (*p) {
        const char *eol = strchr(p, '\n');
        size_t linelen = eol ? (size_t)(eol - p) : strlen(p);
        if (linelen > klen && strncmp(p, key, klen) == 0 && p[klen] == '=') {
            size_t vlen = linelen - klen - 1;
            if (vlen >= outsz)
                vlen = outsz - 1;
            memcpy(out, p + klen + 1, vlen);
            out[vlen] = 0;
            return 0;
        }
        if (!eol)
            break;
        p = eol + 1;
    }
    return -1;
}

/* OH shim BitVector: hex u64 words, high word first. */
#define MAX_WORDS 16
static unsigned long long bv[MAX_WORDS];
static int bv_words;

static void bv_parse(const char *s)
{
    bv_words = 0;
    const char *p = s;
    unsigned long long tmp[MAX_WORDS];
    int n = 0;
    while (*p && n < MAX_WORDS) {
        char *end;
        tmp[n++] = strtoull(p, &end, 16);
        if (end == p)
            break;
        p = end;
        while (*p == ' ')
            p++;
    }
    /* reverse: index 0 = low word */
    for (int i = 0; i < n; i++)
        bv[i] = tmp[n - 1 - i];
    bv_words = n;
}

static int bv_bit(int idx)
{
    int w = idx / 64;
    if (w >= bv_words || w >= MAX_WORDS)
        return 0;
    return (bv[w] >> (idx % 64)) & 1ULL;
}

int main(void)
{
    /* ---- 1. HotplugDetector::Scan ---- */
    DIR *d = opendir("/dev/input");
    check(d != NULL, "opendir(/dev/input)");
    if (!d)
        return failures;

    char nodes[8][64];
    int nnodes = 0;
    struct dirent *de;
    while ((de = readdir(d)) != NULL && nnodes < 8) {
        if (de->d_name[0] == '.')
            continue;
        snprintf(nodes[nnodes], sizeof(nodes[nnodes]), "/dev/input/%s", de->d_name);
        printf("INFO: found node %s\n", nodes[nnodes]);
        nnodes++;
    }
    closedir(d);
    check(nnodes >= 1, "found %d node(s) under /dev/input", nnodes);
    if (nnodes == 0)
        return failures;

    /* ---- inotify watch (HotplugDetector::Init requires this to work) */
    int ifd = inotify_init1(IN_CLOEXEC);
    check(ifd >= 0, "inotify_init1");
    int wd = inotify_add_watch(ifd, "/dev/input", 0x30 /*IN_DELETE|IN_CREATE*/);
    check(wd >= 0, "inotify_add_watch(/dev/input)");

    for (int i = 0; i < nnodes; i++) {
        const char *node = nodes[i];
        printf("\n=== %s ===\n", node);

        /* ---- 2. udev shim walk ---- */
        struct stat st;
        check(stat(node, &st) == 0, "stat(%s)", node);
        char linkpath[256], resolved[1024];
        snprintf(linkpath, sizeof(linkpath), "/sys/dev/char/%d:%d",
                 major(st.st_rdev), minor(st.st_rdev));
        check(resolve_symlink(linkpath, resolved, sizeof(resolved)) == 0,
              "readlink+resolve %s", linkpath);
        printf("INFO: resolved syspath: %s\n", resolved);
        check(strncmp(resolved, "/sys/devices/", 13) == 0,
              "resolved path is under /sys/devices (shim requirement)");

        char evfile[1100], env[4096], envparent[4096];
        snprintf(evfile, sizeof(evfile), "%s/uevent", resolved);
        struct stat ust;
        check(stat(evfile, &ust) == 0, "stat(%s/uevent)", resolved);
        check(read_file(evfile, env, sizeof(env)) == 0, "read event uevent");

        char devname[256] = "";
        int has_dn = env_get(env, "DEVNAME", devname, sizeof(devname)) == 0;
        check(has_dn && devname[0], "DEVNAME present (%s)", devname);

        /* parent inputN kobject */
        char parent[1024];
        snprintf(parent, sizeof(parent), "%s", resolved);
        char *slash = strrchr(parent, '/');
        check(slash != NULL, "parent split");
        if (slash)
            *slash = 0;
        printf("INFO: parent syspath: %s\n", parent);
        char pfile[1100];
        snprintf(pfile, sizeof(pfile), "%s/uevent", parent);
        struct stat pst;
        int parent_ok = stat(pfile, &pst) == 0;
        check(parent_ok, "stat(parent uevent) — libinput walks 1 parent");
        int penv_ok = parent_ok ? read_file(pfile, envparent, sizeof(envparent)) == 0 : 0;
        check(penv_ok, "read parent uevent");
        printf("INFO: parent uevent:\n%s\n", envparent);

        /* subsystem link (GetSubsystem) */
        char subfile[1200], subtarget[1024];
        snprintf(subfile, sizeof(subfile), "%s/subsystem", resolved);
        ssize_t sl = readlink(subfile, subtarget, sizeof(subtarget) - 1);
        if (sl > 0) {
            subtarget[sl] = 0;
            char *base = strrchr(subtarget, '/');
            check(base != NULL && strcmp(base + 1, "input") == 0,
                  "subsystem link basename 'input' (%s)", subtarget);
        } else {
            check(0, "readlink(%s/subsystem)", resolved);
        }

        /* ---- 3. tag computation (CheckKeys / CheckPointers) ---- */
        char key[2048] = "", ev[256] = "", abs[256] = "", rel[256] = "", prop[64] = "0";
        env_get(envparent, "KEY", key, sizeof(key));
        env_get(envparent, "EV", ev, sizeof(ev));
        env_get(envparent, "ABS", abs, sizeof(abs));
        env_get(envparent, "REL", rel, sizeof(rel));
        env_get(envparent, "PROP", prop, sizeof(prop));

        bv_parse(ev);
        int has_key = bv_bit(EV_KEY);
        bv_parse(key);
        int is_keyboard = has_key;
        for (int k = KEY_ESC; k < KEY_D && is_keyboard; k++)
            is_keyboard = bv_bit(k);
        int has_mouse_btn = 0;
        for (int b = BTN_MOUSE; b < BTN_JOYSTICK; b++)
            if (bv_bit(b))
                has_mouse_btn = 1;
        int has_touch = bv_bit(BTN_TOUCH);
        bv_parse(abs);
        int has_abs_xy = bv_bit(ABS_X) && bv_bit(ABS_Y);
        int has_mt = bv_bit(ABS_MT_POSITION_X) && bv_bit(ABS_MT_POSITION_Y);
        bv_parse(prop);
        int is_direct = bv_bit(INPUT_PROP_DIRECT);

        printf("INFO: tags -> ID_INPUT=1%s%s%s%s\n",
               is_keyboard ? " ID_INPUT_KEYBOARD" : "",
               (has_mouse_btn && has_abs_xy) ? " ID_INPUT_MOUSE(abs)" : "",
               (has_touch && is_direct) ? " ID_INPUT_TOUCHSCREEN" : "",
               (has_mt && has_touch && is_direct) ? " (+MT)" : "");
        check(is_keyboard || (has_mouse_btn && has_abs_xy) || (has_touch && is_direct),
              "device receives at least one ID_INPUT tag");

        /* ---- 4. libinput path-mode device open + ioctls + read ---- */
        int fd = open(node, O_RDWR | O_NONBLOCK | O_CLOEXEC);
        check(fd >= 0, "open(%s, O_RDWR|O_NONBLOCK)", node);
        if (fd < 0)
            continue;

        int version = 0;
        check(ioctl(fd, EVIOCGVERSION, &version) == 0 && version == 0x010001,
              "EVIOCGVERSION = %#x", version);
        struct input_id id;
        check(ioctl(fd, EVIOCGID, &id) == 0,
              "EVIOCGID bustype=%#x vendor=%#x", id.bustype, id.vendor);
        char name[128] = "";
        ioctl(fd, EVIOCGNAME(sizeof(name) - 1), name);
        printf("INFO: EVIOCGNAME: %s\n", name);

        unsigned char bits[96];
        if (ioctl(fd, EVIOCGBIT(0, sizeof(bits)), bits) >= 0) {
            int evsyn = bits[0] & 1, evkey = (bits[0] >> 1) & 1,
                evrel = (bits[0] >> 2) & 1, evabs = (bits[0] >> 3) & 1;
            printf("INFO: EVIOCGBIT(0): syn=%d key=%d rel=%d abs=%d\n",
                   evsyn, evkey, evrel, evabs);
            /* cross-check sysfs EV= word against the ioctl bitmap */
            unsigned long long ioctl_word = 0;
            for (int b = 0; b < 64; b++)
                if (bits[b / 8] & (1 << (b % 8)))
                    ioctl_word |= 1ULL << b;
            bv_parse(ev);
            check(bv_words >= 1 && bv[0] == ioctl_word,
                  "sysfs EV= matches EVIOCGBIT(0) (sysfs=%#llx ioctl=%#llx)",
                  bv_words ? bv[0] : 0ULL, ioctl_word);
        } else {
            check(0, "EVIOCGBIT(0)");
        }

        struct input_absinfo ai;
        if (has_abs_xy && ioctl(fd, EVIOCGABS(ABS_X), &ai) == 0)
            printf("INFO: EVIOCGABS(ABS_X): min=%d max=%d\n", ai.minimum, ai.maximum);

        /* nonblocking read must give EAGAIN now, and the fd must poll */
        struct input_event iev;
        ssize_t rn = read(fd, &iev, sizeof(iev));
        check(rn == -1 && errno == EAGAIN, "empty nonblocking read -> EAGAIN");

        if (fork() == 0) {
            /* child: blocking read with 15s timeout via poll */
            struct pollfd pfd = { .fd = fd, .events = POLLIN };
            int pr = poll(&pfd, 1, 15000);
            if (pr <= 0) {
                printf("CHILD-READ: TIMEOUT/ERR (%d)\n", pr);
                _exit(2);
            }
            rn = read(fd, &iev, sizeof(iev));
            if (rn == (ssize_t)sizeof(iev)) {
                printf("CHILD-READ: OK type=%u code=%u value=%d\n",
                       iev.type, iev.code, iev.value);
                _exit(0);
            }
            printf("CHILD-READ: BAD rn=%zd errno=%d\n", rn, errno);
            _exit(3);
        }
        printf("WAIT-FOR-EVENT\n");
        fflush(stdout);
        int status = 0;
        waitpid(-1, &status, 0);
        check(WIFEXITED(status) && WEXITSTATUS(status) == 0,
              "event delivered to a blocking reader");
        close(fd);
    }

    printf("\nSUMMARY: %d failures\n", failures);
    return failures;
}
