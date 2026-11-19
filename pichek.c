// pichek.c — micro-probe: does a plain nanosleep+printf loop lose wakes
// on the R6 build? 4 threads (3 CFS + 1 FIFO-80) hammer 1ms sleeps and
// a counter; the main thread watches for >2s stalls. No futexes.
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
#include <pthread.h>
#include <sched.h>
#include <time.h>
#include <string.h>
static volatile unsigned long prog[4];
static volatile int tids[4];
static volatile int stop_now;
static void msleep(long ms){struct timespec ts={ms/1000,(ms%1000)*1000000L};nanosleep(&ts,NULL);}
static void *w(void *ap){
    int i=(int)(long)ap;
    tids[i]=syscall(224); /* gettid */
    if(i==3){struct sched_param sp={.sched_priority=80};sched_setscheduler(0,SCHED_FIFO,&sp);}
    unsigned long n=0;
    while(!stop_now){ prog[i]=++n; msleep(1); if((n%500)==0){printf("T%d n=%lu\n",i,n);}}
    return NULL;
}
int main(void){
    setvbuf(stdout,NULL,_IONBF,0);
    pthread_t th[4];
    for(long i=0;i<4;i++)pthread_create(&th[i],NULL,w,(void*)i);
    for(int i=0;i<4;i++)while(!tids[i])msleep(2);
    unsigned long last[4];int stalls=0;
    for(int round=0;round<10;round++){
        msleep(1000);
        for(int i=0;i<4;i++)last[i]=prog[i];
        msleep(2000);
        for(int i=0;i<4;i++)
            if(prog[i]==last[i]){stalls++;printf("STALL T%d tid=%d stuck at %lu (round %d)\n",i,tids[i],last[i],round);}
    }
    stop_now=1;
    for(int i=0;i<4;i++)pthread_join(th[i],NULL);
    printf("RES|pichek|%s|stalls=%d\n",stalls?"FAIL":"PASS",stalls);
    return stalls?1:0;
}
