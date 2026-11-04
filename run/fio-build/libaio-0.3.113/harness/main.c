#define _GNU_SOURCE
#include <stdio.h>
#include <errno.h>
#include <assert.h>
#include <stdlib.h>

#include <sys/types.h>
#include <sys/stat.h>
#include <fcntl.h>
#include <unistd.h>

#include <libaio.h>

#if __LP64__ == 0
#define KERNEL_RW_POINTER	((void *)0xc0010000)
#else
//#warning Not really sure where kernel memory is.  Guessing.
#define KERNEL_RW_POINTER	((void *)0xffffffff81000000)
#endif


char test_name[] = TEST_NAME;

#include TEST_NAME

int main(void)
{
	int res;
	const char *test_result;

#if defined(SETUP)
	SETUP;
#endif

	res = test_main();
	switch(res) {
	case 0:
		test_result = "PASSED";
		break;
	case 3:
		test_result = "SKIPPED";
		break;
	default:
		test_result = "FAILED";
		res = 1;
		break;
	}

	printf("test %s completed %s.\n", test_name, test_result);
	fflush(stdout);
	return res;
}
