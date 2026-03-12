/*
 * Test: nanosleep()
 * Test high-precision sleep
 */

#include <time.h>
#include <sys/time.h>

int main(void)
{
    struct timespec req, rem;
    struct timeval tv1, tv2;
    long elapsed_ms;

    req.tv_sec = 0;
    req.tv_nsec = 100000000;  /* 100ms */

    gettimeofday(&tv1, NULL);

    if (nanosleep(&req, &rem) < 0) return 1;

    gettimeofday(&tv2, NULL);

    /* Calculate elapsed time (milliseconds) */
    elapsed_ms = (tv2.tv_sec - tv1.tv_sec) * 1000 +
                 (tv2.tv_usec - tv1.tv_usec) / 1000;

    /* Should have slept at least 90ms */
    if (elapsed_ms < 90) return 2;

    /* Should not exceed 200ms */
    if (elapsed_ms > 200) return 3;

    return 0;
}
