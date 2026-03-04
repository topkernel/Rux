/*
 * Test: time() / gettimeofday()
 * 测试时间系统调用
 */

#include <time.h>
#include <sys/time.h>
#include <unistd.h>

int main(void)
{
    time_t t1, t2;
    struct timeval tv1, tv2;

    /* 测试 time() */
    t1 = time(NULL);
    if (t1 == (time_t)-1) return 1;

    /* 等待一小段时间 */
    sleep(1);

    t2 = time(NULL);
    if (t2 == (time_t)-1) return 2;

    /* 时间应该增加了 */
    if (t2 <= t1) return 3;

    /* 测试 gettimeofday() */
    if (gettimeofday(&tv1, NULL) < 0) return 4;

    sleep(1);

    if (gettimeofday(&tv2, NULL) < 0) return 5;

    /* 秒数应该增加 */
    if (tv2.tv_sec <= tv1.tv_sec) return 6;

    return 0;
}
