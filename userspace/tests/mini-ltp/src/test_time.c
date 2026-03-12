/*
 * Test: time() / gettimeofday()
 * Test time system calls
 */

#include <time.h>
#include <sys/time.h>
#include <unistd.h>

int main(void)
{
    time_t t1, t2;
    struct timeval tv1, tv2;

    /* Test time() */
    t1 = time(NULL);
    if (t1 == (time_t)-1) return 1;

    /* Wait for a short period */
    sleep(1);

    t2 = time(NULL);
    if (t2 == (time_t)-1) return 2;

    /* Time should have increased */
    if (t2 <= t1) return 3;

    /* Test gettimeofday() */
    if (gettimeofday(&tv1, NULL) < 0) return 4;

    sleep(1);

    if (gettimeofday(&tv2, NULL) < 0) return 5;

    /* Seconds should have increased */
    if (tv2.tv_sec <= tv1.tv_sec) return 6;

    return 0;
}
