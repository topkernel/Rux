/* m_gtk_nodisp.c — GTK3 headless probe: call gtk_init_check with NO
 * DISPLAY in the environment. Contract: it must return FALSE (and print
 * "cannot open display"), never crash. Reference behaviour verified on
 * Linux riscv64 via qemu-user.
 *
 * Build (cross, against the glib-gap dev sysroot):
 *   riscv64-linux-gnu-gcc -O1 -g [gtk-3.0 et al. include dirs] \
 *     m_gtk_nodisp.c -lgtk-3 -lgdk-3 -lgdk_pixbuf-2.0 -lpangocairo-1.0 \
 *     -lpango-1.0 -lgio-2.0 -lgobject-2.0 -lglib-2.0 -lcairo -lharfbuzz \
 *     -o m_gtk_nodisp
 * Linux reference: qemu-riscv64 -L <riscv64 rootfs> ./m_gtk_nodisp
 *
 * Closure record (2026-09-30, kernel ebd32be): the historically reported
 * "GTK3 gtk_init_check crashes without DISPLAY" does NOT reproduce —
 * 4 boots on Rux match the Linux reference exactly (FALSE, exit 0, no
 * signal) for both the no-DISPLAY and DISPLAY=:37-no-server variants.
 * Archived as fixed-by-the-mm-era-changes (see 51c01da) / no-bug.
 */
#include <stdio.h>
#include <stdlib.h>
#include <sys/wait.h>
#include <unistd.h>
#include <gtk/gtk.h>

int main(int argc, char **argv) {
    setvbuf(stdout, NULL, _IOLBF, 0);
    (void)argc; (void)argv;
    alarm(60);
    unsetenv("DISPLAY");
    unsetenv("WAYLAND_DISPLAY");
    /* WSLg hosts a real wayland-0 socket under XDG_RUNTIME_DIR — hide it
     * so the probe measures the genuinely headless path on both Linux
     * (qemu-user) and Rux. */
    unsetenv("XDG_RUNTIME_DIR");

    printf("gtk %u.%u.%u\n",
           gtk_get_major_version(), gtk_get_minor_version(),
           gtk_get_micro_version());

    printf("calling gtk_init_check(NULL, NULL) with no DISPLAY\n");
    fflush(stdout);

    gboolean ok = gtk_init_check(NULL, NULL);

    printf("gtk_init_check returned %s\n", ok ? "TRUE" : "FALSE");
    if (ok) {
        GdkDisplay *d = gdk_display_get_default();
        printf("default display: %s\n", d ? gdk_display_get_name(d) : "(none)");
    }
    fflush(stdout);

    if (!ok) {
        printf("RESULT m_gtk_nodisp PASS\n");
    } else {
        printf("RESULT m_gtk_nodisp FAIL (display opened without DISPLAY?)\n");
        return 1;
    }

    /* Variant 2: DISPLAY set to an address with nothing listening —
     * must also return FALSE, not crash. Fresh GTK state via fork().
     * Use display :37 — the WSLg host really runs :0, so :0 would open
     * the host X server and legitimately return TRUE. */
    fflush(stdout);
    pid_t p = fork();
    if (p == 0) {
        setenv("DISPLAY", ":37", 1);
        unsetenv("WAYLAND_DISPLAY");
        unsetenv("XDG_RUNTIME_DIR");
        printf("v2: calling gtk_init_check with DISPLAY=:37 (no server)\n");
        fflush(stdout);
        gboolean ok2 = gtk_init_check(NULL, NULL);
        printf("v2 gtk_init_check returned %s\n", ok2 ? "TRUE" : "FALSE");
        fflush(stdout);
        _exit(ok2 ? 2 : 0);
    }
    int st = 0;
    if (waitpid(p, &st, 0) != p) {
        printf("v2 waitpid failed\n");
        return 4;
    }
    if (WIFSIGNALED(st)) {
        printf("v2 CRASHED sig=%d\n", WTERMSIG(st));
        return 3;
    }
    printf("v2 exited=%d\n", WEXITSTATUS(st));
    return WEXITSTATUS(st) == 0 ? 0 : 5;
}
