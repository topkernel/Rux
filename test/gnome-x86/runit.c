// runit.c — static session bootstrap for the GNOME x86_64 image.
//
// The kernel's x86 init path does not interpret shebang scripts (the
// riscv64 path does), so the gp3i-style /sbin/runit shell script is
// compiled to a static binary instead. Same chain: clear stale X locks,
// both dbus daemons, Xorg on fbdev, then the minimal gnome-shell path;
// finally exec a console shell so the serial port stays usable.
#include <sys/types.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <fcntl.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static void spawn(const char *file, char *const argv[]) {
    pid_t pid = fork();
    if (pid == 0) {
        int devnull = open("/dev/null", O_RDONLY);
        if (devnull >= 0) { dup2(devnull, STDIN_FILENO); close(devnull); }
        setsid();
        execvp(file, argv);
        _exit(127);
    }
}

static void read_first_line(const char *path, char *buf, size_t n) {
    buf[0] = 0;
    int fd = open(path, O_RDONLY);
    if (fd < 0) return;
    ssize_t r = read(fd, buf, n - 1);
    close(fd);
    if (r <= 0) buf[0] = 0; else buf[r] = 0;
    char *nl = strchr(buf, '\n');
    if (nl) *nl = 0;
}

int main(void) {
    signal(SIGCHLD, SIG_IGN);
    setenv("PATH", "/usr/bin:/usr/sbin:/bin:/sbin", 1);
    setenv("HOME", "/root", 1);
    setenv("XDG_RUNTIME_DIR", "/tmp/xdg", 1);
    mkdir("/tmp/xdg", 0700);

    // auto-clear stale X locks (an unclean prior run would wedge Xorg)
    unlink("/tmp/.X0-lock");
    unlink("/tmp/.X11-unix/X0");

    printf("[runit] starting dbus\n");
    mkdir("/run/dbus", 0755);
    spawn("dbus-daemon", (char *[]){ "dbus-daemon", "--system", "--fork",
        "--print-address", NULL });
    sleep(1);
    spawn("dbus-daemon", (char *[]){ "dbus-daemon", "--session", "--fork",
        "--print-address", NULL });
    sleep(1);

    printf("[runit] starting Xorg (fbdev)\n");
    // Xorg output goes to the serial console (console=ttyS0) for
    // headless bring-up diagnosis; the log file copy stays as well.
    int xlog = open("/tmp/Xorg.log", O_WRONLY | O_CREAT | O_TRUNC, 0644);
    pid_t xpid = fork();
    if (xpid == 0) {
        if (xlog >= 0) { dup2(xlog, STDOUT_FILENO); dup2(xlog, STDERR_FILENO); }
        int con = open("/dev/console", O_WRONLY);
        if (con >= 0) { dup2(con, STDOUT_FILENO); dup2(con, STDERR_FILENO); }
        int devnull = open("/dev/null", O_RDONLY);
        if (devnull >= 0) { dup2(devnull, STDIN_FILENO); close(devnull); }
        setsid();
        // /usr/bin/Xorg is a shell wrapper and the kernel does not
        // interpret shebangs — exec the real server binary directly.
        execl("/usr/lib/xorg/Xorg", "Xorg", ":0",
              "-config", "/etc/X11/xorg-fbdev.conf",
              "-noreset", "-verbose", "3", (char *)NULL);
        _exit(127);
    }
    sleep(8);

    printf("[runit] starting gnome-shell (minimal gs path)\n");
    setenv("DISPLAY", ":0", 1);
    char addr[512];
    read_first_line("/tmp/dbus-session.addr", addr, sizeof addr);
    // dbus-daemon --print-address prints "unix:abstract=..." (no pid= form
    // with --fork on this build); pass through verbatim when present.
    if (addr[0]) setenv("DBUS_SESSION_BUS_ADDRESS", addr, 1);

    int gslog = open("/tmp/gnome-shell.log", O_WRONLY | O_CREAT | O_TRUNC, 0644);
    pid_t gpid = fork();
    if (gpid == 0) {
        if (gslog >= 0) { dup2(gslog, STDOUT_FILENO); dup2(gslog, STDERR_FILENO); }
        int devnull = open("/dev/null", O_RDONLY);
        if (devnull >= 0) { dup2(devnull, STDIN_FILENO); close(devnull); }
        setsid();
        execl("/usr/bin/gnome-shell", "gnome-shell", "--replace", (char *)NULL);
        _exit(127);
    }

    printf("[runit] session launched (xorg/shell pids %d %d)\n",
           (int)xpid, (int)gpid);

    // PID 1 never exits: a headless serial backend presents EOF, which
    // would make a console shell exit and power the machine down. Park
    // forever reaping children.
    signal(SIGCHLD, SIG_DFL);
    for (;;) {
        int st;
        pid_t p = wait(&st);
        if (p > 0) printf("[runit] child %d exited st=%d\n", (int)p, st);
        else sleep(2);
    }
    return 0;
}
