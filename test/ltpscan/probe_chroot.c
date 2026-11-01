/* chroot04 replica: root mkdirs, seteuid(nobody), chroot(dir) expect EACCES */
#include <errno.h>
#include <pwd.h>
#include <stdio.h>
#include <sys/stat.h>
#include <unistd.h>

int main(void)
{
    mkdir("/tmp/c4dir", 0222);
    chmod("/tmp/c4dir", 0222);
    struct passwd *pw = getpwnam("nobody");
    if (!pw) { printf("no nobody\n"); return 1; }
    if (seteuid(pw->pw_uid)) { printf("seteuid ERR %d\n", errno); return 1; }
    printf("euid=%d\n", geteuid());
    long r = chroot("/tmp/c4dir");
    printf("chroot absolute 0222 dir -> %ld errno=%d (want -1 EACCES)\n", r, errno);
    errno = 0;
    r = chroot("/tmp/c4dir/nope");
    printf("chroot missing -> %ld errno=%d (want -1 ENOENT)\n", r, errno);
    /* relative from a 0700 nobody-owned dir */
    if (mkdir("/tmp/c4root", 0700)) printf("mkdir c4root ERR %d\n", errno);
    if (seteuid(0)) printf("seteuid0 ERR %d\n", errno);
    if (mkdir("/tmp/c4root/inner", 0222)) printf("mkdir inner ERR %d\n", errno);
    if (chdir("/tmp/c4root")) printf("chdir ERR %d\n", errno);
    if (seteuid(pw->pw_uid)) printf("seteuid2 ERR %d\n", errno);
    errno = 0;
    r = chroot("inner");
    printf("chroot relative inner -> %ld errno=%d (want -1 EACCES)\n", r, errno);
    return 0;
}
