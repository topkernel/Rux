/* memfd_probe.c — the memfd_create(2) contract.
 *
 * The reference kernel (kernel/mm/memfd.c):
 *  - a NULL name faults in the strnlen_user() read and is answered
 *    EFAULT;
 *  - a name longer than MFD_NAME_MAX_LEN (NAME_MAX minus the "memfd:"
 *    prefix length) is EINVAL;
 *  - MFD_EXEC and MFD_NOEXEC_SEAL are mutually exclusive (EINVAL);
 *  - an unknown flag bit is EINVAL;
 *  - F_ADD_SEALS without MFD_ALLOW_SEALING is EPERM, and after
 *    F_SEAL_SEAL no further seal can be added (EPERM).
 *  - F_SEAL_WRITE blocks write(2) and ftruncate(2) with EPERM.
 */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/syscall.h>
#include <unistd.h>

#define MFD_CLOEXEC       0x0001U
#define MFD_ALLOW_SEALING 0x0002U
#define MFD_NOEXEC_SEAL   0x0008U
#define MFD_EXEC          0x0010U

#ifndef F_ADD_SEALS
#define F_ADD_SEALS 1033
#endif
#ifndef F_GET_SEALS
#define F_GET_SEALS 1034
#endif

#define F_SEAL_SEAL   0x0001
#define F_SEAL_WRITE  0x0008

static int failures;

static void expect_err(const char *what, long rc, int want_errno)
{
    if (rc >= 0) {
        printf("PROBE-FAIL %s: returned %ld, expected errno %d\n", what, rc, want_errno);
        failures++;
        if (rc >= 0)
            close((int)rc);
        return;
    }
    if (errno != want_errno) {
        printf("PROBE-FAIL %s: errno=%d, expected %d\n", what, errno, want_errno);
        failures++;
        return;
    }
    printf("PROBE-PASS %s: errno=%d\n", what, want_errno);
}

static void expect_ok(const char *what, long rc)
{
    if (rc < 0) {
        printf("PROBE-FAIL %s: errno=%d\n", what, errno);
        failures++;
        return;
    }
    printf("PROBE-PASS %s: fd=%ld\n", what, rc);
    close((int)rc);
}

int main(void)
{
    /* A NULL name is EFAULT (the reference reads the name with
     * strnlen_user, which faults on a NULL pointer). */
    errno = 0;
    expect_err("null name", memfd_create(NULL, 0), EFAULT);

    /* MFD_EXEC and MFD_NOEXEC_SEAL together are EINVAL. */
    errno = 0;
    expect_err("exec+noexec_seal",
               memfd_create("probe", MFD_EXEC | MFD_NOEXEC_SEAL), EINVAL);

    /* An unknown flag bit is EINVAL. */
    errno = 0;
    expect_err("unknown flag", memfd_create("probe", 0x100), EINVAL);

    /* The name limit: MFD_NAME_MAX_LEN bytes are accepted, one more is
     * EINVAL. */
    char name[300];
    memset(name, 'a', sizeof(name));
    name[249] = '\0';
    errno = 0;
    expect_ok("name of 249 bytes", memfd_create(name, 0));
    memset(name, 'a', sizeof(name));
    name[250] = '\0';
    errno = 0;
    expect_err("name of 250 bytes", memfd_create(name, 0), EINVAL);

    /* Sealing: F_ADD_SEALS needs MFD_ALLOW_SEALING. */
    long fd = memfd_create("probe", 0);
    if (fd < 0) {
        printf("PROBE-FAIL plain memfd: errno=%d\n", errno);
        failures++;
        return 1;
    }
    errno = 0;
    if (fcntl((int)fd, F_ADD_SEALS, F_SEAL_SEAL) == 0) {
        printf("PROBE-FAIL seal without MFD_ALLOW_SEALING: succeeded\n");
        failures++;
    } else if (errno != EPERM) {
        printf("PROBE-FAIL seal without MFD_ALLOW_SEALING: errno=%d, expected EPERM\n", errno);
        failures++;
    } else {
        printf("PROBE-PASS seal without MFD_ALLOW_SEALING: errno=EPERM\n");
    }
    close((int)fd);

    /* A sealable memfd: F_GET_SEALS is 0, F_SEAL_SEAL closes the set. */
    fd = memfd_create("probe", MFD_ALLOW_SEALING);
    if (fd < 0) {
        printf("PROBE-FAIL sealable memfd: errno=%d\n", errno);
        failures++;
        return 1;
    }
    if (fcntl((int)fd, F_GET_SEALS, 0) != 0) {
        printf("PROBE-FAIL F_GET_SEALS on a fresh memfd: not 0\n");
        failures++;
    } else {
        printf("PROBE-PASS F_GET_SEALS on a fresh memfd: 0\n");
    }
    if (fcntl((int)fd, F_ADD_SEALS, F_SEAL_SEAL) != 0) {
        printf("PROBE-FAIL F_ADD_SEALS(F_SEAL_SEAL): errno=%d\n", errno);
        failures++;
    } else {
        printf("PROBE-PASS F_ADD_SEALS(F_SEAL_SEAL)\n");
    }
    errno = 0;
    if (fcntl((int)fd, F_ADD_SEALS, F_SEAL_WRITE) == 0) {
        printf("PROBE-FAIL seal after F_SEAL_SEAL: succeeded\n");
        failures++;
    } else if (errno != EPERM) {
        printf("PROBE-FAIL seal after F_SEAL_SEAL: errno=%d, expected EPERM\n", errno);
        failures++;
    } else {
        printf("PROBE-PASS seal after F_SEAL_SEAL: errno=EPERM\n");
    }
    close((int)fd);

    /* F_SEAL_WRITE blocks write(2) and ftruncate(2). */
    fd = memfd_create("probe", MFD_ALLOW_SEALING);
    if (fd < 0) {
        printf("PROBE-FAIL sealable memfd 2: errno=%d\n", errno);
        failures++;
        return 1;
    }
    if (fcntl((int)fd, F_ADD_SEALS, F_SEAL_WRITE) != 0) {
        printf("PROBE-FAIL F_ADD_SEALS(F_SEAL_WRITE): errno=%d\n", errno);
        failures++;
    } else {
        char c = 'x';
        errno = 0;
        if (write((int)fd, &c, 1) == 1) {
            printf("PROBE-FAIL write after F_SEAL_WRITE: succeeded\n");
            failures++;
        } else if (errno != EPERM) {
            printf("PROBE-FAIL write after F_SEAL_WRITE: errno=%d, expected EPERM\n", errno);
            failures++;
        } else {
            printf("PROBE-PASS write after F_SEAL_WRITE: errno=EPERM\n");
        }
        errno = 0;
        if (ftruncate((int)fd, 4096) == 0) {
            printf("PROBE-FAIL ftruncate after F_SEAL_WRITE: succeeded\n");
            failures++;
        } else if (errno != EPERM) {
            printf("PROBE-FAIL ftruncate after F_SEAL_WRITE: errno=%d, expected EPERM\n", errno);
            failures++;
        } else {
            printf("PROBE-PASS ftruncate after F_SEAL_WRITE: errno=EPERM\n");
        }
    }
    close((int)fd);

    if (failures == 0)
        printf("PROBE-RESULT: PASS\n");
    else
        printf("PROBE-RESULT: FAIL (%d)\n", failures);
    return failures ? 1 : 0;
}
