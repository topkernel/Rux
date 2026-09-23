/* musl pthread smoke: create/join/mutex/cond — the W1 acceptance target. */
#include <pthread.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

static pthread_mutex_t mtx = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t cond = PTHREAD_COND_INITIALIZER;
static int shared = 0, ready = 0, done = 0;

static void *worker(void *arg) {
    long id = (long)arg;
    pthread_mutex_lock(&mtx);
    shared += 1;
    if (id == 3) { ready = 1; pthread_cond_broadcast(&cond); }
    while (!ready) pthread_cond_wait(&cond, &mtx);
    done++;
    pthread_mutex_unlock(&mtx);
    return (void *)(id * 100);
}

int main(void) {
    pthread_t th[4];
    void *ret;
    for (long i = 0; i < 4; i++) {
        int r = pthread_create(&th[i], NULL, worker, (void *)i);
        if (r) { printf("create-fail %d\n", r); return 1; }
    }
    for (int i = 0; i < 4; i++) {
        pthread_join(th[i], &ret);
        printf("join %d ret=%ld\n", i, (long)ret);
    }
    printf("shared=%d done=%d\n", shared, done);
    if (shared == 4 && done == 4) { puts("PTHREAD PASS"); return 0; }
    puts("PTHREAD FAIL"); return 1;
}
