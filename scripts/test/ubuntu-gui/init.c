// udesk-init — PID 1 for the Ubuntu GUI session image.
//
// Mounts /proc, performs the proven one-shot /bin/true exec warmup (stabilises
// the first real exec after boot), then keeps the udesk framebuffer session
// alive on the console, respawning it if it ever exits.
//
// Shutdown cascade (PID 1 side):
// - SIGTERM (from shutdown(8) / reboot(2) cascade): terminate every user
//   process (TERM then KILL), then exit 0 — the kernel reboot(2) path is
//   waiting for exactly this before it syncs and powers the machine down.
// - SIGINT (from the kernel Ctrl-Alt-Del handler, Linux C.A.D semantics):
//   same teardown, then call reboot(RB_AUTOBOOT) ourselves — the kernel
//   cascade detects its caller IS PID 1, skips the signal step and goes
//   straight to sync + SBI reset.
#include <unistd.h>
#include <sys/mount.h>
#include <sys/wait.h>
#include <sys/reboot.h>
#include <dirent.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <errno.h>

static volatile sig_atomic_t g_shutdown; // SIGTERM -> exit
static volatile sig_atomic_t g_reboot;   // SIGINT (CAD) -> reboot(2)

static void on_sigterm(int sig) { (void)sig; g_shutdown = 1; }
static void on_sigint(int sig)  { (void)sig; g_reboot = 1; }

// SIGTERM every user process (numeric /proc entries), then SIGKILL the
// survivors after a grace period. PID 1 (us) is never a target.
static void kill_all(void) {
    for (int pass = 0; pass < 2; pass++) {
        int signo = pass == 0 ? SIGTERM : SIGKILL;
        DIR *d = opendir("/proc");
        if (!d) return;
        struct dirent *e;
        while ((e = readdir(d)) != NULL) {
            char *end;
            long pid = strtol(e->d_name, &end, 10);
            if (*end != '\0' || pid <= 1) continue; // not a pid / never us
            kill((pid_t)pid, signo);
        }
        closedir(d);
        if (pass == 0) usleep(300 * 1000); // let graceful exits finish
    }
}

static void teardown(const char *why) {
    printf("udesk-init: %s, stopping all processes\n", why);
    kill_all();
    printf("udesk-init: session down\n");
}

int main(void) {
    mount("proc", "/proc", "proc", 0, 0);
    setvbuf(stdout, NULL, _IONBF, 0);

    // sa_flags = 0: no SA_RESTART, so a blocked waitpid() surfaces EINTR
    // the moment a shutdown signal arrives.
    struct sigaction sa;
    memset(&sa, 0, sizeof sa);
    sigemptyset(&sa.sa_mask);
    sa.sa_handler = on_sigterm;
    sigaction(SIGTERM, &sa, NULL);
    sa.sa_handler = on_sigint;
    sigaction(SIGINT, &sa, NULL);

    printf("udesk-init: boot ok\n");

    {
        char *w[] = {"/bin/true", NULL};
        char *ev[] = {"HOME=/root", "PATH=/bin:/usr/bin:/sbin", NULL};
        pid_t wp = fork();
        if (wp == 0) { execve(w[0], w, ev); _exit(127); }
        int wst; waitpid(wp, &wst, 0);
        printf("udesk-init: warmup st=%d\n", wst);
    }

    for (;;) {
        if (g_shutdown || g_reboot) break;

        pid_t pid = fork();
        if (pid == 0) {
            char *av[] = {"/usr/bin/udesk", NULL};
            char *ev[] = {"HOME=/root", "PATH=/bin:/usr/bin:/sbin", "TERM=dumb", NULL};
            execve(av[0], av, ev);
            _exit(127);
        }
        // Poll instead of blocking: a shutdown signal must be honored
        // within one tick even if waitpid would auto-restart.
        for (;;) {
            if (g_shutdown || g_reboot) break;
            int st;
            pid_t r = waitpid(pid, &st, WNOHANG);
            if (r == pid) {
                printf("udesk-init: udesk st=%d, respawning\n", st);
                sleep(1);
                break;
            }
            if (r < 0 && errno == EINTR) continue;
            sleep(1);
        }
    }

    if (g_reboot) {
        teardown("Ctrl-Alt-Del (SIGINT)");
        printf("udesk-init: calling reboot(RB_AUTOBOOT)\n");
        reboot(RB_AUTOBOOT); // kernel: sync + SBI reset (we ARE pid 1)
        printf("udesk-init: reboot failed errno=%d\n", errno);
        _exit(1);
    }
    teardown("SIGTERM");
    printf("udesk-init: exiting, kernel cascade takes over\n");
    _exit(0);
}
