/* pathprobe.c — functional probe for the perf-path lane's walk changes.
 * Boots as PID 1 (init=/test/pathprobe) on the lmbench image and
 * exercises the paths the lock-free fast paths touch: dcache-hit
 * stat/open (hot), symlink follows, /proc mount traversal, trailing
 * slashes, relative paths, deep paths (>16 components, the stack-array
 * spill), long paths (>127 bytes, the NUL-gated probe fallback), and
 * repeated open/close. Prints one PASS/FAIL line per check, then
 * powers off.
 */
#include <stdio.h>
#include <string.h>
#include <unistd.h>
#include <fcntl.h>
#include <sys/stat.h>
#include <sys/mount.h>
#include <sys/reboot.h>
#include <errno.h>

static int fails;

static void chk(const char *what, int ok)
{
    printf("%s: %s\n", what, ok ? "PASS" : "FAIL");
    if (!ok)
        fails++;
}

int main(void)
{
    struct stat st;
    int fd, rc;
    mkdir("/tmp", 0755);

    /* 1. hot stat/open of an existing file (dcache-hit walk) */
    rc = stat("/usr/include/linux/types.h", &st);
    chk("stat existing", rc == 0 && st.st_size > 0);
    fd = open("/usr/include/linux/types.h", O_RDONLY);
    chk("open existing", fd >= 0);
    if (fd >= 0)
        close(fd);

    /* 2. repeated open/close cycles (fdtable alloc+install merge) */
    int ok = 1;
    for (int i = 0; i < 200; i++) {
        int f = open("/usr/include/linux/types.h", O_RDONLY);
        if (f < 0) { ok = 0; break; }
        if (close(f) != 0) { ok = 0; break; }
    }
    chk("200x open/close", ok);
    /* lowest-fd discipline after the churn */
    int a = open("/usr/include/linux/types.h", O_RDONLY);
    int b = open("/usr/include/linux/types.h", O_RDONLY);
    chk("sequential fds", b == a + 1);
    close(a); close(b);

    /* 3. ENOENT for a missing file */
    rc = stat("/usr/include/linux/definitely-missing", &st);
    chk("stat missing ENOENT", rc == -1 && errno == ENOENT);

    /* 4. symlink follow + lstat nofollow */
    ok = (symlink("/usr/include/linux/types.h", "/tmp/link1") == 0);
    rc = ok ? stat("/tmp/link1", &st) : -1;
    chk("stat symlink follows", rc == 0 && st.st_size > 0);
    rc = lstat("/tmp/link1", &st);
    chk("lstat symlink nofollow", rc == 0 && S_ISLNK(st.st_mode));

    /* 5. relative symlink + chained symlink */
    ok = 1;
    rc = chdir("/usr/include/linux");
    if (rc == 0) {
        symlink("types.h", "rel1");   /* link in the CWD, relative target */
        rc = stat("rel1", &st);
        ok = rc == 0 && st.st_size > 0;
        unlink("rel1");
        chdir("/");
    }
    chk("relative symlink", ok);
    symlink("/tmp/link1", "/tmp/link2");
    rc = stat("/tmp/link2", &st);
    chk("chained symlink", rc == 0);
    unlink("/tmp/rel1"); unlink("/tmp/link2");

    /* 6. mount traversal: /proc is a mountpoint in this image */
    rc = stat("/proc/self/exe", &st);
    chk("proc magic link", rc == 0 && S_ISREG(st.st_mode));
    rc = stat("/proc/self", &st);
    chk("proc dir across mount", rc == 0 && S_ISDIR(st.st_mode));

    /* 7. trailing slash on a regular file = ENOTDIR */
    rc = stat("/usr/include/linux/types.h/", &st);
    chk("trailing slash ENOTDIR", rc == -1 && errno == ENOTDIR);

    /* 8. dot components */
    rc = stat("/usr/./include/../include/linux/./types.h", &st);
    chk("dot/dotdot components", rc == 0 && st.st_size > 0);

    /* 9. deep path: 20 components (stack array spills to Vec) */
    ok = 1;
    char deep[256] = "/tmp";
    for (int i = 0; i < 20; i++) {
        char comp[16];
        snprintf(comp, sizeof(comp), "/d%d", i);
        strcat(deep, comp);
        if (mkdir(deep, 0755) != 0) { ok = 0; break; }
    }
    strcat(deep, "/leaf");
    if (ok) {
        int f = open(deep, O_CREAT | O_WRONLY, 0644);
        if (f < 0) ok = 0; else close(f);
        struct stat st2;
        if (ok && stat(deep, &st2) != 0) ok = 0;
    }
    chk("deep path (spill)", ok);

    /* 10. long absolute path > 127 bytes (probe NUL gate falls back) */
    {
        char longp[512];
        strcpy(longp, "/tmp");
        for (int i = 0; i < 24; i++) strcat(longp, "/component-name-padding");
        strcat(longp, "/leaf");
        /* reuse the deep tree by making one more long chain */
        ok = 1;
        char *p = longp;
        while (ok) {
            char *slash = strchr(p + 1, '/');
            if (!slash) break;
            *slash = 0;
            if (mkdir(longp, 0755) != 0 && errno != EEXIST) { ok = 0; break; }
            *slash = '/';
            p = slash;
        }
        if (ok) {
            int f = open(longp, O_CREAT | O_WRONLY, 0644);
            if (f < 0) ok = 0; else close(f);
            struct stat st2;
            if (stat(longp, &st2) != 0) ok = 0;
        }
        chk("long path (fallback)", ok);
    }

    /* 11. openat relative to dirfd */
    {
        int d = open("/usr/include/linux", O_RDONLY | O_DIRECTORY);
        struct stat st2;
        rc = fstatat(d, "types.h", &st2, 0);
        chk("fstatat dirfd relative", d >= 0 && rc == 0 && st2.st_size > 0);
        close(d);
    }

    /* 12. unlink + re-stat (dentry detach, icache remove path) */
    {
        int f = open("/tmp/recreate", O_CREAT | O_WRONLY, 0644);
        if (f >= 0) close(f);
        struct stat st2;
        ok = stat("/tmp/recreate", &st2) == 0;
        ok = ok && unlink("/tmp/recreate") == 0;
        ok = ok && stat("/tmp/recreate", &st2) == -1;
        chk("unlink then stat ENOENT", ok);
        /* recreate the same name: icache identity regen path */
        f = open("/tmp/recreate", O_CREAT | O_WRONLY, 0644);
        ok = f >= 0;
        if (ok) close(f);
        ok = ok && stat("/tmp/recreate", &st2) == 0;
        chk("recreate same name", ok);
        unlink("/tmp/recreate");
    }

    printf("PATHPROBE %s (%d failures)\n", fails ? "FAIL" : "PASS", fails);
    fflush(stdout);
    sync();
    reboot(RB_POWER_OFF);
    return 0;
}
