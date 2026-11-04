// SPDX-License-Identifier: GPL-2.0-or-later
#ifndef __PI_STRESS_H
#define __PI_STRESS_H
#include <stdio.h>
#include <errno.h>
#include <stdlib.h>
#include <stdarg.h>
#include <pthread.h>
#include <sched.h>
#include <unistd.h>
#include <string.h>
#include <signal.h>
#include <getopt.h>
#include <sys/types.h>
#include <sys/mman.h>
#include <sys/wait.h>
#include <sys/syscall.h>
#include <termios.h>
#include <stdint.h>
#include <inttypes.h>
#include <limits.h>

#include "rt-sched.h"
#include "rt-utils.h"
#include "rt-error.h"

/* test timeout */
#define TIMEOUT 2

/* determine if the C library supports Priority Inheritance mutexes */
#if defined(_POSIX_THREAD_PRIO_INHERIT) && _POSIX_THREAD_PRIO_INHERIT != -1
#define HAVE_PI_MUTEX 1
#else
#define HAVE_PI_MUTEX 0
#endif

#if HAVE_PI_MUTEX == 0
#error "Can't run this test without PI Mutex support"
#endif

#define SUCCESS 0
#define FAILURE 1

/* cursor control */
#define UP_ONE "\033[1A"
#define DOWN_ONE "\033[1B"

#define pi_info(fmt, arg...) \
	do { info(verbose, fmt, ## arg); } while (0)
#define pi_debug(fmt, arg...) \
	do { debug(debugging, fmt, ## arg); } while (0)
#define pi_error(fmt, arg...) \
	do { err_msg(fmt, ## arg); have_errors = 1; } while (0)

#define NUM_TEST_THREADS 3
#define NUM_ADMIN_THREADS 1

#define SA_INIT_LOW	(1 << 0)
#define SA_INIT_MED	(1 << 1)
#define SA_INIT_HIGH	(1 << 2)
#define SA_INIT_ADMIN	(1 << 3)

/* number of consecutive watchdog hits before quitting */
#define WATCHDOG_LIMIT 5

struct group_parameters {

	/* group id (index) */
	int id;

	/* cpu this group is bound to */
	long cpu;

	/* threads in the group */
	pthread_t low_tid;
	pthread_t med_tid;
	pthread_t high_tid;

	/* number of machine iterations to perform */
	int inversions;

	/* group mutex */
	pthread_mutex_t mutex;

	/* state barriers */
	pthread_barrier_t start_barrier;
	pthread_barrier_t locked_barrier;
	pthread_barrier_t elevate_barrier;
	pthread_barrier_t finish_barrier;

	/* Either everyone goes through the loop, or else no-ones does */
	pthread_barrier_t loop_barr;
	pthread_mutex_t loop_mtx;	/* Protect access to int loop */
	int loop;	/* boolean, loop or not, connected to shutdown */

	/* state variables */
	volatile int watchdog;

	/* total number of inversions performed */
	unsigned long total;

	/* total watchdog hits */
	int watchdog_hits;

};

/* forward prototypes */
void *low_priority(void *arg);
void *med_priority(void *arg);
void *high_priority(void *arg);
void *reporter(void *arg);
void *watchdog(void *arg);
int setup_thread_attr(pthread_attr_t *attr, struct sched_attr *sa,
		      cpu_set_t *mask);
int set_cpu_affinity(cpu_set_t *test_mask, cpu_set_t *admin_mask);
void process_command_line(int argc, char **argv);
void usage(int error);
int block_signals(void);
int allow_sigterm(void);
void set_shutdown_flag(void);
int initialize_group(struct group_parameters *group);
int create_group(struct group_parameters *group);
unsigned long total_inversions(void);
void banner(void);
void summary(void);
void write_stats(FILE *f, void *data);
void wait_for_termination(void);
int barrier_init(pthread_barrier_t *b, const pthread_barrierattr_t *attr,
		 unsigned int count, const char *name);
void setup_sched_attr(struct sched_attr *attr, int policy, int prio);
void setup_sched_config(int policy);

#endif /* __PI_STRESS_H */
