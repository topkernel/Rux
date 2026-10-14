/*
 * glibprobe.c — exercise the REAL libglib (dlopen'd from the image) on the
 * exact call sequence gnome-session-binary uses for autostart discovery:
 *
 *   g_dir_open(dir, 0, &err)                    gsm-util / g_dir_open
 *   g_dir_read_name(dir)                        entry loop
 *   g_key_file_new() + g_key_file_load_from_file(path, G_KEY_FILE_NONE, &err)
 *   g_key_file_has_key("Desktop Entry", "Exec") the GsmAutostartApp filter
 *   g_key_file_get_string("X-GNOME-Autostart-Phase")
 *
 * Build (dynamic, runs on host under qemu-riscv64 -L <img rootfs>, and
 * in the Rux guest):
 *   riscv64-linux-gnu-gcc -O1 -o glibprobe glibprobe.c -ldl
 *
 * Every glib failure prints its domain/message — that tells us exactly
 * why gnome-session rejects every .desktop file.
 */
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <dlfcn.h>
#include <errno.h>

/* --- minimal glib API surface (ABI-stable for decades) --- */
typedef void *GDir;
typedef void *GKeyFile;
typedef struct { void *dummy[2]; } *GErrorPtr;   /* GError* is a pointer */
typedef struct { unsigned long dummy[4]; } GErrorContents;

typedef GDir *(*g_dir_open_fn)(const char *path, unsigned flags, void **err);
typedef const char *(*g_dir_read_name_fn)(GDir *dir);
typedef void (*g_dir_close_fn)(GDir *dir);
typedef GKeyFile *(*g_key_file_new_fn)(void);
typedef void (*g_key_file_free_fn)(GKeyFile *kf);
typedef int (*g_key_file_load_from_file_fn)(GKeyFile *kf, const char *path,
                                            unsigned flags, void **err);
typedef int (*g_key_file_has_key_fn)(GKeyFile *kf, const char *group,
                                     const char *key, void **err);
typedef char *(*g_key_file_get_string_fn)(GKeyFile *kf, const char *group,
                                          const char *key, void **err);
typedef void (*g_error_free_fn)(void *err);
typedef const char *(*g_strerror_fn)(int errnum);

#define KF_NONE 0
#define ENTRY "Desktop Entry"

static g_dir_open_fn g_dir_open;
static g_dir_read_name_fn g_dir_read_name;
static g_dir_close_fn g_dir_close;
static g_key_file_new_fn g_key_file_new;
static g_key_file_free_fn g_key_file_free;
static g_key_file_load_from_file_fn g_key_file_load_from_file;
static g_key_file_has_key_fn g_key_file_has_key;
static g_key_file_get_string_fn g_key_file_get_string;
static g_error_free_fn g_error_free;

static void dump_err(const char *what, const char *name, void *err)
{
    /* GError layout: domain(quark=ulong), code(int), message(char*) on LP64 */
    if (!err) {
        printf("GLIB: %s %s failed err=NULL\n", what, name);
        return;
    }
    unsigned long *q = (unsigned long *)err;
    int code = *(int *)(q + 1);
    char **msgp = (char **)((char *)err + sizeof(unsigned long) + sizeof(int)
                            + 4 /* padding */);
    printf("GLIB: %s %s FAILED domain=%lu code=%d msg=%s\n",
           what, name, q[0], code, msgp && *msgp ? *msgp : "(?)");
    g_error_free(err);
}

int main(int argc, char **argv)
{
    const char *dir = (argc > 1) ? argv[1] : "/etc/xdg/autostart";
    void *h, *h2;
    GDir *d;
    const char *nm;
    int n_ok = 0, n_fail = 0, n_total = 0;

    setvbuf(stdout, NULL, _IOLBF, 0);
    printf("GLIB: BEGIN dir=%s\n", dir);

    h = dlopen("libglib-2.0.so.0", RTLD_NOW);
    if (!h) {
        printf("GLIB: dlopen libglib-2.0.so.0 FAILED: %s\n", dlerror());
        return 1;
    }
    g_dir_open = (g_dir_open_fn)dlsym(h, "g_dir_open");
    g_dir_read_name = (g_dir_read_name_fn)dlsym(h, "g_dir_read_name");
    g_dir_close = (g_dir_close_fn)dlsym(h, "g_dir_close");
    g_key_file_new = (g_key_file_new_fn)dlsym(h, "g_key_file_new");
    g_key_file_free = (g_key_file_free_fn)dlsym(h, "g_key_file_free");
    g_key_file_load_from_file =
        (g_key_file_load_from_file_fn)dlsym(h, "g_key_file_load_from_file");
    g_key_file_has_key = (g_key_file_has_key_fn)dlsym(h, "g_key_file_has_key");
    g_key_file_get_string =
        (g_key_file_get_string_fn)dlsym(h, "g_key_file_get_string");
    g_error_free = (g_error_free_fn)dlsym(h, "g_error_free");
    if (!g_dir_open || !g_dir_read_name || !g_key_file_new ||
        !g_key_file_load_from_file || !g_key_file_has_key || !g_error_free) {
        printf("GLIB: dlsym incomplete\n");
        return 1;
    }
    printf("GLIB: symbols resolved\n");

    d = g_dir_open(dir, 0, NULL);
    if (!d) {
        printf("GLIB: g_dir_open(%s) returned NULL (errno=%d %s)\n",
               dir, errno, strerror(errno));
        return 1;
    }
    printf("GLIB: g_dir_open OK\n");

    while ((nm = g_dir_read_name(d)) != NULL) {
        size_t l = strlen(nm);
        char path[1024];
        GKeyFile *kf;
        void *err = NULL;
        int rc;

        if (l <= 8 || strcmp(nm + l - 8, ".desktop"))
            continue;
        n_total++;
        snprintf(path, sizeof(path), "%s/%s", dir, nm);

        kf = g_key_file_new();
        rc = g_key_file_load_from_file(kf, path, KF_NONE, &err);
        if (!rc) {
            dump_err("LOAD", nm, err);
            g_key_file_free(kf);
            n_fail++;
            continue;
        }
        err = NULL;
        if (!g_key_file_has_key(kf, ENTRY, "Exec", &err)) {
            if (err) dump_err("HASKEY-EXEC", nm, err);
            else printf("GLIB: %s has no Exec key\n", nm);
            g_key_file_free(kf);
            n_fail++;
            continue;
        }
        {
            char *phase = g_key_file_get_string(kf, ENTRY,
                                                "X-GNOME-Autostart-Phase", NULL);
            char *prov = g_key_file_get_string(kf, ENTRY,
                                               "X-GNOME-Autostart-ProvidedBy", NULL);
            printf("GLIB: OK %s phase=%s providedBy=%s\n", nm,
                   phase ? phase : "-", prov ? prov : "-");
            free(phase); free(prov);
        }
        g_key_file_free(kf);
        n_ok++;
    }
    g_dir_close(d);
    printf("GLIB: END total=%d ok=%d fail=%d\n", n_total, n_ok, n_fail);
    return (n_ok == n_total && n_total > 0) ? 0 : 2;
}
