// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * Copyright (c) 2022 Richard Palethorpe <rpalethorpe@suse.com>
 * Copyright (c) 2023 Andrea Cervesato <andrea.cervesato@suse.com>
 */

#ifndef MSGPACK_UNPACK_H
#define MSGPACK_UNPACK_H

#include <stddef.h>
#include <stdint.h>
#include "message.h"

/* Current unpacking operation status */
enum {
	MP_UNPACKER_SUCCESS = 0,
	MP_UNPACKER_NEED_DATA,
	MP_UNPACKER_TYPE_ERROR,
};

/* object used to unpack messages */
struct mp_unpacker
{
	struct mp_message *msg;
	size_t pos;
	int needs_length;
	int status;
};

/* Initialize unpacker object */
void mp_unpacker_init(struct mp_unpacker *up);

/* Reserve message for unpacker object */
void mp_unpacker_reserve(struct mp_unpacker *up, struct mp_message *msg);

/* Return the current unpacking operation status */
int mp_unpacker_status(struct mp_unpacker *const up);

/* Feed buffer with new data and unpack given message.
 * It returns the number of bytes which have been used inside `data`.
 */
size_t mp_unpacker_feed(
	struct mp_unpacker *up,
	uint8_t *data,
	const size_t size);

#endif /* MSGPACK_UNPACK_H */
