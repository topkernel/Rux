// bindmkdir.c — sc28 repro probe: mkdir through a bind of "/" fails ENOENT
// forever inside a chroot-prefix jail (OH appspawn sandbox prep:
// mkdir("/mnt/sandbox/100") -> -2 while mkdir("/mnt/sandbox") -> EEXIST).
//
// Shape under test (system-sandbox stand-in):
//   /a                = sandbox root (pivot target stand-in)
//   /a/mnt            = bind of the real root "/" (MS_BIND|MS_REC) — init's
//                       system-sandbox /mnt bind (source "/", recursive)
//   chroot("/a")      = the pivoted services' root (string-prefix jail)
// Then: mkdir("/mnt/<name>") inside the jail must cross the bind and create
// <name> in the real root; a pre-stat (negative dentry) must not poison it.
#define _GNU_SOURCE
#include <sys/mount.h>
#include <sys/stat.h>
#include <unistd.h>
#include <errno.h>
#include <stdio.h>
#include <string.h>
#include <fcntl.h>

static int step = 0;

static void rep(const char *what, long rc) {
    printf("BMD[%02d] %-52s rc=%-4ld errno=%d %s\n",
           ++step, what, rc, errno, rc == 0 ? "OK" : strerror(errno));
}

static int exists(const char *p) {
    struct stat st;
    return lstat(p, &st) == 0 ? 0 : -1;
}

int main(void) {
    setvbuf(stdout, NULL, _IONBF, 0);
    mkdir("/a", 0755);
    mkdir("/a/mnt", 0755);

    // The "/"-bind (recursive) — clones land under the fresh mount too,
    // including the fresh mount itself one level deeper (sc28 shape).
    rep("mount(\"/\",\"/a/mnt\",BIND|REC)",
        mount("/", "/a/mnt", NULL, MS_BIND | MS_REC, NULL));

    // Direct global-view mkdir through the bind.
    rep("stat /dev (global, post-rootbind)", exists("/dev"));
    rep("stat /proc (global, post-rootbind)", exists("/proc"));
    rep("mkdir /a/mnt/DIRECT", mkdir("/a/mnt/DIRECT", 0755));
    rep("lstat /DIRECT (created through bind?)", exists("/DIRECT"));
    rep("lstat /a/mnt/DIRECT (bind view)", exists("/a/mnt/DIRECT"));

    // Negative-dentry priming, then create the same name (OH stats the
    // sandbox path before MakeDirRecursive).
    rep("access /a/mnt/NEG100 F_OK (prime)", access("/a/mnt/NEG100", F_OK));
    rep("mkdir /a/mnt/NEG100", mkdir("/a/mnt/NEG100", 0711));
    rep("lstat /a/mnt/NEG100 after mkdir", exists("/a/mnt/NEG100"));
    rep("lstat /NEG100 (host side)", exists("/NEG100"));

    // App-sandbox idiom (sc33/sc34): self-bind the sandbox root, bind
    // /dev INSIDE (the target walk crosses the self-bind), jail, open
    // /dev/null. This is the appspawn child shape that dies at
    // appspawn_common.c:350 'open dev_null error: 2'.
    rep("mkdir /a2", mkdir("/a2", 0755));
    rep("self-bind /a2 -> /a2 (BIND|REC)",
        mount("/a2", "/a2", NULL, MS_BIND | MS_REC, NULL));
    rep("stat /dev (global, post-selfbind)", exists("/dev"));
    rep("stat /proc (global, post-selfbind)", exists("/proc"));
    rep("mkdir /a2/dev", mkdir("/a2/dev", 0755));
    rep("stat /dev (global, post-mkdir)", exists("/dev"));
    rep("bind /dev -> /a2/dev (BIND|REC)",
        mount("/dev", "/a2/dev", NULL, MS_BIND | MS_REC, NULL));
    rep("chroot /a2", chroot("/a2"));
    rep("chdir /", chdir("/"));
    {
        int fd = open("/dev/null", O_RDWR);
        rep("open /dev/null (app-sandbox jail)", fd >= 0 ? 0 : -1);
        if (fd >= 0) close(fd);
    }

    // The pivoted-services view: string-prefix jail.
    rep("chroot /a", chroot("/a"));
    rep("chdir /", chdir("/"));

    rep("mkdir /mnt/JAILDIR", mkdir("/mnt/JAILDIR", 0711));
    rep("lstat /mnt/JAILDIR (jail view)", exists("/mnt/JAILDIR"));

    rep("access /mnt/JAILNEG F_OK (prime)", access("/mnt/JAILNEG", F_OK));
    rep("mkdir /mnt/JAILNEG", mkdir("/mnt/JAILNEG", 0711));
    rep("lstat /mnt/JAILNEG after mkdir", exists("/mnt/JAILNEG"));
    rep("mkdir /mnt/JAILNEG again (want EEXIST)", mkdir("/mnt/JAILNEG", 0711));

    // Deeper jail path crossing the bind twice: /mnt/a/mnt/... (the
    // self-clone lives at /a/mnt/a/mnt in the bind view).
    rep("mkdir /mnt/a/mnt/DEEP", mkdir("/mnt/a/mnt/DEEP", 0711));
    rep("lstat /mnt/a/mnt/DEEP", exists("/mnt/a/mnt/DEEP"));


    // openmiss instrument check + dev/null through the bind
    {
        int fd = open("/dev/null", O_RDWR);
        rep("open /dev/null (jail, through bind)", fd >= 0 ? 0 : -1);
        if (fd >= 0) close(fd);
        fd = open("/mnt/definitely-missing-file", O_RDWR);
        rep("open missing (openmiss probe)", fd >= 0 ? 0 : -1);
        if (fd >= 0) close(fd);
    }

    printf("BMD-DONE\n");
    return 0;
}
