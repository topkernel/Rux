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
    rep("mkdir /a/mnt/DIRECT", mkdir("/a/mnt/DIRECT", 0755));
    rep("lstat /DIRECT (created through bind?)", exists("/DIRECT"));
    rep("lstat /a/mnt/DIRECT (bind view)", exists("/a/mnt/DIRECT"));

    // Negative-dentry priming, then create the same name (OH stats the
    // sandbox path before MakeDirRecursive).
    rep("access /a/mnt/NEG100 F_OK (prime)", access("/a/mnt/NEG100", F_OK));
    rep("mkdir /a/mnt/NEG100", mkdir("/a/mnt/NEG100", 0711));
    rep("lstat /a/mnt/NEG100 after mkdir", exists("/a/mnt/NEG100"));
    rep("lstat /NEG100 (host side)", exists("/NEG100"));

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

    printf("BMD-DONE\n");
    return 0;
}
