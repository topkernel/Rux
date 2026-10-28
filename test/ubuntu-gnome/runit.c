// runit.c — static session bootstrap for the Ubuntu riscv64 GNOME image.
//
// Port of test/gnome-x86/runit.c (the surviving mirror of the lost gp3i
// chain). Chain: system dbus, session dbus (address captured through a
// pipe so every later client shares ONE bus), Xorg on fbdev, then the
// gnome-session chain. riscv64 differences from the x86 version:
//   - /etc/runit-session selects the session name (default "gnome": the
//     full stock chain with gsd; "rux" pins org.gnome.Shell as the only
//     required component for a minimal bring-up).
//   - Compiled with the repo musl toolchain (same reason as udesk: the
//     host cross-gcc static glibc targets a newer baseline than QEMU
//     rv64). See test/ubuntu-gui/build-img.sh.
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
// ("unix:path=...") through a pipe, so the whole session shares one bus
// instead of every client dbus-launching its own.
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

// Run `file argv` with stdout/stderr on the console; wait up to
// `max_sec` seconds (SIGKILL a straggler) and return its exit status
// (-1 on timeout/fork failure). Used by the gdm branch for the
// synchronous udev coldplug steps.
static int run_console_wait(const char *file, const char *const argv[],
                            int max_sec) {
    pid_t pid = fork();
    if (pid < 0) return -1;
    if (pid == 0) {
        int con = open("/dev/console", O_WRONLY);
        if (con >= 0) { dup2(con, STDOUT_FILENO); dup2(con, STDERR_FILENO); }
        int devnull = open("/dev/null", O_RDONLY);
        if (devnull >= 0) { dup2(devnull, STDIN_FILENO); close(devnull); }
        execv(file, (char *const *)argv);
        _exit(127);
    }
    for (int i = 0; i < max_sec; i++) {
        int st;
        pid_t r = waitpid(pid, &st, WNOHANG);
        if (r == pid) return st;
        if (r < 0) return -1;
        sleep(1);
    }
    kill(pid, SIGKILL);
    waitpid(pid, NULL, 0);
    printf("[runit] %s timed out after %ds\n", file, max_sec);
    return -1;
}

// X-state probe: ground truth for the "windows never paint" hunt
// (leg-6). Clients like gnome-session-failed run for the whole session
// yet no window ever appears on screen — the shell's own chrome
// (panel/wallpaper/cursor) paints fine, so the open question is
// whether client windows ever MAP and get MANAGED before blaming the
// compositing (GLX texture-from-pixmap) path. xwininfo -tree -root
// lists every window with map state and geometry; xprop then shows
// which windows the WM reports as its clients (_NET_CLIENT_LIST).
// x11-xserver-utils ships both. Console-only, ~30 lines when idle.
static void dump_xstate(void) {
    static const char *const probes[][4] = {
        { "== dump X tree (xwininfo -root -tree) ==",
          "/usr/bin/xwininfo", "-root", "-tree" },
        { "== dump X clients (xprop _NET_CLIENT_LIST) ==",
          "/usr/bin/xprop", "-root", "_NET_CLIENT_LIST _NET_SUPPORTING_WM_CHECK" },
        // GLX capabilities as seen by a CLIENT on this stack (mesa-utils
        // joined the image in r8): server glX extensions string incl.
        // GLX_EXT_texture_from_pixmap — the capability mutter's X11
        // compositing needs. Windows map+frame but never paint; if TFP
        // is missing from this string the silent-drop hypothesis is
        // settled image-side, if present the hunt moves to
        // glXCreatePixmap texturing (leg-7).
        { "== dump GLX caps (glxinfo -B) ==",
          "/usr/bin/glxinfo", "-B", "" },
    };
    for (unsigned i = 0; i < sizeof probes / sizeof probes[0]; i++) {
        printf("%s\n", probes[i][0]);
        fflush(stdout);
        pid_t d = fork();
        if (d == 0) {
            int con = open("/dev/console", O_WRONLY);
            if (con >= 0) { dup2(con, STDOUT_FILENO); dup2(con, STDERR_FILENO); }
            setenv("DISPLAY", ":0", 1);
            if (probes[i][3][0])
                execl(probes[i][1], "probe", probes[i][2], probes[i][3],
                      (char *)NULL);
            else
                execl(probes[i][1], "probe", probes[i][2], (char *)NULL);
            _exit(127);
        }
        int st; waitpid(d, &st, 0);
        if (st != 0)
            printf("[runit] xprobe %s exited st=%d\n", probes[i][1], st);
    }
    // Server GLX extension string (leg-8): glxinfo -B prints only the
    // brief block; the full run's "server glx extensions" list is where
    // GLX_EXT_texture_from_pixmap (mutter's X11 composite texture path)
    // appears or does not. sh -c so the pipe lives guest-side.
    {
        printf("== dump GLX server extensions (TFP check) ==\n");
        fflush(stdout);
        pid_t d = fork();
        if (d == 0) {
            int con = open("/dev/console", O_WRONLY);
            if (con >= 0) { dup2(con, STDOUT_FILENO); dup2(con, STDERR_FILENO); }
            setenv("DISPLAY", ":0", 1);
            execl("/bin/sh", "sh", "-c",
                  "/usr/bin/glxinfo 2>/dev/null | "
                  "sed -n '/server glx extensions/,+8p'; "
                  "/usr/bin/glxinfo 2>/dev/null | grep -c texture_from_pixmap "
                  "| sed 's/^/tfp-lines=/'; "
                  "/usr/bin/glxinfo 2>/dev/null | grep -m2 'GLX visual\\|GLX fbconfig'",
                  (char *)NULL);
            _exit(127);
        }
        int st; waitpid(d, &st, 0);
    }
    // Per-window content walk (leg-8): XGetImage every sizable window's
    // own content. An xterm body with white-with-glyph pixels in its own
    // window (absent from scanout) convicts the mutter composite path;
    // an all-black xterm window convicts client drawing. Replaces the
    // hard-coded rects as the primary frontier instrument (the two rect
    // probes stay for root-vs-scanout continuity).
    {
        printf("== dump XWALK (per-window content) ==\n");
        fflush(stdout);
        pid_t d = fork();
        if (d == 0) {
            int con = open("/dev/console", O_WRONLY);
            if (con >= 0) { dup2(con, STDOUT_FILENO); dup2(con, STDERR_FILENO); }
            setenv("DISPLAY", ":0", 1);
            execl("/usr/local/bin/xregion", "xregion", "-w", "100",
                  (char *)NULL);
            _exit(127);
        }
        int st; waitpid(d, &st, 0);
        if (st != 0)
            printf("[runit] xregion -w exited st=%d\n", st);
    }
    // Composited-content probe (leg-7): XGetImage the root window where
    // the xterm probe window should be (its geometry has been 904x536 at
    // +140+90 or +140+127 — sample the interior so either placement is
    // covered), plus the panel strip. If XREGION shows the window body
    // in the root but the QMP screendump of the same area does not, the
    // break is Xorg->scanout; if the root lacks it too, mutter never
    // composited the client (GLX texture-from-pixmap hunt).
    {
        static const char *const rects[][5] = {
            { "window-body", "200", "200", "700", "300" },
            { "panel-strip", "0", "0", "1280", "30" },
        };
        for (unsigned i = 0; i < sizeof rects / sizeof rects[0]; i++) {
            printf("== dump XREGION %s ==\n", rects[i][0]);
            fflush(stdout);
            pid_t d = fork();
            if (d == 0) {
                int con = open("/dev/console", O_WRONLY);
                if (con >= 0) { dup2(con, STDOUT_FILENO); dup2(con, STDERR_FILENO); }
                setenv("DISPLAY", ":0", 1);
                execl("/usr/local/bin/xregion", "xregion",
                      rects[i][1], rects[i][2], rects[i][3], rects[i][4],
                      (char *)NULL);
                _exit(127);
            }
            int st; waitpid(d, &st, 0);
            if (st != 0)
                printf("[runit] xregion %s exited st=%d\n", rects[i][0], st);
        }
    }
}

// --- kernel trace forensics (merged-main tracing subsystem) ---
// /sys/kernel/debug/tracing is auto-mounted by the kernel. Arm the
// sched + page-fault events before the session chain starts so any
// stall or crash has a scheduling/fault trail. Knob runit-trace=0 on
// the kernel cmdline disables (default on).
#define TRACE_BASE "/sys/kernel/debug/tracing"

static void write_file_text(const char *path, const char *text) {
    int fd = open(path, O_WRONLY);
    if (fd < 0) return;
    write(fd, text, strlen(text));
    close(fd);
}

static void arm_tracing(int on) {
    if (!on) return;
    // Focused set: scheduling + page faults only. Arming every event
    // (the old events/enable=1) included sys_enter/sys_exit, which
    // floods the per-CPU rings on every syscall — under the gsd storm
    // the buffer held only ~160ms of history (1.5M+ overwrites/10min),
    // far too little to see a stall. sched+exceptions keeps enough
    // history to name where the chain stalls.
    write_file_text(TRACE_BASE "/events/sched/enable", "1");
    write_file_text(TRACE_BASE "/events/exceptions/enable", "1");
    write_file_text(TRACE_BASE "/tracing_on", "1");
}

// Tail of the buffered trace records + per-CPU stats, to the console.
// Called when a session child dies (and periodically by the tailer) so
// the serial log carries the last scheduled events before the death.
static void dump_trace(int lines) {
    struct stat st;
    if (stat(TRACE_BASE "/trace", &st) != 0) return;
    printf("== trace tail (last events before death) ==\n");
    fflush(stdout);
    dump_log_prefix(TRACE_BASE "/trace", lines);
    dump_log_prefix(TRACE_BASE "/per_cpu/cpu0/stats", 4);
}

// Read a one-line knob file (/etc/runit-mode, /etc/runit-session).
// Falls back to the kernel cmdline (/proc/cmdline) knob of the same
// name: "runit-mode=gs runit-session=rux" on the qemu -append line
// overrides the image-baked defaults, so fire.py can flip modes per
// boot without an image rebuild (boots use -snapshot).
static void read_knob(const char *path, char *buf, size_t cap) {
    buf[0] = 0;
    const char *eq = strrchr(path, '/');
    const char *key = eq ? eq + 1 : path;
    char cmb[2048] = "";
    int cf = open("/proc/cmdline", O_RDONLY);
    if (cf >= 0) {
        ssize_t n = read(cf, cmb, sizeof cmb - 1);
        close(cf);
        if (n > 0) cmb[n] = 0;
    }
    char pat[64];
    snprintf(pat, sizeof pat, "%s=", key);
    char *cm = cmb[0] ? strstr(cmb, pat) : NULL;
    if (cm) {
        cm += strlen(pat);
        size_t i = 0;
        while (cm[i] && cm[i] != ' ' && cm[i] != '\n' && i < cap - 1) {
            buf[i] = cm[i];
            i++;
        }
        buf[i] = 0;
        return; // cmdline knob wins over the image file
    }
    int mf = open(path, O_RDONLY);
    if (mf >= 0) {
        ssize_t n = read(mf, buf, cap - 1);
        close(mf);
        if (n > 0) {
            buf[n] = 0;
            for (size_t i = 0; buf[i]; i++)
                if (buf[i] == '\n' || buf[i] == '\r') buf[i] = 0;
        }
    }
}

// --- notification-banner probe (leg-8) ---
// The shell's org.freedesktop.Notifications daemon registers at the END
// of the JS init walk ("GNOME Shell started"), so a banner can only be
// tested once that line appears. Watch /tmp/gnome-shell.log for the
// marker; on first sight (then every 200s) fire a Notify call with a
// long expire-time and print NOTIFY-SENT — fire.py sees the marker and
// takes a burst of screendumps timed to the banner lifetime, so the
// banner check does not depend on the 30s idle shot cadence.
static int shell_started(void)
{
    // Cheap tail scan: the marker is printed by the shell itself and
    // also mirrored by the periodic log tailer, so the last 8 KiB is
    // enough.
    int fd = open("/tmp/gnome-shell.log", O_RDONLY);
    if (fd < 0) return 0;
    char buf[8192];
    ssize_t n = read(fd, buf, sizeof buf - 1);
    close(fd);
    if (n <= 0) return 0;
    buf[n] = 0;
    return strstr(buf, "GNOME Shell started") != NULL;
}

static void fire_notify_banner(void)
{
    pid_t d = fork();
    if (d == 0) {
        int con = open("/dev/console", O_WRONLY);
        if (con >= 0) { dup2(con, STDOUT_FILENO); dup2(con, STDERR_FILENO); }
        execl("/usr/bin/gdbus", "gdbus", "call", "--session",
              "--dest", "org.freedesktop.Notifications",
              "--object-path", "/org/freedesktop/Notifications",
              "--method", "org.freedesktop.Notifications.Notify",
              "rux-probe", "0", "", "Rux banner probe",
              "banner body alpha 24680", "[]", "{}", "45000",
              (char *)NULL);
        _exit(127);
    }
    int st; waitpid(d, &st, 0);
    printf("[runit] NOTIFY-SENT gdbus st=%d\n", st);
    fflush(stdout);
}

int main(void) {
    // NOTE: SIGCHLD must stay SIG_DFL — SIG_IGN is inherited across
    // fork AND exec, and it makes gnome-session/glib waitpid() calls
    // return ECHILD (auto-reap), so every spawned component looks like
    // an instant failure.

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

    char modebuf[16] = "";
    read_knob("/etc/runit-mode", modebuf, sizeof modebuf);
    // "gs" = unsupervised gnome-shell --replace (TCG-friendly: no 20s
    // register deadline). "gdm" = stock display-manager chain (logind +
    // gdm3; see the gdm branch below). Default "session" = full
    // gnome-session chain. Exact match only — "g" alone is NOT gs.
    const char *MODE = "session";
    if (strcmp(modebuf, "gs") == 0) MODE = "gs";
    else if (strcmp(modebuf, "gdm") == 0) MODE = "gdm";
    printf("[runit] mode=%s\n", MODE);

    char tracebuf[8] = "";
    read_knob("/etc/runit-trace", tracebuf, sizeof tracebuf);
    int TRACE = (tracebuf[0] == '0') ? 0 : 1;
    arm_tracing(TRACE);
    printf("[runit] trace armed=%d\n", TRACE);

    char sessbuf[32] = "";
    read_knob("/etc/runit-session", sessbuf, sizeof sessbuf);
    const char *SESSION = sessbuf[0] ? sessbuf : "gnome";
    printf("[runit] session=%s\n", SESSION);

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
    // NO_AT_BRIDGE: every GTK app and gsd plugin retries the missing
    // org.a11y.Bus for the whole session ("AT-SPI: Error retrieving
    // accessibility bus address" — dozens of dbind warnings per boot,
    // each potentially costing a serialized dbus round trip in the
    // 25s-timeout shell init walk). GTK respects this env by skipping
    // the a11y bridge entirely. Cheaper than shipping at-spi correctly
    // and removes a whole timeout class from the init walk (leg-7).
    setenv("NO_AT_BRIDGE", "1", 1);
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

    // upowerd from PID1 instead of dbus activation: gsd-power exits(1)
    // and gnome-session respawn-loops it when org.freedesktop.UPower is
    // missing. The daemon IS activatable via the (now-fixed) setuid
    // helper, but a cold upowerd start takes ~20s even on a QUIET boot
    // (measured, run/upowerd-probe) — dbus's 25s service_start_timeout
    // loses that race under the session storm and activation times out.
    // Starting it here removes the race entirely.
    {
        pid_t u = launch_logged("/tmp/upowerd.log", "/usr/libexec/upowerd",
                                "upowerd",
                                (const char *const []){ "upowerd", NULL });
        (void)u;
        printf("[runit] upowerd started (pid %d)\n", (int)u);
    }

    // accounts-daemon from PID1, same reasoning as upowerd (leg-7): the
    // shell's ActUserManager and gsd-xsettings talk to
    // org.freedesktop.Accounts on the SYSTEM bus. With the service
    // absent every proxy creation runs the activator, times out or
    // errors ("ActUserManager: user (null) has no username", the
    // accountsservice-CRITICAL spam in every serial so far), and the
    // per-plugin waits serialize into the shell init walk. The daemon
    // is small and idles quietly once up. jammy ships it under
    // /usr/libexec; the legacy /usr/lib/accountsservice path is the
    // fallback.
    {
        const char *acct = "/usr/libexec/accounts-daemon";
        if (access(acct, X_OK) != 0)
            acct = "/usr/lib/accountsservice/accounts-daemon";
        pid_t a = launch_logged("/tmp/accounts-daemon.log", acct,
                                "accounts-daemon",
                                (const char *const []){ "accounts-daemon",
                                                        NULL });
        (void)a;
        printf("[runit] accounts-daemon started (pid %d, %s)\n",
               (int)a, acct);
    }

    printf("[runit] starting Xorg (fbdev)\n");
    // gdk-pixbuf loaders cache: dpkg -x skips the postinst that writes
    // it, and without the cache every pixbuf load beyond the built-ins
    // fails ("Could not load a pixbuf from icon theme" — the shell's
    // SVG icons need the librsvg loader registered). Generate it once,
    // guest-side, before anything paints.
    {
        pid_t q = fork();
        if (q == 0) {
            const char *cachepath =
                "/usr/lib/riscv64-linux-gnu/gdk-pixbuf-2.0/2.10.0/loaders.cache";
            int out = open(cachepath, O_WRONLY | O_CREAT | O_TRUNC, 0644);
            if (out >= 0) { dup2(out, STDOUT_FILENO); close(out); }
            execl("/usr/lib/riscv64-linux-gnu/gdk-pixbuf-2.0/gdk-pixbuf-query-loaders",
                  "gdk-pixbuf-query-loaders", (char *)NULL);
            _exit(127);
        }
        if (q > 0) { int st; waitpid(q, &st, 0); }
    }
    // Xorg output goes to the serial console (console=ttyS0) for
    // headless bring-up diagnosis; the log file copy stays as well.
    // Xorg is runit's OWN bring-up server for the session/gs modes. In
    // gdm mode gdm3 owns the display stack completely (its greeter X
    // server, its vt/display bookkeeping) — a second Xorg from PID1
    // would squat :0 and gdm's display would collide with it (r4: both
    // ran; gdm3 died on an option anyway, but the design is single
    // owner). gdk-pixbuf cache + stale-lock cleanup stay: cheap and
    // idempotent, and the greeter wants the loaders cache too.
    pid_t xpid = -1;
    if (strcmp(MODE, "gdm") != 0) {
        pid_t xfork = fork();
        if (xfork == 0) {
            int xlog = open("/tmp/Xorg.log", O_WRONLY | O_CREAT | O_TRUNC, 0644);
            if (xlog >= 0) { dup2(xlog, STDOUT_FILENO); dup2(xlog, STDERR_FILENO); }
            int con = open("/dev/console", O_WRONLY);
            if (con >= 0) { dup2(con, STDOUT_FILENO); dup2(con, STDERR_FILENO); }
            int devnull = open("/dev/null", O_RDONLY);
            if (devnull >= 0) { dup2(devnull, STDIN_FILENO); close(devnull); }
            setsid();
            // The riscv64 kernel does interpret shebangs, but exec the real
            // server binary directly anyway — same as the proven x86 path.
            execl("/usr/lib/xorg/Xorg", "Xorg", ":0",
                  "-config", "/etc/X11/xorg-fbdev.conf",
                  "-noreset", "-verbose", "3", (char *)NULL);
            _exit(127);
        }
        xpid = xfork;
        wait_for_x(30);
        sleep(2); // let GLX initialize before the first client connects
    }

    printf("[runit] starting session chain (mode=%s session=%s)\n", MODE, SESSION);
    // Periodic log tailer: every 20s push the tail of the session logs
    // to the console, so bring-up forensics do not depend on death
    // detection working.
    {
        pid_t tp = fork();
        if (tp == 0) {
            int cyc = 0;
            int notified = 0;
            for (;;) {
                sleep(20);
                cyc++;
                // gdm-mode logs ride along (stat-guarded; missing files
                // are skipped) so the first display-manager boots leave
                // their forensics on the serial console too.
                const char *logs[7] = { "/tmp/gnome-session.log",
                                        "/tmp/gnome-shell.log",
                                        "/tmp/Xorg.log",
                                        "/tmp/logind.log",
                                        "/tmp/gdm3.log",
                                        // gdm's X server logs (root-written)
                                        "/var/log/Xorg.0.log",
                                        NULL };
                for (int i = 0; i < 6; i++) {
                    struct stat st;
                    if (stat(logs[i], &st) != 0) continue;
                    dump_log_prefix(logs[i], 150);
                }
                // notification-banner probe (leg-8): fire on the shell
                // marker, re-fire every 10 cycles so at least one lands
                // in a calm window (each call prints NOTIFY-SENT).
                if (shell_started()) {
                    if (!notified || (cyc % 10) == 0) {
                        notified = 1;
                        fire_notify_banner();
                    }
                }
                if (TRACE && (cyc % 3) == 0) {
                    dump_log_prefix(TRACE_BASE "/per_cpu/cpu0/stats", 4);
                    // Short scheduling snapshot every minute: names the
                    // tasks the CPUs are actually running (or failing to
                    // run) during long stalls like the slow Shell boot.
                    dump_log_prefix(TRACE_BASE "/trace", 24);
                }
                // X window-state snapshot every minute: window map/
                // manage ground truth (see dump_xstate). Once the
                // desktop is up this is ~20 lines; before X starts the
                // probes just fail fast on connect.
                if ((cyc % 3) == 0)
                    dump_xstate();
            }
            _exit(0);
        }
    }
    if (strcmp(MODE, "gdm") == 0) {
        // Real display-manager path (gdm bring-up, session 17): runit
        // starts ONLY the background daemons and hands the display to
        // gdm3, which owns Xorg, the greeter session (gnome-shell
        // --mode=gdm via gdm-session-worker), PAM authentication and
        // the logged-in user session. No manual Xorg, no manual session
        // bus, no session chain — everything past this point is stock.
        // - /run/systemd/system makes sd_booted() return true for gdm
        //   and libsystemd clients without systemd as PID1.
        // - logind runs STANDALONE from the systemd package: it owns
        //   org.freedesktop.login1 on the system bus, needs the cgroup2
        //   mount (already up) and /dev/tty0 (kernel VT layer — the
        //   bring-up question this mode maps).
        mkdir("/run/systemd", 0755);
        mkdir("/run/systemd/system", 0755);
        // logind does NOT take --debug (r2 lesson: "unrecognized
        // option" -> exit 1); debug level comes from this env var.
        // G_MESSAGES_DEBUG=all: gdm3 logs through GLib to stderr —
        // without it /tmp/gdm3.log stays empty (its syslog() calls go
        // nowhere; there is no /dev/log on this image).
        setenv("SYSTEMD_LOG_LEVEL", "debug", 1);
        setenv("G_MESSAGES_DEBUG", "all", 1);
        // udevd + coldplug (r5 lesson): gdm3 never launches the greeter
        // while seat0's CanGraphical is false, and logind's seat0 gets
        // graphics devices ONLY through udev database tags
        // (71-seat.rules: master-of-seat) — without udevd logind runs
        // fine but its seat stays graphics-less and gdm idles forever.
        // The kernel already broadcasts userspace-triggered uevents
        // (write "add" to any /sys/.../uevent — sysfs uevent attr), so
        // stock udevadm trigger coldplugs drm/input/graphics.
        {
            const char *const uargv[] = { "systemd-udevd", "--daemon", NULL };
            pid_t upid = launch_logged("/tmp/udevd.log",
                                       "/lib/systemd/systemd-udevd",
                                       "systemd-udevd", uargv);
            printf("[runit] systemd-udevd started (pid %d)\n", (int)upid);
            sleep(3);
            // Synchronous coldplug of the seat-relevant subsystems
            // (full-tree trigger is slow under TCG for zero seat value).
            const char *const trig[] = {
                "udevadm", "trigger", "--action=add",
                "--subsystem-match=drm",
                "--subsystem-match=input",
                "--subsystem-match=graphics", NULL };
            run_console_wait("/usr/bin/udevadm", trig, 120);
            const char *const settle[] = {
                "udevadm", "settle", "--timeout=15", NULL };
            run_console_wait("/usr/bin/udevadm", settle, 30);
        }
        {
            const char *logind = "/lib/systemd/systemd-logind";
            if (access(logind, X_OK) != 0)
                logind = "/usr/lib/systemd/systemd-logind";
            const char *const largv[] = { "systemd-logind", NULL };
            pid_t lpid = launch_logged("/tmp/logind.log", logind,
                                       "systemd-logind", largv);
            printf("[runit] logind started (pid %d, %s)\n", (int)lpid, logind);
            sleep(3); // own login1 on the system bus before gdm asks
        }
        {
            // No --no-daemon: gdm 42 rejects it ("Failed to parse
            // options", r4). It daemonizes; the detached daemon keeps
            // running like dbus-daemon --fork. Debug logging rides on
            // [debug] Enable in custom.conf (syslog-less image: GLib
            // warnings still reach stderr only pre-daemonize).
            const char *const gargv[] = { "gdm3", NULL };
            pid_t gpid2 = launch_logged("/tmp/gdm3.log", "/usr/sbin/gdm3",
                                        "gdm3", gargv);
            printf("[runit] gdm3 started (pid %d)\n", (int)gpid2);
            (void)gpid2;
        }
        for (;;) {
            int st;
            pid_t p = wait(&st);
            if (p > 0)
                printf("[runit] child %d exited st=%d\n", (int)p, st);
            else
                sleep(2);
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
                if (p == gpid) {
                    if (TRACE) dump_trace(40);
                    dump_log("/tmp/gnome-shell.log"); dump_log("/tmp/Xorg.log");
                }
            } else {
                sleep(2);
            }
        }
    }
    {
        char sessflag[64];
        snprintf(sessflag, sizeof sessflag, "--session=%s", SESSION);
        const char *const argv[] = { "gnome-session-binary", sessflag, NULL };
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
                if (TRACE) dump_trace(40);
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
