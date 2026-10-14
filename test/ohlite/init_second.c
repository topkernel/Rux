/*
 * OH-lite second-stage init — the /bin/init stand-in that OH execs after
 * switch_root to /usr (system.img).
 *
 * Replicates the OH second-stage bring-up shape:
 *   - re-verify /proc /sys /dev survived switch_root (MS_MOVE kept them)
 *   - write the /proc/sys knobs OH's init.cfg touches first (tolerated)
 *   - coldplug the misc devices OH's ueventd.config creates from
 *     /sys/class/misc/<dev>/dev: binder hwbinder vndbinder ashmem
 *     access_token_id
 *   - "ServiceStart" each stand-in service, respawning on exit (init's
 *     restart semantics), and stay alive >= 60s — the runbook's Phase 1
 *     gate — printing heartbeats with the alive count.
 *
 * Services (see svc_oh.c): binder (samgr stand-in: open+mmap+VERSION+
 * SET_MAX_THREADS+CONTEXT_MGR+ENTER_LOOPER then park), ashmem, tokenid.
 */

#define _GNU_SOURCE
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <sys/sysmacros.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

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

static void sigchld(int sig) { (void)sig; }

struct svc {
    const char *name;
    const char *bin;
    pid_t pid;
    int restarts;
};

static struct svc services[] = {
    {"samgr-lite", "/bin/svc_binder", 0, 0},
    {"ashmem-lite", "/bin/svc_ashmem", 0, 0},
    {"tokenid-lite", "/bin/svc_tokenid", 0, 0},
};
#define NSVC (int)(sizeof(services) / sizeof(services[0]))

/* Coldplug one misc device the way ueventd does: read "maj:min" from
 * /sys/class/misc/<n>/dev and mknod /dev/<n>. */
static int coldplug_misc(const char *name)
{
    char p[160], mm[32];
    snprintf(p, sizeof(p), "/sys/class/misc/%s/dev", name);
    int fd = open(p, O_RDONLY);
    if (fd < 0)
        return -1;
    ssize_t n = read(fd, mm, sizeof(mm) - 1);
    close(fd);
    if (n <= 0)
        return -1;
    mm[n] = '\0';
    int major, minor;
    if (sscanf(mm, "%d:%d", &major, &minor) != 2)
        return -1;
    char devp[96];
    snprintf(devp, sizeof(devp), "/dev/%s", name);
    unlink(devp);
    if (mknod(devp, S_IFCHR | 0660, makedev(major, minor)) != 0)
        return -1;
    return 0;
}

static void start_service(struct svc *s)
{
    pid_t p = fork();
    if (p < 0) {
        ohlog("fork for %s failed: %s", s->name, strerror(errno));
        return;
    }
    if (p == 0) {
        char *argv[] = {(char *)s->bin, NULL};
        char *env[] = {"PATH=/bin:/usr/bin", "HOME=/root", "TERM=linux", NULL};
        execve(s->bin, argv, env);
        ohlog("SVC %s execve failed: %s", s->name, strerror(errno));
        _exit(127);
    }
    s->pid = p;
    ohlog("ServiceStart %s pid %d", s->name, p);
}

int main(int argc, char **argv)
{
    ohlog("second-stage init running (pid %d, argv0 %s, argc %d)", getpid(),
          argv[0], argc);
    for (int i = 1; i < argc; i++)
        ohlog("arg%d: %s", i, argv[i]);

    /* fds survive switch_root: stdout is still the UART console; kmsg was
     * re-created by us below (the /dev tmpfs moved with us) */
    kmsg_fd = open("/dev/kmsg", O_WRONLY);

    /* /proc sanity — cmdline readable through the moved mount */
    {
        char b[512];
        int fd = open("/proc/cmdline", O_RDONLY);
        if (fd >= 0) {
            ssize_t n = read(fd, b, sizeof(b) - 1);
            close(fd);
            if (n > 0) {
                b[n] = '\0';
                ohlog("M5 proc alive after switch_root (cmdline %d bytes)", (int)n);
            }
        } else {
            ohlog("M5 proc NOT alive after switch_root: %s", strerror(errno));
        }
    }

    /* /proc/sys surface OH init writes early — must not wedge us */
    const char *knobs[] = {
        "/proc/sys/kernel/printk_devkmsg",
        "/proc/sys/kernel/randomize_va_space",
        "/proc/sys/kernel/oom_score_adj", /* dir knobs use children; tolerate */
    };
    for (size_t i = 0; i < sizeof(knobs) / sizeof(knobs[0]); i++) {
        int fd = open(knobs[i], O_WRONLY);
        if (fd < 0) {
            ohlog("knob %s open failed: %s (tolerated)", knobs[i],
                  strerror(errno));
            continue;
        }
        ssize_t w = write(fd, "0\n", 2);
        (void)w;
        close(fd);
        ohlog("knob %s written (tolerated result)", knobs[i]);
    }

    /* device nodes for the OH ABI devices */
    const char *miscs[] = {"binder", "hwbinder", "vndbinder", "ashmem",
                           "access_token_id"};
    for (size_t i = 0; i < sizeof(miscs) / sizeof(miscs[0]); i++) {
        if (coldplug_misc(miscs[i]) == 0)
            ohlog("M6 coldplug misc %s ok", miscs[i]);
        else
            ohlog("M6 coldplug misc %s FAILED (errno %d)", miscs[i], errno);
    }

    /* cgroup v2 surface memgr touches: mount (likely already) + mkdir */
    mkdir("/dev/memcg", 0755);
    if (mount("cgroup", "/sys/fs/cgroup", "cgroup2", 0, NULL) == 0)
        ohlog("M6 cgroup2 mounted at /sys/fs/cgroup");
    else
        ohlog("M6 cgroup2 mount: %s (tolerated)", strerror(errno));

    /* service supervisor loop — the runbook gate: stay up 60s */
    signal(SIGCHLD, sigchld);
    for (int i = 0; i < NSVC; i++)
        start_service(&services[i]);

    time_t start = time(NULL);
    int beat = 0;
    for (;;) {
        int status;
        pid_t d;
        while ((d = waitpid(-1, &status, WNOHANG)) > 0) {
            for (int i = 0; i < NSVC; i++) {
                if (services[i].pid == d) {
                    ohlog("SVC %s pid %d exited status %d — restarting",
                          services[i].name, d, status);
                    services[i].restarts++;
                    sleep(1);
                    start_service(&services[i]);
                }
            }
        }
        sleep(5);
        beat++;
        int alive = 0;
        for (int i = 0; i < NSVC; i++)
            if (kill(services[i].pid, 0) == 0 || services[i].pid == 0)
                alive++;
        time_t up = time(NULL) - start;
        ohlog("M7 second-stage up %lds beat %d alive %d/%d restarts %d/%d/%d",
              (long)up, beat, alive, NSVC, services[0].restarts,
              services[1].restarts, services[2].restarts);
        if (up >= 60)
            ohlog("BOOT GATE PASS: second-stage init stayed up 60s");
    }
    return 0;
}
