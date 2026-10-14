// runit.c — static session bootstrap for the GNOME x86_64 image.
//
// The kernel's x86 init path does not interpret shebang scripts (the
// riscv64 path does), so the gp3i-style /sbin/runit shell script is
// compiled to a static binary instead. Chain: clear stale X locks,
// system dbus, session dbus (address captured through a pipe so every
// later client shares ONE bus), Xorg on fbdev, then the full
// gnome-session (session file "rux": org.gnome.Shell is the only
// required component — gsd is not installed in this image). If the
// session dies within 150s of launch, fall back once to the proven
// minimal path (gnome-shell --replace) so the boot still paints.
// PID1 parks in a reap loop; when Xorg/the session dies it dumps the
// logs to the console for bring-up forensics.
#include <sys/types.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <fcntl.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

static void spawn_stdin_null(const char *file, char *const argv[]) {
    pid_t pid = fork();
    if (pid == 0) {
        int devnull = open("/dev/null", O_RDONLY);
        if (devnull >= 0) { dup2(devnull, STDIN_FILENO); close(devnull); }
        setsid();
        execvp(file, argv);
        _exit(127);
    }
}

// Start the session dbus daemon and capture its printed address
// ("unix:abstract=..." / "unix:path=...") through a pipe, so the whole
// session shares one bus instead of every client dbus-launching its own.
// The explicit --address pins a pathname socket (Rux AF_UNIX resolves
// pathname bindings; abstract worked for dbus-launch too, but the path
// form is debuggable from the shell). If --address is rejected by this
// dbus build, the caller logs it and clients fall back to autolaunch.
static int start_session_bus(char *addr_out, size_t addr_cap) {
    int fds[2];
    if (pipe(fds) < 0) return -1;
    pid_t pid = fork();
    if (pid == 0) {
        close(fds[0]);
        dup2(fds[1], STDOUT_FILENO);
        dup2(fds[1], STDERR_FILENO);
        close(fds[1]);
        int devnull = open("/dev/null", O_RDONLY);
        if (devnull >= 0) { dup2(devnull, STDIN_FILENO); close(devnull); }
        setsid();
        execl("/usr/bin/dbus-daemon", "dbus-daemon", "--session", "--fork",
              "--address=unix:path=/tmp/dbus-session.sock",
              "--print-address=1", "--nopidfile", (char *)NULL);
        _exit(127);
    }
    close(fds[1]);
    ssize_t n = read(fds[0], addr_out, addr_cap - 1);
    close(fds[0]);
    if (n <= 0) return -1;
    addr_out[n] = 0;
    char *nl = strchr(addr_out, '\n');
    if (nl) *nl = 0;
    int st; waitpid(pid, &st, 0);
    if (strncmp(addr_out, "unix:", 5) != 0) return -1; // exec/config failure text
    return 0;
}

// Wait until Xorg's socket shows up (or give up after `sec` seconds).
static void wait_for_x(int sec) {
    for (int i = 0; i < sec; i++) {
        struct stat st;
        if (stat("/tmp/.X11-unix/X0", &st) == 0) return;
        sleep(1);
    }
}

// Launch a session child with stdout/stderr to `logpath`, stdin null.
static pid_t launch_logged(const char *logpath, const char *file,
                           const char *arg0, const char *const *argv) {
    int log = open(logpath, O_WRONLY | O_CREAT | O_TRUNC, 0644);
    pid_t pid = fork();
    if (pid == 0) {
        if (log >= 0) { dup2(log, STDOUT_FILENO); dup2(log, STDERR_FILENO); }
        int devnull = open("/dev/null", O_RDONLY);
        if (devnull >= 0) { dup2(devnull, STDIN_FILENO); close(devnull); }
        setsid();
        execv(file, (char *const *)argv);
        printf("[runit] exec %s failed errno-dummy\n", file);
        _exit(127);
    }
    if (log >= 0) close(log);
    (void)arg0;
    return pid;
}

// Print "== dump PATH ==" then `tail -n <n> PATH` to the console.
static void dump_log_prefix(const char *path, int n) {
    printf("== dump %s (tail) ==\n", path);
    fflush(stdout);
    char nbuf[8];
    snprintf(nbuf, sizeof nbuf, "%d", n);
    pid_t d = fork();
    if (d == 0) {
        int con = open("/dev/console", O_WRONLY);
        if (con >= 0) { dup2(con, STDOUT_FILENO); dup2(con, STDERR_FILENO); }
        execl("/usr/bin/tail", "tail", "-n", nbuf, path, (char *)NULL);
        _exit(127);
    }
    int st; waitpid(d, &st, 0);
}

static void dump_log(const char *path) {
    dump_log_prefix(path, 40);
}

int main(void) {
    // NOTE: SIGCHLD must stay SIG_DFL — SIG_IGN is inherited across
    // fork AND exec, and it makes gnome-session/glib waitpid() calls
    // return ECHILD (auto-reap), so every spawned component looks like
    // an instant failure. Any daemons forked here leave at most a
    // couple of zombies until the reap loop at the bottom collects them.

    // PID1's inherited stdout is not the console (kernel init path):
    // attach it explicitly so every printf below reaches the serial log.
    {
        int con = open("/dev/console", O_WRONLY);
        if (con >= 0) {
            dup2(con, STDOUT_FILENO);
            dup2(con, STDERR_FILENO);
            close(con);
        }
    }

    // Mode selection: /etc/runit-mode ("gs" or "session") so the same
    // image boots either chain without a rebuild. Default "session"
    // falls back to "gs" if the supervised session dies early.
    // Rationale: gnome-session-binary kills the shell at its hardcoded
    // 20s register timeout — far too short under single-threaded TCG,
    // where the SpiderMonkey boot needs minutes. The gs mode runs
    // gnome-shell --replace unsupervised (no register deadline).
    char modebuf[16] = "";
    {
        int mf = open("/etc/runit-mode", O_RDONLY);
        if (mf >= 0) {
            ssize_t n = read(mf, modebuf, sizeof modebuf - 1);
            close(mf);
            if (n > 0) {
                modebuf[n] = 0;
                for (int i = 0; modebuf[i]; i++)
                    if (modebuf[i] == '\n' || modebuf[i] == '\r') modebuf[i] = 0;
            }
        }
    }
    const char *MODE = (modebuf[0] == 'g') ? "gs" : "session";
    printf("[runit] mode=%s\n", MODE);

    setenv("PATH", "/usr/bin:/usr/sbin:/bin:/sbin", 1);
    setenv("HOME", "/root", 1);
    setenv("USER", "root", 1);
    setenv("LANG", "C.UTF-8", 1);
    setenv("XDG_RUNTIME_DIR", "/tmp/xdg", 1);
    setenv("DISPLAY", ":0", 1);
    setenv("XDG_SESSION_TYPE", "x11", 1);
    setenv("XDG_CURRENT_DESKTOP", "GNOME", 1);
    setenv("LIBGL_ALWAYS_SOFTWARE", "1", 1);
    setenv("GALLIUM_DRIVER", "llvmpipe", 1);
    mkdir("/tmp/xdg", 0700);

    // auto-clear stale X locks (an unclean prior run would wedge Xorg)
    unlink("/tmp/.X0-lock");
    unlink("/tmp/.X11-unix/X0");

    printf("[runit] starting system dbus\n");
    mkdir("/run/dbus", 0755);
    spawn_stdin_null("dbus-daemon", (char *[]){ "dbus-daemon", "--system", "--fork",
        "--print-address", NULL });
    sleep(1);

    char addr[512] = "";
    if (start_session_bus(addr, sizeof addr) == 0 && addr[0]) {
        printf("[runit] session bus: %s\n", addr);
        setenv("DBUS_SESSION_BUS_ADDRESS", addr, 1);
    } else {
        printf("[runit] session bus capture FAILED (%s); clients will dbus-launch\n", addr);
    }

    printf("[runit] starting Xorg (fbdev)\n");
    // Xorg output goes to the serial console (console=ttyS0) for
    // headless bring-up diagnosis; the log file copy stays as well.
    pid_t xpid = fork();
    if (xpid == 0) {
        int xlog = open("/tmp/Xorg.log", O_WRONLY | O_CREAT | O_TRUNC, 0644);
        if (xlog >= 0) { dup2(xlog, STDOUT_FILENO); dup2(xlog, STDERR_FILENO); }
        int con = open("/dev/console", O_WRONLY);
        if (con >= 0) { dup2(con, STDOUT_FILENO); dup2(con, STDERR_FILENO); }
        int devnull = open("/dev/null", O_RDONLY);
        if (devnull >= 0) { dup2(devnull, STDIN_FILENO); close(devnull); }
        setsid();
        // /usr/bin/Xorg is a shell wrapper and the kernel does not
        // interpret shebangs on the init path — exec the real server
        // binary directly.
        execl("/usr/lib/xorg/Xorg", "Xorg", ":0",
              "-config", "/etc/X11/xorg-fbdev.conf",
              "-noreset", "-verbose", "3", (char *)NULL);
        _exit(127);
    }
    wait_for_x(30);
    sleep(2); // let GLX initialize before the first client connects

    printf("[runit] starting session chain (mode=%s)\n", MODE);
    // Periodic log tailer: every 20s push the tail of the session logs
    // to the console, so bring-up forensics do not depend on death
    // detection working.
    {
        pid_t tp = fork();
        if (tp == 0) {
            for (;;) {
                sleep(20);
                const char *logs[3] = { "/tmp/gnome-session.log",
                                        "/tmp/gnome-shell.log",
                                        "/tmp/Xorg.log" };
                for (int i = 0; i < 3; i++) {
                    struct stat st;
                    if (stat(logs[i], &st) != 0) continue;
                    dump_log_prefix(logs[i], 150);
                }
            }
            _exit(0);
        }
    }
    if (strcmp(MODE, "gs") == 0) {
        // Unsupervised shell: no register timeout, unlimited JS boot
        // time (the TCG-friendly bring-up path).
        pid_t gpid = launch_logged("/tmp/gnome-shell.log",
                                   "/usr/bin/gnome-shell", "gnome-shell",
                                   (const char *const []){
                                       "gnome-shell", "--replace", NULL });
        signal(SIGCHLD, SIG_DFL);
        for (;;) {
            int st;
            pid_t p = wait(&st);
            if (p > 0) {
                printf("[runit] child %d exited st=%d\n", (int)p, st);
                if (p == gpid) { dump_log("/tmp/gnome-shell.log"); dump_log("/tmp/Xorg.log"); }
            } else {
                sleep(2);
            }
        }
    }
    {
        const char *const argv[] = { "gnome-session-binary", "--session=rux", NULL };
        pid_t gpid = launch_logged("/tmp/gnome-session.log",
                                   "/usr/libexec/gnome-session-binary",
                                   "gnome-session-binary", argv);
        time_t t0 = time(NULL);
        signal(SIGCHLD, SIG_DFL);
        int gs_dead = 0;
        for (;;) {
            int st;
            pid_t p = wait(&st);
            if (p == gpid) {
                gs_dead = 1;
                printf("[runit] gnome-session died st=%d after %lds\n",
                       st, (long)(time(NULL) - t0));
                dump_log("/tmp/gnome-session.log");
                dump_log("/tmp/gnome-shell.log");
                if ((long)(time(NULL) - t0) < 150) break; // early death -> fallback
                // late death: keep reaping, no restart (bring-up mode)
                gpid = -1;
            } else if (p > 0) {
                printf("[runit] child %d exited st=%d\n", (int)p, st);
            } else {
                sleep(2);
            }
            if (p == xpid) {
                dump_log("/tmp/Xorg.log");
                break;
            }
        }
        if (gs_dead) {
            printf("[runit] falling back to minimal path: gnome-shell --replace\n");
            const char *const argv2[] = { "gnome-shell", "--replace", NULL };
            gpid = launch_logged("/tmp/gnome-shell.log",
                                 "/usr/bin/gnome-shell", "gnome-shell", argv2);
            for (;;) {
                int st;
                pid_t p = wait(&st);
                if (p > 0) {
                    printf("[runit] child %d exited st=%d\n", (int)p, st);
                    if (p == gpid) { dump_log("/tmp/gnome-shell.log"); dump_log("/tmp/Xorg.log"); }
                } else {
                    sleep(2);
                }
            }
        }
        // Xorg died under a live session — park reaping.
        for (;;) {
            int st;
            pid_t p = wait(&st);
            if (p > 0) printf("[runit] child %d exited st=%d\n", (int)p, st);
            else sleep(2);
        }
    }
    return 0;
}
