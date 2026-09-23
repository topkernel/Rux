#include <pthread.h>
#include <stdio.h>
#include <string.h>
static pthread_mutex_t m = PTHREAD_MUTEX_INITIALIZER;
static int step = 0;
static void *plain(void *a){ printf("P%ld\n",(long)a); return (void*)0; }
static void *locked(void *a){
    pthread_mutex_lock(&m); step++; printf("L%ld(%d)\n",(long)a,step); pthread_mutex_unlock(&m);
    return (void*)0;
}
int main(void){
    printf("A:start\n");
    // T1: 2 plain threads
    pthread_t t1,t2;
    pthread_create(&t1,0,plain,(void*)1);
    pthread_create(&t2,0,plain,(void*)2);
    pthread_join(t1,0); pthread_join(t2,0);
    printf("B:plain2 done\n");
    // T2: 4 locked threads
    pthread_t ts[4];
    for(long i=0;i<4;i++) pthread_create(&ts[i],0,locked,(void*)i);
    for(int i=0;i<4;i++) pthread_join(ts[i],0);
    printf("C:mutex4 done step=%d\n", step);
    // T3: cond minimal
    pthread_mutex_t m2 = PTHREAD_MUTEX_INITIALIZER;
    pthread_cond_t c = PTHREAD_COND_INITIALIZER;
    pthread_mutex_lock(&m2);
    step = 0;
    pthread_t tc;
    pthread_create(&tc,0,locked,(void*)9);  // will block on m2
    pthread_cond_wait(&c,&m2);              // main waits
    pthread_mutex_unlock(&m2);
    pthread_join(tc,0);
    printf("D:cond done\n");
    return 0;
}
