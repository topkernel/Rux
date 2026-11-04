/*
 * Copyright 2017, Red Hat, Inc.
 *
 * Test RWF_NOWAIT.
 *
 * RWF_NOWAIT will cause -EAGAIN to be returned in the io_event for
 * any I/O that cannot be serviced without blocking the submission
 * thread.  Instances covered by the kernel at the time this test was
 * written include:
 * - O_DIRECT I/O to a file offset that has populated page cache pages
 * - the submission context cannot obtain the inode lock
 * - space allocation is necessary
 * - we need to wait for other I/O (e.g. in the misaligned I/O case)
 * - ...
 *

 * The easiest of these to test is that a direct I/O is writing to a
 * file offset with populated page cache.  We also test to ensure that
 * we can perform I/O in the absence of the above conditions.
 *
 * Author: Jeff Moyer <jmoyer@redhat.com>
 */
#define _GNU_SOURCE
#include <stdio.h>
#include <unistd.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <signal.h>
#include <sched.h>
#include <libaio.h>

#define TEMPLATE "21.XXXXXX"
#define BUFLEN 4096

#ifndef RWF_NOWAIT
#define RWF_NOWAIT	0x00000008
#endif

int
open_temp_file()
{
	int fd;
	char temp_file[sizeof(TEMPLATE)];

	strncpy(temp_file, TEMPLATE, sizeof(temp_file));
	fd = mkstemp(temp_file);
	if (fd < 0) {
		perror("mkstemp");
		return -1;
	}
	unlink(temp_file);
	return fd;
}

int
test_main()
{
	int fd, flags;
	int ret;
	io_context_t ctx;
	struct iocb iocb, *iocbp = &iocb;
	struct io_event event;
	char buf[BUFLEN] __attribute__((aligned (4096)));
	struct iovec iov;

	fd = open_temp_file();
	if (fd < 0)
		return 1;

	memset(&ctx, 0, sizeof(ctx));
	ret = io_setup(1, &ctx);
	if (ret != 0) {
		fprintf(stderr, "io_setup failed with %d\n", ret);
		return 1;
	}

	/*
	 * Perform a buffered write to a file.  This instantiates the
	 * block and adds the page to the page cache.
	 */
	memset(buf, 0xa, BUFLEN);
	ret = write(fd, buf, BUFLEN);
	if (ret != BUFLEN) {
		perror("write");
		return 1;
	}

	/*
	 * Now attempt an aio/dio pwritev2 with the RWF_NONBLOCK flag
	 * set.
	 */
	flags = fcntl(fd, F_GETFL);
	ret = fcntl(fd, F_SETFL, flags | O_DIRECT);
	if (ret < 0) {
		/* SKIP this test if O_DIRECT is not available on this fs */
		if (errno == EINVAL)
			return 3;
		perror("fcntl");
		return 1;
	}

	memset(buf, 0, BUFLEN);
	iov.iov_base = buf;
	iov.iov_len = BUFLEN;
	io_prep_preadv2(&iocb, fd, &iov, 1, 0, RWF_NOWAIT);

	ret = io_submit(ctx, 1, &iocbp);

	/*
	 * io_submit will return -EINVAL if RWF_NOWAIT is not supported by
	 * the kernel, and EOPNOTSUPP if it's not supported by the fs.
	 */
	if (ret != 1) {
		if (ret == -EINVAL || ret == -ENOTSUP) {
			fprintf(stderr, "RWF_NOWAIT not supported by %s.\n",
				ret == -EINVAL ? "kernel" : "file system");
			/* skip this test */
			return 3;
		}
		errno = -ret;
		perror("io_submit");
		return 1;
	}

	ret = io_getevents(ctx, 1, 1, &event, NULL);
	if (ret != 1) {
		errno = -ret;
		perror("io_getevents");
		return 1;
	}

	/*
	 * We expect -EAGAIN due to the existence of a page cache page
	 * for the file system block we are writing.
	 */
	if (event.res != -EAGAIN) {
		fprintf(stderr, "Expected -EAGAIN, got %lu\n", event.res);
		return 1;
	}

	/*
	 * An O_DIRECT write to the page will force the page out of the
	 * page cache, allowing the subsequent RWF_NOWAIT I/O to complete.
	 */
	ret = pwrite(fd, buf, BUFLEN, 0);
	if (ret != BUFLEN) {
		perror("write");
		return 1;
	}

	/*
	 * Now retry the RWF_NOWAIT I/O.  This should succeed.
	 */
	ret = io_submit(ctx, 1, &iocbp);
	if (ret != 1) {
		errno = -ret;
		perror("io_submit");
		return 1;
	}

	ret = io_getevents(ctx, 1, 1, &event, NULL);
	if (ret != 1) {
		errno = -ret;
		perror("io_getevents");
		return 1;
	}

	if (event.res != BUFLEN) {
		fprintf(stderr, "Expected %d, got %lu\n", BUFLEN, event.res);
		return 1;
	}

	return 0;
}
/*
 * Local variables:
 *  mode: c
 *  c-basic-offset: 8
 * End:
 */
