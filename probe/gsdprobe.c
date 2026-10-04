/*
 * gsdprobe.c — replicate gnome-session-binary's gsd component discovery
 * path with full syscall-level logging, to find why /etc/xdg/autostart
 * scanning yields zero apps on the Rux kernel.
 *
 * Build: riscv64-linux-gnu-gcc -static -O1 -o gsdprobe gsdprobe.c
 *
 * Everything the discovery path touches:
 *   1. getenv XDG_CONFIG_DIRS / XDG_CONFIG_HOME / HOME / XDG_DATA_DIRS
 *   2. opendir + readdir(getdents64) on the autostart dir  (g_dir_open)
 *   3. per .desktop: open + fstat + read loop + close       (g_file_get_contents)
 *   4. mini keyfile parse: [Desktop Entry] + Type/Exec keys (g_key_file)
 *   5. second readdir pass on the same DIR* (glib re-scan)
 *   6. raw getdents64 with a tiny buffer (batch-boundary test)
 *   7. stat + access on each file
 * Output: lines tagged PROBE: for serial-console grepping.
 */
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <errno.h>
#include <fcntl.h>
#include <unistd.h>
#include <dirent.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <sys/syscall.h>

#define MAXFILES 128
#define NAMELEN 256

static char names[MAXFILES][NAMELEN];
static int nfiles = 0;

static void perr(const char *msg)
{
    printf("PROBE: ERR %s: errno=%d (%s)\n", msg, errno, strerror(errno));
}

static int cmp_names(const void *a, const void *b)
{
    return strcmp((const char *)a, (const char *)b);
}

/* mini keyfile validation: mimic what g_key_file does to reject a file:
 * must contain a group header line and key=value lines; must be valid
 * UTF-8-ish (we check: no stray NUL bytes inside the read length). */
static int keyfile_check(const char *buf, ssize_t len, const char *name)
{
    int has_group = 0, has_type = 0, has_exec = 0, has_name = 0;
    int nul_inside = 0, lines = 0;
    ssize_t i;
    char *copy, *line, *save = NULL;

    for (i = 0; i < len; i++)
        if (buf[i] == '\0')
            nul_inside = 1;
    if (len > 0 && buf[len - 1] != '\n')
        printf("PROBE: WARN %s: last byte not \\n\n", name);

    copy = malloc(len + 1);
    if (!copy) return -1;
    memcpy(copy, buf, len);
    copy[len] = '\0';
    for (line = strtok_r(copy, "\n", &save); line;
         line = strtok_r(NULL, "\n", &save)) {
        lines++;
        if (line[0] == '[' && strchr(line, ']'))
            has_group++;
        if (!strncmp(line, "Type=", 5)) has_type = 1;
        if (!strncmp(line, "Exec=", 5)) has_exec = 1;
        if (!strncmp(line, "Name=", 5)) has_name = 1;
    }
    free(copy);
    printf("PROBE: KF %s len=%zd lines=%d group=%d type=%d exec=%d name=%d nul=%d\n",
           name, len, lines, has_group, has_type, has_exec, has_name, nul_inside);
    if (!has_group || !has_type) {
        printf("PROBE: KF-FAIL %s (group=%d type=%d)\n", name, has_group, has_type);
        return 0;
    }
    return 1;
}

static void scan_file(const char *dir, const char *name, int pass)
{
    char path[1024];
    char buf[16384];
    struct stat st;
    int fd, ok = 1;
    ssize_t n, total = 0;

    snprintf(path, sizeof(path), "%s/%s", dir, name);
    fd = open(path, O_RDONLY);
    if (fd < 0) {
        perr(path);
        printf("PROBE: OPEN-FAIL %s pass%d\n", name, pass);
        return;
    }
    if (fstat(fd, &st) < 0) {
        perr("fstat");
        printf("PROBE: FSTAT-FAIL %s pass%d\n", name, pass);
        close(fd);
        return;
    }
    if (!S_ISREG(st.st_mode)) {
        printf("PROBE: NOT-REG %s mode=%o\n", name, st.st_mode);
        close(fd);
        return;
    }
    while (total < (ssize_t)sizeof(buf)) {
        n = read(fd, buf + total, sizeof(buf) - total);
        if (n < 0) {
            perr("read");
            ok = 0;
            break;
        }
        if (n == 0) break;
        total += n;
    }
    close(fd);
    if (total != st.st_size)
        printf("PROBE: SIZE-MISMATCH %s st_size=%lld read=%zd\n",
               name, (long long)st.st_size, total);
    if (!ok) return;
    if (!keyfile_check(buf, total, name))
        return;
    /* stat + access like gsm's condition checks */
    if (stat(path, &st) < 0) perr("stat");
    if (access(path, R_OK) < 0) perr("access R_OK");
    printf("PROBE: OK %s pass%d size=%zd\n", name, pass, total);
}

static void scan_dir(const char *dir, int pass)
{
    DIR *d;
    struct dirent *de;
    int ndesktop = 0, nother = 0;
    int i;

    printf("PROBE: SCAN pass%d dir=%s\n", pass, dir);
    d = opendir(dir);
    if (!d) {
        perr(dir);
        printf("PROBE: OPENDIR-FAIL %s pass%d\n", dir, pass);
        return;
    }
    errno = 0;
    while ((de = readdir(d)) != NULL) {
        size_t l = strlen(de->d_name);
        if (de->d_type == DT_DIR) continue;
        if (l > 8 && !strcmp(de->d_name + l - 8, ".desktop")) {
            if (pass == 1 && nfiles < MAXFILES) {
                strncpy(names[nfiles], de->d_name, NAMELEN - 1);
                names[nfiles][NAMELEN - 1] = '\0';
                nfiles++;
            }
            ndesktop++;
        } else {
            nother++;
        }
    }
    if (errno) perr("readdir");
    closedir(d);
    printf("PROBE: DIRENTS %s pass%d desktop=%d other=%d\n",
           dir, pass, ndesktop, nother);

    if (pass == 1) {
        qsort(names, nfiles, NAMELEN, cmp_names);
        for (i = 0; i < nfiles; i++)
            scan_file(dir, names[i], pass);
    }
}

/* raw getdents64 with a tiny buffer to stress batch boundaries */
static void raw_getdents(const char *dir)
{
    int fd;
    char buf[64];  /* deliberately small: partial entries + continuation */
    long total = 0, calls = 0;
    printf("PROBE: RAW open %s ...\n", dir);
    fflush(stdout);
    fd = open(dir, O_RDONLY | O_DIRECTORY);
    if (fd < 0) {
        perr("open dir raw");
        return;
    }
    printf("PROBE: RAW open fd=%d\n", fd);
    fflush(stdout);
    for (;;) {
        int n = syscall(SYS_getdents64, fd, buf, sizeof(buf));
        if (n < 0) {
            perr("getdents64");
            break;
        }
        calls++;
        if (n == 0) break;
        total += n;
        if (calls <= 3 || (calls % 50) == 0) {
            /* parse entries: ino(8) off(8) reclen(2) type(1) name... */
            int off = 0, k = 0;
            printf("PROBE: RAW call#%ld n=%d pos=%ld\n",
                   calls, n, (long)lseek(fd, 0, SEEK_CUR));
            for (k = 0; k < n && k < 24; k++)
                printf("PROBE: RAW   byte[%02d]=%02x %c\n", k,
                       (unsigned char)buf[k],
                       (buf[k] >= 32 && buf[k] < 127) ? buf[k] : '.');
            k = 0;
            while (off + 19 <= n && k < 6) {
                unsigned short reclen = *(unsigned short *)(buf + off + 16);
                printf("PROBE: RAW   ent%d reclen=%u type=%u name=%s\n",
                       k, reclen, (unsigned char)buf[off + 18], buf + off + 19);
                if (reclen < 20 || off + reclen > n) break;
                off += reclen;
                k++;
            }
            fflush(stdout);
        }
        if (calls > 500) {
            printf("PROBE: RAW LOOP-GUARD HIT (no EOF after 500 calls)\n");
            fflush(stdout);
            break;
        }
    }
    close(fd);
    printf("PROBE: RAW-GETDENTS %s bytes=%ld calls=%ld\n", dir, total, calls);
    fflush(stdout);
}

/* the 16 required components from gnome.session (per the failing log) */
static const char *required[] = {
    "org.gnome.SettingsDaemon.A11ySettings",
    "org.gnome.SettingsDaemon.Color",
    "org.gnome.SettingsDaemon.Datetime",
    "org.gnome.SettingsDaemon.Housekeeping",
    "org.gnome.SettingsDaemon.Keyboard",
    "org.gnome.SettingsDaemon.MediaKeys",
    "org.gnome.SettingsDaemon.Power",
    "org.gnome.SettingsDaemon.PrintNotifications",
    "org.gnome.SettingsDaemon.Rfkill",
    "org.gnome.SettingsDaemon.ScreensaverProxy",
    "org.gnome.SettingsDaemon.Sharing",
    "org.gnome.SettingsDaemon.Smartcard",
    "org.gnome.SettingsDaemon.Sound",
    "org.gnome.SettingsDaemon.UsbProtection",
    "org.gnome.SettingsDaemon.Wacom",
    "org.gnome.SettingsDaemon.XSettings",
    NULL,
};

static void check_required(void)
{
    int i, j, found;
    for (i = 0; required[i]; i++) {
        char want[NAMELEN + 16];
        found = 0;
        snprintf(want, sizeof(want), "%s.desktop", required[i]);
        for (j = 0; j < nfiles; j++)
            if (!strcmp(names[j], want)) { found = 1; break; }
        printf("PROBE: REQ %s %s\n", required[i], found ? "FOUND" : "MISSING");
    }
}

int main(int argc, char **argv)
{
    const char *dir = (argc > 1) ? argv[1] : "/etc/xdg/autostart";
    const char *e;

    setvbuf(stdout, NULL, _IOLBF, 0);
    printf("PROBE: BEGIN pid=%d ppid=%d uid=%d gid=%d\n",
           (int)getpid(), (int)getppid(), (int)getuid(), (int)getgid());
    e = getenv("XDG_CONFIG_DIRS");   printf("PROBE: ENV XDG_CONFIG_DIRS=%s\n", e ? e : "(null)");
    e = getenv("XDG_CONFIG_HOME");   printf("PROBE: ENV XDG_CONFIG_HOME=%s\n", e ? e : "(null)");
    e = getenv("XDG_DATA_DIRS");     printf("PROBE: ENV XDG_DATA_DIRS=%s\n", e ? e : "(null)");
    e = getenv("HOME");              printf("PROBE: ENV HOME=%s\n", e ? e : "(null)");
    e = getenv("DISPLAY");           printf("PROBE: ENV DISPLAY=%s\n", e ? e : "(null)");
    e = getenv("PATH");              printf("PROBE: ENV PATH=%s\n", e ? e : "(null)");

    scan_dir(dir, 1);
    scan_dir(dir, 2);       /* second pass: same DIR flow as glib re-scan */
    check_required();
    raw_getdents(dir);

    /* also check the session definition file the components come from */
    {
        const char *sfile = "/usr/share/gnome-session/sessions/gnome.session";
        char buf[8192];
        int fd = open(sfile, O_RDONLY);
        ssize_t n;
        if (fd < 0) {
            perr(sfile);
        } else {
            n = read(fd, buf, sizeof(buf) - 1);
            if (n < 0) perr("read session");
            else {
                buf[n] = 0;
                printf("PROBE: SESSION-FILE len=%zd req-in-file=%d\n", n,
                       strstr(buf, "RequiredComponents") ? 1 : 0);
            }
            close(fd);
        }
    }
    /* errno fidelity: missing path component must be ENOENT, not ENOTDIR */
    {
        const char *p = "/etc/dconf/profile/user";
        int fd = open(p, O_RDONLY);
        printf("PROBE: ERRNO-CHK open(%s) -> %d (%s) [Linux: ENOENT=2]\n",
               p, fd >= 0 ? 0 : errno, fd >= 0 ? "OPENED?!" : strerror(errno));
        if (fd >= 0) close(fd);
        p = "/etc/xdg/autostart/nope.desktop";
        fd = open(p, O_RDONLY);
        printf("PROBE: ERRNO-CHK open(%s) -> %d (%s)\n",
               p, fd >= 0 ? 0 : errno, fd >= 0 ? "OPENED?!" : strerror(errno));
        if (fd >= 0) close(fd);
    }
    printf("PROBE: END files=%d\n", nfiles);
    return 0;
}
