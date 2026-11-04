/*
 * Copyright (C) 2006-2018 Free Software Foundation, Inc.
 * Copyright (C) 2018 Christoph Hellwig.
 * License: LGPLv2.1 or later.
 *
 * Description: test aio poll and io_pgetevents signal handling.
 *
 * Very roughly based on glibc tst-pselect.c.
 */
#include <errno.h>
#include <signal.h>
#include <stdio.h>
#include <unistd.h>
#include <sys/poll.h>
#include <sys/wait.h>
#include <stdlib.h>

static volatile int handler_called;

static void
handler(int sig)
{
	handler_called = 1;
}

int test_main(void)
{
	struct timespec to = { .tv_sec = 0, .tv_nsec = 500000000 };
	pid_t parent = getpid(), p;
	int pipe1[2], pipe2[2];
	struct sigaction sa = { .sa_flags = 0 };
	sigset_t sigmask;
	struct io_context *ctx = NULL;
	struct io_event ev;
	struct iocb iocb;
	struct iocb *iocbs[] = { &iocb };
	int ret;

	sigemptyset(&sa.sa_mask);

	sa.sa_handler = handler;
	if (sigaction(SIGUSR1, &sa, NULL) != 0) {
		printf("sigaction(1) failed\n");
		return 1;
	}

	sa.sa_handler = SIG_IGN;
	if (sigaction(SIGCHLD, &sa, NULL) != 0) {
		printf("sigaction(2) failed\n");
		return 1;
	}

	sigemptyset(&sigmask);
	sigaddset(&sigmask, SIGUSR1);
	if (sigprocmask(SIG_BLOCK, &sigmask, NULL) != 0) {
		printf("sigprocmask failed\n");
		return 1;
	}

	if (pipe(pipe1) != 0 || pipe(pipe2) != 0) {
		printf("pipe failed\n");
		return 1;
	}

	sigprocmask(SIG_SETMASK, NULL, &sigmask);
	sigdelset(&sigmask, SIGUSR1);

	p = fork();
	switch (p) {
	case -1:
		printf("fork failed\n");
		exit(2);
	case 0:
		close(pipe1[1]);
		close(pipe2[0]);

		ret = io_setup(1, &ctx);
		if (ret) {
			printf("child: io_setup failed: %s\n", strerror(-ret));
			return 1;
		}

		io_prep_poll(&iocb, pipe1[0], POLLIN);
		ret = io_submit(ctx, 1, iocbs);
		if (ret != 1) {
			/* if poll isn't supported, skip the test */
			if (ret == -EINVAL)
				return 3;
			printf("child: io_submit failed: %s\n", strerror(-ret));
			return 1;
		}

		do {
			if (getppid() != parent) {
				printf("parent died\n");
				exit(2);
			}
			ret = io_pgetevents(ctx, 1, 1, &ev, &to, &sigmask);
		} while (ret == 0);

		/* SKIP if the syscall has not been implemented. */
		if (ret == -ENOSYS)
			return 3;

		if (ret != -EINTR) {
			printf("child: io_pgetevents did not set errno to "
			       "EINTR: %s\n", strerror(-ret));
			return 1;
		}

		do {
			errno = 0;
			ret = write(pipe2[1], "foo", 3);
		} while (ret == -1 && errno == EINTR);

		exit(0);
	default:
		close(pipe1[0]);
		close(pipe2[1]);

		io_prep_poll(&iocb, pipe2[0], POLLIN);

		ret = io_setup(1, &ctx);
		if (ret) {
			printf("parent: io_setup failed: %s\n", strerror(-ret));
			return 1;
		}

		ret = io_submit(ctx, 1, iocbs);
		if (ret != 1) {
			/* if poll isn't supported, skip the test */
			if (ret == -EINVAL)
				return 3;
			printf("parent: io_submit failed with %d: %s\n",
			       ret, strerror(-ret));
			return 1;
		}

		kill(p, SIGUSR1);

		ret = io_pgetevents(ctx, 1, 1, &ev, NULL, &sigmask);
		/* SKIP if the syscall has not been implemented. */
		if (ret == -ENOSYS)
			return 3;
		if (ret < 0) {
			printf("parent: io_pgetevents failed: %s\n",
			       strerror(-ret));
			return 1;
		}
		if (ret != 1) {
			printf("parent: io_pgetevents did not report event\n");
			return 1;
		}
		if (ev.obj != &iocb) {
			printf("parent: io_pgetevents reports wrong fd\n");
			return 1;
		}
		if ((ev.res & POLLIN) != POLLIN) {
			printf("parent: io_pgetevents did not report readable fd\n");
			return 1;
		}

		return 0;
	}
}
/*
 * Local variables:
 *  mode: c
 *  c-basic-offset: 8
 * End:
 */
