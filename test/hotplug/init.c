// hp-init — PID 1 for the udev/hotplug verification image
// (test/hotplug). C binary: Rux does not exec shebang scripts as init.
//
// Flow (markers grepped by test/hotplug/run.py on the serial console):
//   1. mount /proc /sys
//   2. start the uevent netlink daemon (uevent_probe -m /bin/mdev)
//   3. coldplug: delete /dev/vda + /dev/input/event*, run `busybox mdev -s`
//      (nodes must be rebuilt purely from the /sys scan) → HP:COLD-OK/FAIL
//   4. HP:READY, then an exec loop: each console line runs under
//      /bin/sh -c and replies "CMD-DONE <status>" — the host drives the
//      uevent-file trigger and the PCI-rescan hotplug phase this way.
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>

static int exists(const char *path) {
    struct stat st;
    return stat(path, &st) == 0;
}

static int is(const char *path, int mode_bits) {
    struct stat st;
    return stat(path, &st) == 0 && (st.st_mode & S_IFMT) == mode_bits;
}

static void run(const char *path, char *const argv[]) {
    pid_t pid = fork();
    if (pid == 0) {
        execv(path, argv);
        fprintf(stderr, "exec %s: %s\n", path, strerror(errno));
        _exit(127);
    }
    int st;
    waitpid(pid, &st, 0);
}

int main(void) {
    setvbuf(stdout, NULL, _IONBF, 0);

    mount("proc", "/proc", "proc", 0, 0);
    mount("sysfs", "/sys", "sysfs", 0, 0);
    printf("hp-init: /proc /sys mounted\n");

    // uevent daemon: netlink listener that execs mdev per event
    {
        char *av[] = {"/bin/uevent_probe", "-m", "/bin/mdev", NULL};
        char *ev[] = {"PATH=/bin:/sbin:/usr/bin:/usr/sbin", NULL};
        pid_t pid = fork();
        if (pid == 0) {
            int logfd = open("/run/hpevent.log", O_WRONLY | O_CREAT | O_TRUNC, 0644);
            if (logfd >= 0) { dup2(logfd, 1); dup2(logfd, 2); }
            execve(av[0], av, ev);
            _exit(127);
        }
        printf("hp-init: uevent daemon started (pid %d)\n", pid);
    }

    // coldplug: remove kernel-created nodes, rebuild from /sys via mdev -s
    unlink("/dev/vda");
    unlink("/dev/input/event0");
    unlink("/dev/input/event1");
    {
        char *av[] = {"/bin/busybox", "mdev", "-s", NULL};
        run("/bin/busybox", av);
    }

    const char *fail = "";
    if (!is("/dev/vda", S_IFBLK)) fail = " vda-block-node";
    else if (!is("/dev/input/event0", S_IFCHR)) fail = " event0-node";
    else if (!is("/dev/input/event1", S_IFCHR)) fail = " event1-node";
    else if (!exists("/sys/class/block/vda/dev")) fail = " sysfs-vda";
    else if (!exists("/sys/class/input/event0/dev")) fail = " sysfs-event0";
    else if (!exists("/sys/class/block/vda/removable")) fail = " sysfs-removable";
    if (fail[0] == '\0') {
        printf("HP:COLD-OK\n");
    } else {
        printf("HP:COLD-FAIL%s\n", fail);
    }
    printf("HP:READY\n");

    // exec loop: host sends one shell command per line
    char line[512];
    for (;;) {
        printf("RES> ");
        if (!fgets(line, sizeof(line), stdin)) break;
        size_t n = strlen(line);
        while (n > 0 && (line[n - 1] == '\n' || line[n - 1] == '\r')) line[--n] = '\0';
        if (n == 0) continue;
        pid_t pid = fork();
        if (pid == 0) {
            char *av[] = {"/bin/busybox", "sh", "-c", line, NULL};
            execv("/bin/busybox", av);
            _exit(127);
        }
        int st;
        waitpid(pid, &st, 0);
        printf("CMD-DONE %d\n", WIFEXITED(st) ? WEXITSTATUS(st) : -1);
    }
    return 0;
}
