/*
 * Test: getuid() / getgid()
 * Test user/group ID retrieval
 */

#include <unistd.h>

int main(void)
{
    uid_t uid, euid;
    gid_t gid, egid;

    uid = getuid();
    euid = geteuid();
    gid = getgid();
    egid = getegid();

    /* In a simple system, these should be 0 (root) or valid values */
    if (uid == (uid_t)-1) return 1;
    if (euid == (uid_t)-1) return 2;
    if (gid == (gid_t)-1) return 3;
    if (egid == (gid_t)-1) return 4;

    return 0;
}
