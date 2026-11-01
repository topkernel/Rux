/* setgid-create probe: rules from LTP creat08/creat09 (CVE-2018-13405). */
#include <fcntl.h>
#include <stdio.h>
#include <sys/stat.h>
#include <unistd.h>
#include <string.h>

static int show(const char *name)
{
    struct stat st;
    char path[256];
    snprintf(path, sizeof(path), "%s", name);
    if (stat(path, &st) < 0) { printf("stat %s: ERR\n", name); return 1; }
    printf("%s: mode=%o gid=%d\n", name, st.st_mode & 07777, st.st_gid);
    return 0;
}

int main(void)
{
    mkdir("/tmp/sgid_dir", 0777);
    chown("/tmp/sgid_dir", 65534, 3);
    chmod("/tmp/sgid_dir", 02777);
    show("/tmp/sgid_dir");

    /* root in setgid dir with explicit S_ISGID: must keep it (CAP_FSETID) */
    int fd = open("/tmp/sgid_dir/rootfile", O_CREAT | O_RDWR, 02777);
    if (fd < 0) printf("open rootfile ERR\n");
    else close(fd);
    show("/tmp/sgid_dir/rootfile");

    /* root in plain dir with explicit S_ISGID: keep (no strip without dir bit) */
    mkdir("/tmp/plain_dir", 0777);
    fd = open("/tmp/plain_dir/rootfile", O_CREAT | O_RDWR, 02777);
    if (fd < 0) printf("open plain rootfile ERR\n");
    else close(fd);
    show("/tmp/plain_dir/rootfile");

    /* nobody in setgid dir, not in group: strip S_ISGID (CVE-2018-13405) */
    if (setgid(65534) || setreuid(-1, 65534)) printf("setuid ERR\n");
    fd = open("/tmp/sgid_dir/nobodyfile", O_CREAT | O_RDWR, 02777);
    if (fd < 0) printf("open nobodyfile ERR\n");
    else close(fd);
    show("/tmp/sgid_dir/nobodyfile");

    /* setgid group-member file: nobody in its own group keeps S_ISGID */
    mkdir("/tmp/sgid_dir2", 0777);
    if (setuid(0)) printf("setuid0 back ERR (euid still nobody?)\n");
    chown("/tmp/sgid_dir2", 65534, 65534);
    chmod("/tmp/sgid_dir2", 02777);
    if (setreuid(-1, 65534)) printf("re-nobody ERR\n");
    fd = open("/tmp/sgid_dir2/memberfile", O_CREAT | O_RDWR, 02777);
    if (fd < 0) printf("open memberfile ERR\n");
    else close(fd);
    show("/tmp/sgid_dir2/memberfile");
    return 0;
}
