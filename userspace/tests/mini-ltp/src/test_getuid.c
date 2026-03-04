/*
 * Test: getuid() / getgid()
 * 测试用户/组 ID 获取
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

    /* 在简单系统中，这些应该是 0 (root) 或有效值 */
    if (uid == (uid_t)-1) return 1;
    if (euid == (uid_t)-1) return 2;
    if (gid == (gid_t)-1) return 3;
    if (egid == (gid_t)-1) return 4;

    return 0;
}
