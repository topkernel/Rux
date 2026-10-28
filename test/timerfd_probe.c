/* timerfd_probe.c — the timerfd_settime(2) contract.
 *
 * The reference kernel (kernel/fs/timerfd.c):
 *  - the itimerspec is validated BEFORE the old timer is cancelled, so a
 *    rejected call (EINVAL) leaves the running timer alone;
 *  - the old settings are copied out AFTER the timer has been
 *    reprogrammed, so a failing copy (EFAULT) still leaves the new timer
 *    programmed;
 *  - timerfd_setup() sets the new interval and clears the tick counter in
 *    both the arming and the disarming case, so a disarm leaves
 *    it_interval at the new value and the timer expired count at zero.
 */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/timerfd.h>
#include <unistd.h>

static int failures;

static void fail(const char *what, const char *detail)
{
    printf("PROBE-FAIL %s: %s\n", what, detail);
    failures++;
}

static void pass(const char *what)
{
    printf("PROBE-PASS %s\n", what);
}

static int new_timerfd(void)
{
    int fd = timerfd_create(CLOCK_MONOTONIC, 0);
    if (fd < 0) {
        printf("PROBE-FAIL timerfd_create: errno=%d\n", errno);
        failures++;
        return -1;
    }
    return fd;
}

int main(void)
{
    struct itimerspec its, cur;

    /* (1) A rejected itimerspec must not disarm the running timer.
     *
     * Arm a 10 s timer, then hand settime an it_value whose tv_nsec is
     * out of range: the call is EINVAL and the timer must still be
     * running afterwards. */
    int fd = new_timerfd();
    if (fd < 0)
        return 1;
    memset(&its, 0, sizeof(its));
    its.it_value.tv_sec = 10;
    if (timerfd_settime(fd, 0, &its, NULL) != 0) {
        fail("arm 10s", "timerfd_settime failed");
        close(fd);
        return 1;
    }
    memset(&its, 0, sizeof(its));
    its.it_value.tv_sec = 1;
    its.it_value.tv_nsec = 2000000000L; /* out of range */
    errno = 0;
    if (timerfd_settime(fd, 0, &its, NULL) != -1 || errno != EINVAL) {
        fail("invalid itimerspec", "expected EINVAL");
    } else {
        pass("invalid itimerspec is EINVAL");
    }
    if (timerfd_gettime(fd, &cur) != 0) {
        fail("gettime after EINVAL", "timerfd_gettime failed");
    } else if (cur.it_value.tv_sec <= 5) {
        /* The 10 s timer must still be running: a cancelled timer reports
         * it_value = 0. */
        char buf[128];
        snprintf(buf, sizeof(buf), "timer was disarmed by the rejected call (it_value=%ld)",
                 (long)cur.it_value.tv_sec);
        fail("rejected settime keeps the timer armed", buf);
    } else {
        pass("rejected settime keeps the timer armed");
    }
    close(fd);

    /* (2) A failing old_value copy must still leave the new timer
     * programmed: the reference copies `old` out after the reprogramming. */
    fd = new_timerfd();
    if (fd < 0)
        return 1;
    memset(&its, 0, sizeof(its));
    its.it_value.tv_sec = 10;
    if (timerfd_settime(fd, 0, &its, NULL) != 0) {
        fail("arm 10s (2)", "timerfd_settime failed");
        close(fd);
        return 1;
    }
    memset(&its, 0, sizeof(its));
    its.it_value.tv_sec = 1; /* the new, short timer */
    errno = 0;
    /* A bad old_value pointer: the kernel must answer EFAULT ... */
    if (timerfd_settime(fd, 0, &its, (struct itimerspec *)(void *)0x1) != -1 ||
        errno != EFAULT) {
        fail("bad old_value", "expected EFAULT");
    } else {
        pass("bad old_value is EFAULT");
    }
    /* ... and the NEW timer (1 s) must be the one that is armed. */
    if (timerfd_gettime(fd, &cur) != 0) {
        fail("gettime after EFAULT", "timerfd_gettime failed");
    } else if (cur.it_value.tv_sec > 5) {
        char buf[128];
        snprintf(buf, sizeof(buf), "the old 10s timer is still armed (it_value=%ld)",
                 (long)cur.it_value.tv_sec);
        fail("EFAULT old_value still reprograms the timer", buf);
    } else {
        pass("EFAULT old_value still reprograms the timer");
    }
    close(fd);

    /* (3) A disarm sets the new interval and clears the tick count. */
    fd = new_timerfd();
    if (fd < 0)
        return 1;
    memset(&its, 0, sizeof(its));
    its.it_value.tv_sec = 5;
    its.it_interval.tv_sec = 2;
    if (timerfd_settime(fd, 0, &its, NULL) != 0) {
        fail("arm 5s/2s", "timerfd_settime failed");
        close(fd);
        return 1;
    }
    memset(&its, 0, sizeof(its)); /* it_value = 0 -> disarm, it_interval = 0 */
    memset(&cur, 0xAA, sizeof(cur));
    errno = 0;
    if (timerfd_settime(fd, 0, &its, &cur) != 0) {
        fail("disarm", "timerfd_settime failed");
    } else {
        pass("disarm succeeds");
        /* the old value reports the settings the timer had BEFORE the
         * disarm (the remaining time and the old interval) */
        if (cur.it_value.tv_sec <= 0) {
            fail("disarm old_value", "the old it_value should be the remaining time");
        } else {
            pass("disarm old_value reports the previous remaining time");
        }
    }
    if (timerfd_gettime(fd, &cur) != 0) {
        fail("gettime after disarm", "timerfd_gettime failed");
    } else if (cur.it_value.tv_sec != 0 || cur.it_value.tv_nsec != 0) {
        fail("disarm clears the timer", "it_value is not zero");
    } else {
        pass("disarm clears the timer");
    }
    if (cur.it_interval.tv_sec != 0 || cur.it_interval.tv_nsec != 0) {
        char buf[128];
        snprintf(buf, sizeof(buf), "it_interval=%ld.%09ld, want 0 (the new interval)",
                 (long)cur.it_interval.tv_sec, (long)cur.it_interval.tv_nsec);
        fail("disarm applies the new interval", buf);
    } else {
        pass("disarm applies the new interval");
    }
    close(fd);

    if (failures == 0)
        printf("PROBE-RESULT: PASS\n");
    else
        printf("PROBE-RESULT: FAIL (%d)\n", failures);
    return failures ? 1 : 0;
}
