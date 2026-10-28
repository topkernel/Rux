// glnit.c — GL bring-up probe for the riscv64 GNOME image (runs as init).
//
// Root question for the "GLX: could not load software renderer" wall:
// WHERE inside the swrast driver init does the image's own Mesa fail?
// No dev headers needed: everything is dlopen/dlsym by name, shapes
// from mesa-23.2.1 dri_interface.h.
//
// Ladder (each rung loud on serial, /dev/console attached like runit):
//   S1  dlopen swrast_dri.so (RTLD_LAZY|RTLD_GLOBAL, like the Xserver)
//   S2  __driDriverGetExtensions_swrast -> walk pointer array, print
//       name/version pairs (correct layout this time)
//   S2b fork; child calls DRI_SWRast v4 createNewScreen2(0, loader_exts,
//       driver_exts, &configs, NULL) with a v1 __DRI_SWRastLoader; the
//       parent prints the child's wait status (clean NULL vs crash) —
//       this is the EXACT entry point the Xserver calls when it logs
//       "AIGLX error: Calling driver entry point failed".
//   S3  fork; child runs the EGL ladder (eglGetDisplay/Initialize on
//       default + surfaceless platform, forced software)
// Verdict line: GLNIT RESULT: ...
// All risky calls happen in forked children so a crash can never take
// PID 1 (and the machine) down.
#include <dlfcn.h>
#include <fcntl.h>
#include <stdint.h>
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>

/* ---- minimal DRI shapes (dri_interface.h, mesa 23.2) ---- */
struct dri_ext { const char *name; int version; };
struct dri_swrast_ext {           /* __DRI_SWRAST_VERSION 4 */
    struct dri_ext base;
    void *(*createNewScreen)(int, const struct dri_ext **,
                             const void ***, void *);
    void *(*createNewDrawable)(void *, const void *, void *);
    void *(*createNewContextForAPI)(void *, int, const void *, void *, void *);
    void *(*createContextAttribs)(void *, int, const void *, void *,
                                  unsigned, const uint32_t *, unsigned *, void *);
    void *(*createNewScreen2)(int, const struct dri_ext **,
                              const struct dri_ext **,
                              const void ***, void *);
};
/* loader side: __DRIswrastLoaderExtension v1 = base + 3 callbacks */
struct swrast_loader_v1 {
    struct dri_ext base;
    void (*getDrawableInfo)(void *, int *, int *, int *, int *, void *);
    void (*putImage)(void *, int, int, int, int, int, char *, void *);
    void (*getImage)(void *, int, int, int, int, char *, void *);
};

static void l_getDrawableInfo(void *d, int *x, int *y, int *w, int *h, void *p)
{ (void)d; (void)p; *x = *y = 0; *w = 64; *h = 64; }
static void l_putImage(void *d, int op, int x, int y, int w, int h, char *data, void *p)
{ (void)d; (void)op; (void)x; (void)y; (void)w; (void)h; (void)data; (void)p; }
static void l_getImage(void *d, int x, int y, int w, int h, char *data, void *p)
{ (void)d; (void)x; (void)y; (void)w; (void)h; (void)data; (void)p; }

static const struct swrast_loader_v1 my_swrast_loader = {
    { "DRI_SWRastLoader", 1 },
    l_getDrawableInfo, l_putImage, l_getImage,
};

static void attach_console(void)
{
    int con = open("/dev/console", O_WRONLY);
    if (con >= 0) {
        dup2(con, STDOUT_FILENO);
        dup2(con, STDERR_FILENO);
        close(con);
    }
    setvbuf(stdout, NULL, _IONBF, 0);
}

static void report_child(const char *what, int st)
{
    if (WIFEXITED(st))
        printf("[GLNIT] %s: child exited rc=%d\n", what, WEXITSTATUS(st));
    else if (WIFSIGNALED(st))
        printf("[GLNIT] %s: child killed by signal %d%s\n", what, WTERMSIG(st),
               WCOREDUMP(st) ? " (core dumped)" : "");
    else
        printf("[GLNIT] %s: child st=%08x\n", what, st);
}


static void dump_maps(void)
{
    /* cheap: fork `cat /proc/self/maps`-style via raw read */
    int fd = open("/proc/self/maps", O_RDONLY);
    if (fd < 0) { printf("[GLNIT] (no /proc/self/maps)\n"); return; }
    char buf[4096];
    ssize_t n = read(fd, buf, sizeof buf - 1);
    close(fd);
    if (n <= 0) { printf("[GLNIT] (maps read empty)\n"); return; }
    buf[n] = 0;
    /* print only lines mentioning swrast or total line count */
    int lines = 0;
    for (char *p = buf; *p; p++) if (*p == '\n') lines++;
    printf("[GLNIT] maps: %d bytes, %d lines (swrast lines):\n", (int)n, lines);
    char *save = NULL;
    for (char *line = strtok_r(buf, "\n", &save); line;
         line = strtok_r(NULL, "\n", &save))
        if (strstr(line, "swrast"))
            printf("[GLNIT]   %s\n", line);
}

static int run_ladder(void)
{
    printf("[GLNIT] === S1 dlopen swrast ===\n");
    const char *sw_paths[] = {
        "/usr/lib/riscv64-linux-gnu/dri/swrast_dri.so",
        "/usr/lib/x86_64-linux-gnu/dri/swrast_dri.so",
    };
    void *sw = NULL;
    for (unsigned i = 0; i < sizeof sw_paths / sizeof sw_paths[0]; i++) {
        sw = dlopen(sw_paths[i], RTLD_LAZY | RTLD_GLOBAL);
        if (sw) {
            printf("[GLNIT] dlopen %s OK\n", sw_paths[i]);
            break;
        }
        printf("[GLNIT] dlopen %s: %s\n", sw_paths[i], dlerror());
    }
    if (!sw) {
        printf("[GLNIT] dlopen swrast FAILED (all paths)\n");
        return 1;
    }

    printf("\n[GLNIT] === S2 walk extensions ===\n");
    printf("[GLNIT] rung: dlsym getExtensions\n");
    struct dri_ext **(*getext)(void) =
        (struct dri_ext **(*)(void))dlsym(sw, "__driDriverGetExtensions_swrast");
    if (!getext) {
        printf("[GLNIT] dlsym getExtensions FAILED: %s\n", dlerror());
        return 2;
    }
    printf("[GLNIT] rung: call getExtensions\n");
    struct dri_ext **exts = getext();
    if (!exts) {
        printf("[GLNIT] getExtensions returned NULL\n");
        return 2;
    }
    /* raw words first: if a later entry is a wild pointer we see the
     * pattern before anyone derefs it */
    printf("[GLNIT] raw array words: %016llx %016llx %016llx %016llx\n",
           (unsigned long long)(uintptr_t)exts[0], (unsigned long long)(uintptr_t)exts[1],
           (unsigned long long)(uintptr_t)exts[2], (unsigned long long)(uintptr_t)exts[3]);
    dump_maps();
    struct dri_swrast_ext *swrast = NULL;
    for (int i = 0; exts[i]; i++) {
        printf("[GLNIT] rung: deref ext[%d] @%p\n", i, (void *)exts[i]);
        const char *nm = exts[i]->name ? exts[i]->name : "(null-name)";
        int v = exts[i]->version;
        printf("[GLNIT]   ext[%d] %s v%d\n", i, nm, v);
        if (strcmp(nm, "DRI_SWRast") == 0 && v >= 3)
            swrast = (struct dri_swrast_ext *)exts[i];
    }
    if (!swrast) {
        printf("[GLNIT] no DRI_SWRast v>=3 extension found\n");
        return 2;
    }
    printf("[GLNIT] DRI_SWRast v%d, createNewScreen2=%p\n",
           swrast->base.version, (void *)swrast->createNewScreen2);

    printf("\n[GLNIT] === S2b createNewScreen2 ===\n");
    const struct dri_ext *loader_exts[] = {
        (const struct dri_ext *)&my_swrast_loader, NULL };
    const void **configs = NULL;
    printf("[GLNIT] rung: createNewScreen2\n");
    fflush(NULL);
    void *scr = swrast->createNewScreen2(0, loader_exts,
                                         (const struct dri_ext **)exts,
                                         &configs, NULL);
    if (scr) {
        printf("[GLNIT] createNewScreen2 OK: screen=%p configs=%p\n",
               scr, (void *)configs);
    } else {
        printf("[GLNIT] createNewScreen2 returned NULL (the Xserver failure)\n");
        return 3;
    }

    printf("\n[GLNIT] === S3 EGL ladder ===\n");
    printf("[GLNIT] rung: dlopen libEGL\n");
    void *egllib = dlopen("libEGL.so.1", RTLD_LAZY | RTLD_GLOBAL);
    if (!egllib) { printf("[GLNIT] dlopen libEGL failed: %s\n", dlerror()); return 4; }
    void *(*getDisplay)(void *) = (void *(*)(void *))dlsym(egllib, "eglGetDisplay");
    unsigned (*initialize)(void *, int *, int *) =
        (unsigned (*)(void *, int *, int *))dlsym(egllib, "eglInitialize");
    int (*getErr)(void) = (int (*)(void))dlsym(egllib, "eglGetError");
    const char *(*qString)(void *, unsigned) =
        (const char *(*)(void *, unsigned))dlsym(egllib, "eglQueryString");
    void *(*getPlatformDisplay)(unsigned, void *, const intptr_t *) =
        (void *(*)(unsigned, void *, const intptr_t *))dlsym(egllib, "eglGetPlatformDisplay");
    printf("[GLNIT] rung: eglGetDisplay(default)\n");
    void *dpy = getDisplay(NULL);
    printf("[GLNIT] eglGetDisplay(default)=%p\n", dpy);
    int ma = 0, mi = 0;
    if (dpy && initialize(dpy, &ma, &mi)) {
        printf("[GLNIT] eglInitialize(default) OK %d.%d vendor=%s\n",
               ma, mi, qString(dpy, 0x3053));
        return 0;
    }
    printf("[GLNIT] eglInitialize(default) FAILED err=0x%x\n", getErr());
    printf("[GLNIT] rung: eglGetPlatformDisplay(SURFACELESS)\n");
    dpy = getPlatformDisplay ? getPlatformDisplay(0x31DD, NULL, NULL) : NULL;
    printf("[GLNIT] eglGetPlatformDisplay(SURFACELESS)=%p\n", dpy);
    if (dpy && initialize(dpy, &ma, &mi)) {
        printf("[GLNIT] eglInitialize(surfaceless) OK %d.%d vendor=%s\n",
               ma, mi, qString(dpy, 0x3053));
        return 0;
    }
    printf("[GLNIT] eglInitialize(surfaceless) FAILED err=0x%x\n", getErr());
    return 5;
}

int main(void)
{
    attach_console();
    printf("[GLNIT] GL bring-up probe (pid %d)\n", (int)getpid());
    setenv("MESA_DEBUG", "1", 1);
    setenv("LIBGL_DEBUG", "verbose", 1);
    setenv("GALLIUM_DRIVER", "llvmpipe", 1);
    setenv("LIBGL_ALWAYS_SOFTWARE", "1", 1);

    /* The whole ladder runs in a forked child: a segfault anywhere
     * leaves PID 1 alive to report the wait status (the last printed
     * rung names the crash site). GLNIT_NOFORK=1 runs it inline for
     * gdb sessions (qemu-user's gdbstub cannot follow forked guests). */
    fflush(NULL);
    if (getenv("GLNIT_NOFORK")) {
        int rc = run_ladder();
        printf("\nGLNIT RESULT: ladder rc=%d (nofork)\n", rc);
        fflush(stdout);
        for (;;) pause();
    }
    pid_t c = fork();
    if (c == 0) {
        attach_console();
        int rc = run_ladder();
        printf("\nGLNIT RESULT: ladder rc=%d\n", rc);
        _exit(rc);
    }
    int st = 0;
    waitpid(c, &st, 0);
    report_child("ladder", st);
    printf("\nGLNIT RESULT: see rungs above (child st=%08x)\n", st);
    fflush(stdout);
    for (;;) pause();
    return 0;
}
