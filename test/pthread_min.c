#include <pthread.h>
#include <stdio.h>
static void *w(void *a){ printf("thread %ld alive\n",(long)a); return (void*)((long)a+1); }
int main(void){
    printf("START\n");
    pthread_t t; void *r;
    int e = pthread_create(&t,0,w,(void*)7);
    printf("create=%d\n", e);
    if(e) return 1;
    e = pthread_join(t,&r);
    printf("join=%d ret=%ld\n", e, (long)r);
    return 0;
}
