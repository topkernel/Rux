// uevent_probe — NETLINK_KOBJECT_UEVENT listener for the Rux minimal
// udev/hotplug path (test/hotplug).
//
// Binds AF_NETLINK/SOCK_RAW/NETLINK_KOBJECT_UEVENT (protocol 15) and
// prints every kernel uevent as one block:
//
//   add@/class/block/vdb
//   ACTION=add
//   DEVPATH=/class/block/vdb
//   SUBSYSTEM=block
//   MAJOR=254
//   MINOR=16
//   DEVNAME=vdb
//   SEQNUM=7
//
// Modes:
//   uevent_probe            — print events forever
//   uevent_probe -c         — coldplug dump: print events, then exit after
//                             -t seconds of silence (default 3)
//   uevent_probe -m /bin/mdev
//                           — udev-like daemon: on every event, fork+exec
//                             MDEV with ACTION/DEVPATH/MAJOR/MINOR/DEVNAME/
//                             SUBSYSTEM/SEQNUM exported (exactly the env a
//                             kernel uevent_helper would provide), so node
//                             creation is genuinely netlink-driven.
//
// Build: riscv64-linux-gnu-gcc -static -O2 -o uevent_probe uevent_probe.c

#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/wait.h>
#include <unistd.h>

#ifndef NETLINK_KOBJECT_UEVENT
#define NETLINK_KOBJECT_UEVENT 15
#endif

struct sockaddr_nl {
    unsigned short nl_family;
    unsigned short nl_pad;
    unsigned int nl_pid;
    unsigned int nl_groups;
};

static volatile sig_atomic_t got_alarm;

static void on_alarm(int sig) {
    (void)sig;
    got_alarm = 1;
}

static void print_event(const char *buf, ssize_t len) {
    // Payload: "action@devpath\0KEY=VALUE\0...\0KEY=VALUE\0" (no nlmsghdr).
    const char *p = buf;
    const char *end = buf + len;
    int nfield = 0;
    printf("UEVENT-BEGIN\n");
    while (p < end) {
        const char *nul = memchr(p, '\0', (size_t)(end - p));
        size_t flen = nul ? (size_t)(nul - p) : (size_t)(end - p);
        if (flen > 0) {
            if (nfield == 0) {
                // Header: split action@devpath for readability.
                const char *at = memchr(p, '@', flen);
                if (at) {
                    printf("ACTION@DEVPATH=%.*s | devpath=%.*s\n",
                           (int)(at - p), p,
                           (int)(flen - (at - p) - 1), at + 1);
                } else {
                    printf("%.*s\n", (int)flen, p);
                }
            } else {
                printf("%.*s\n", (int)flen, p);
            }
            nfield++;
        }
        p = nul ? nul + 1 : end;
    }
    printf("UEVENT-END\n");
    fflush(stdout);
}

// Export the parsed KEY=VALUE pairs and exec mdev (hotplug mode).
static void run_mdev(const char *mdev, char *const env_keep[]) {
    pid_t pid = fork();
    if (pid < 0) {
        perror("fork");
        return;
    }
    if (pid == 0) {
        // Environ = the uevent KEY=VALUEs only (mdev reads ACTION/DEVPATH/
        // MAJOR/MINOR/DEVNAME/SUBSYSTEM from its environment).
        char *argv[] = {(char *)mdev, NULL};
        execve(mdev, argv, env_keep);
        perror("execve mdev");
        _exit(127);
    }
    int st;
    waitpid(pid, &st, 0);
    printf("mdev exit=%d\n", WIFEXITED(st) ? WEXITSTATUS(st) : -1);
    fflush(stdout);
}

// Split the payload into an env vector (KEY=VALUE strings, NUL-separated
// in place). Returns the count; vector is caller-freed (strings point
// into buf).
static int payload_to_env(char *buf, ssize_t len, char **env, int max) {
    int n = 0;
    char *p = buf;
    char *end = buf + len;
    while (p < end && n < max - 1) {
        char *nul = memchr(p, '\0', (size_t)(end - p));
        if (!nul) break;
        // Skip the "action@devpath" header (before the first '=').
        if (nul != p && memchr(p, '=', (size_t)(nul - p))) {
            env[n++] = p;
        }
        p = nul + 1;
    }
    env[n] = NULL;
    return n;
}

int main(int argc, char **argv) {
    const char *mdev = NULL;
    int coldboot = 0;
    int quiet_secs = 3;

    for (int i = 1; i < argc; i++) {
        if (!strcmp(argv[i], "-c")) {
            coldboot = 1;
        } else if (!strcmp(argv[i], "-m") && i + 1 < argc) {
            mdev = argv[++i];
        } else if (!strcmp(argv[i], "-t") && i + 1 < argc) {
            quiet_secs = atoi(argv[++i]);
        } else {
            fprintf(stderr, "usage: %s [-c] [-t secs] [-m /path/to/mdev]\n", argv[0]);
            return 2;
        }
    }

    int fd = socket(AF_NETLINK, SOCK_RAW, NETLINK_KOBJECT_UEVENT);
    if (fd < 0) {
        perror("socket(AF_NETLINK, NETLINK_KOBJECT_UEVENT)");
        return 1;
    }

    struct sockaddr_nl addr;
    memset(&addr, 0, sizeof(addr));
    addr.nl_family = AF_NETLINK;
    addr.nl_pid = 0;     // kernel assigns
    addr.nl_groups = 1;  // kernel uevent multicast group
    if (bind(fd, (struct sockaddr *)&addr, sizeof(addr)) < 0) {
        // Non-fatal: Rux accepts the bind but does not model groups.
        printf("bind: %s (continuing)\n", strerror(errno));
    }

    printf("uevent_probe: listening (mdev=%s)\n", mdev ? mdev : "-");
    fflush(stdout);

    if (coldboot) {
        signal(SIGALRM, on_alarm);
    }

    char buf[8192];
    for (;;) {
        if (coldboot) {
            alarm((unsigned)quiet_secs);
        }
        ssize_t n = recv(fd, buf, sizeof(buf) - 1, 0);
        if (coldboot) {
            alarm(0);
            if (n < 0 && errno == EINTR && got_alarm) {
                printf("uevent_probe: quiet %ds, coldboot done\n", quiet_secs);
                break;
            }
        }
        if (n < 0) {
            if (errno == EINTR) continue;
            perror("recv");
            return 1;
        }
        buf[n] = '\0';
        print_event(buf, n);
        if (mdev) {
            char *env[32];
            payload_to_env(buf, n, env, 32);
            run_mdev(mdev, env);
        }
    }

    close(fd);
    return 0;
}
