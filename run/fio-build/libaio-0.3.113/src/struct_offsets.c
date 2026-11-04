/*
 * Ensure that data structure offsets in the iocb.u union match.
 * Note that this code does not end up in the compiled object files.
 * Its sole purpose is to abort the build if the structure padding
 * is incorrect.
 */
#include <stddef.h>
#include <assert.h>
#include <libaio.h>

void
offset_check(void)
{
	static_assert(offsetof(struct iocb, u.v.nr) ==
		      offsetof(struct iocb, u.c.nbytes),
		      "Error: iocb.u.v.nr does not match the offset of iocb.u.c.nbytes.");
	static_assert(offsetof(struct iocb, u.v.offset) ==
		      offsetof(struct iocb, u.c.offset),
		      "Error: iocb.u.v.offset does not match the offset of iocb.u.c.offset");
	static_assert(offsetof(struct iocb, u.saddr.len) ==
		      offsetof(struct iocb, u.c.nbytes),
		      "Error: iocb.u.saddr.len does not match the offset of iocb.u.c.nbytes");
}
