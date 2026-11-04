/*
 * Copyright 2017, Red Hat, Inc.
 * Author: Jeff Moyer <jmoyer@redhat.com>
 * Based on test code from Mauricio Faria de Oliveira
 * 			   <mauricfo@linux.vnet.ibm.com>
 * License: GPLv2
 *
 * Description: Ensure that aio-max-nr requests can be allocated, or,
 * if not, that the reason is not faulty accounting.
 */
#include <libaio.h>
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
#include <string.h>
#include <sys/types.h>
#include <sys/wait.h>

#define FAIL 1

#define AIO_MAX_NR	"/proc/sys/fs/aio-max-nr"
#define AIO_NR		"/proc/sys/fs/aio-nr"

static unsigned aio_max_nr;

int
read_proc_val(const char *path, unsigned *val)
{
	FILE *fp;
	int ret;

	fp = fopen(path, "r");
	if (!fp) {
		fprintf(stderr, "Unable to open proc file \"%s\" for reading\n",
			path);
		return FAIL;
	}

	ret = fscanf(fp, "%u\n", val);
	fclose(fp);

	if (ret == EOF) {
		fprintf(stderr, "Failed to read from proc file \"%s\"\n",
			path);
		return FAIL;
	}

	return 0;
}

/*
 * Create as many ioctx-s with nr_events each as possible (up to aio_max_nr).
 * Report any failures of -EAGAIN.
 */
int
do_alloc_ioctxs(int nr_events)
{
	long ret;
	unsigned i, avail, aio_nr, nr_ctxs;
	io_context_t *ioctx;

	ret = read_proc_val(AIO_NR, &aio_nr);
	if (ret)
		return FAIL;

	avail = aio_max_nr - aio_nr;
	nr_ctxs = avail / nr_events;
	ioctx = calloc(nr_ctxs, sizeof(*ioctx));
	if (!ioctx) {
		fprintf(stderr, "allocating %u ioctx-s failed with %d\n",
			nr_ctxs, errno);
		return FAIL;
	}

	fprintf(stderr, "Creating %u ioctx-s with %u events each...\n", nr_ctxs,
		nr_events);
	fflush(stderr);
	for (i = 0; i < nr_ctxs; i++) {
		ret = io_setup(nr_events, &ioctx[i]);
		if (ret) {
			/*
			 * EAGAIN is the only failure case we're interested
			 * in.  -ENOMEM, for example, is expected in this
			 * test.
			 */
			if (ret != -EAGAIN)
				break;

			fprintf(stderr,"io_setup(%u) failed on iteration %d.\n",
				nr_events, i);
			fprintf(stderr, "allocated %u of %u possible events.\n",
				nr_events * i, aio_max_nr);
			ret = read_proc_val(AIO_NR, &aio_nr);
			if (ret == 0)
				fprintf(stderr, "aio_nr is currently at %u\n",
					aio_nr);

			free(ioctx);
			return FAIL;
		}
	}
	fprintf(stderr, "Successfully created %u io_context-s\n", i);
	if (i < nr_ctxs - 1)
		fprintf(stderr, "Last io_setup call returned %ld (%s)\n", ret,
			strerror(-ret));
	fflush(stderr);

	return 0;
}

/*
 * We fork off a child to do the actual work.  The reason is that each
 * io_destroy will incur an rcu grace period to complete.  That really
 * adds up, depending on the number of io_contexts created.  We take
 * advantage of an optimization in the kernel that waits for all
 * contexts to be torn down in one grace period.  In other words, just
 * exiting the process without tearing down the ioctx-s using
 * io_destroy is way faster.
 */
int
alloc_ioctxs(int nr_events)
{
	pid_t child;
	int ret, status;

	child = fork();
	switch (child) {
	case 0: /* child */
		ret = do_alloc_ioctxs(nr_events);
		exit(ret);
	case -1:
		fprintf(stderr, "fork() failed with %d\n", errno);
		return FAIL;
	default:
		break;
	}
	if (waitpid(child, &status, 0) < 0)
		return FAIL;
	if (WIFEXITED(status) && WEXITSTATUS(status) == 0)
		return 0;

	return FAIL;
}

int
test_main()
{
	int ret;
	unsigned nr_events;

	ret = read_proc_val(AIO_MAX_NR, &aio_max_nr);
	if (ret)
		return FAIL;

	fprintf(stderr, "aio_max_nr: %u\n", aio_max_nr);

	nr_events = 1;
	while (1) {
		ret = alloc_ioctxs(nr_events);
		if (ret)
			return FAIL;

		if (nr_events == aio_max_nr)
			break;

		nr_events *= 2;
		if (nr_events > aio_max_nr)
			nr_events = aio_max_nr;
	}

	return 0;
}
/*
 * Local variables:
 *  mode: c
 *  c-basic-offset: 8
 * End:
 */
