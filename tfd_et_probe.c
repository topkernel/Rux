/*
 * tfd_et_probe — the FFRT DelayedWorker pattern in isolation.
 *
 * FFRT's delayed task engine (foundation/resourceschedule/ffrt
 * sync/delayed_worker.cpp) drives EVERY delayed FFRT task through:
 *   timerfd_create(CLOCK_MONOTONIC, TFD_NONBLOCK|TFD_CLOEXEC)
 *   epoll_ctl(ADD, tfd, EPOLLIN|EPOLLET)      // edge-triggered
 *   loop { timerfd_settime(TFD_TIMER_ABSTIME, next_deadline);
 *          epoll_wait(180s) }                  // and NEVER reads the tfd
 *
 * If the kernel loses the expiry->epoll edge anywhere (wake lost while
 * the worker is between waits; entry-poll misses a readable ET fd;
 * ABSTIME re-arm produces no new edge), delayed tasks rot in the map,
 * every serial queue behind them starves, and services wedge.
 *
 * This probe counts MISSES: rounds where the timer's deadline passed
 * but epoll_wait(timeout > deadline-now + slack) reported nothing.
 *
 * Verdict line: TFDPROBE-DONE rounds=<N> miss_in_wait=<a> miss_after_gap=<b>
 *   Linux reference behavior: a == 0 && b == 0.
 */
#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>
#include <sys/epoll.h>
#include <sys/timerfd.h>
#include <time.h>

static uint64_t now_ns(void)
{
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (uint64_t)ts.tv_sec * 1000000000ull + (uint64_t)ts.tv_nsec;
}

static int arm_abs(int tfd, uint64_t deadline_ns)
{
    struct itimerspec its;
    memset(&its, 0, sizeof(its));
    its.it_value.tv_sec = (time_t)(deadline_ns / 1000000000ull);
    its.it_value.tv_nsec = (long)(deadline_ns % 1000000000ull);
    return timerfd_settime(tfd, TFD_TIMER_ABSTIME, &its, NULL);
}

int main(void)
{
    const int ROUNDS = 300;
    const uint64_t DELAY_NS = 20ull * 1000 * 1000;  /* 20ms per round */
    int miss_in_wait = 0, miss_after_gap = 0, reported = 0;
    struct epoll_event ev, out;

    setvbuf(stdout, NULL, _IONBF, 0);

    int tfd = timerfd_create(CLOCK_MONOTONIC, TFD_NONBLOCK | TFD_CLOEXEC);
    if (tfd < 0) {
        printf("TFDPROBE-FAIL create errno=%d\n", errno);
        return 1;
    }
    int ep = epoll_create1(EPOLL_CLOEXEC);
    if (ep < 0) {
        printf("TFDPROBE-FAIL epoll errno=%d\n", errno);
        return 1;
    }
    memset(&ev, 0, sizeof(ev));
    ev.events = EPOLLIN | EPOLLET;
    ev.data.fd = tfd;
    if (epoll_ctl(ep, EPOLL_CTL_ADD, tfd, &ev) != 0) {
        printf("TFDPROBE-FAIL ctl errno=%d\n", errno);
        return 1;
    }

    for (int i = 0; i < ROUNDS; i++) {
        int gap = (i % 5 == 0); /* every 5th round: sleep past the
                                 * deadline BEFORE waiting, so the
                                 * expiry lands while nobody is in
                                 * epoll_wait (the FFRT worker is
                                 * running HandleWork then). */
        uint64_t deadline = now_ns() + DELAY_NS;
        if (arm_abs(tfd, deadline) != 0) {
            printf("TFDPROBE-FAIL settime errno=%d\n", errno);
            return 1;
        }
        if (gap)
            usleep(80 * 1000); /* 4x the delay: expiry lands here */

        /* Wait well past the deadline: a healthy kernel must report
         * the (already or about-to-be) readable timerfd. */
        int nfds = epoll_wait(ep, &out, 1, 500);
        if (nfds == 1 && (out.events & EPOLLIN)) {
            reported++;
            /* FFRT never reads the tfd — neither do we. */
        } else if (nfds == 0) {
            if (gap)
                miss_after_gap++;
            else
                miss_in_wait++;
            printf("TFDPROBE-MISS round=%d gap=%d errno=%d\n", i, gap, errno);
        } else {
            printf("TFDPROBE-ERR round=%d nfds=%d errno=%d\n", i, nfds, errno);
            return 1;
        }
    }
    printf("TFDPROBE-DONE rounds=%d reported=%d miss_in_wait=%d miss_after_gap=%d\n",
           ROUNDS, reported, miss_in_wait, miss_after_gap);
    return (miss_in_wait || miss_after_gap) ? 2 : 0;
}
