// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * Copyright (c) 2022 Richard Palethorpe <rpalethorpe@suse.com>
 * Copyright (c) 2023 Andrea Cervesato <andrea.cervesato@suse.com>
 */

#include <stdio.h>
#include <stdint.h>
#include <stddef.h>
#include <stdlib.h>
#include <string.h>
#include <assert.h>
#include "unpack.h"
#include "message.h"

void mp_unpacker_init(struct mp_unpacker *up)
{
	up->msg = NULL;
	up->pos = 0;
	up->needs_length = -1;
	up->status = -1;
}

void mp_unpacker_reserve(struct mp_unpacker *up, struct mp_message *msg)
{
	up->msg = msg;
}

inline int mp_unpacker_status(struct mp_unpacker *const up)
{
	return up->status;
}

/* copy all given data inside the message */
static size_t mp_populate(
	struct mp_unpacker *up,
	const uint8_t *const data,
	const size_t len)
{
	size_t tocopy = 0;
	size_t left;

	if (len) {
		left = up->msg->length - up->pos;
		tocopy = left < len ? left : len;
		memcpy(up->msg->data + up->pos, data, tocopy);

		(up->pos) += tocopy;
	}

	return tocopy;
}

size_t mp_unpacker_feed(
	struct mp_unpacker *up,
	uint8_t *const data,
	const size_t size)
{
	assert(up);
	assert(data);
	assert(size > 0);

	if (!up->pos) {
		size_t basesize = mp_message_base_size(data[0]);
		if (!basesize) {
			up->status = MP_UNPACKER_TYPE_ERROR;
			return 0;
		}

		up->msg->data = (uint8_t *)realloc(
			up->msg->data,
			basesize);

		assert(up->msg->data);
		memset(up->msg->data, 0, basesize);

		up->msg->length = basesize;
	}

	size_t counter = mp_populate(up, data, size);

	if (up->msg->length > 1 && !up->msg->data[0]) {
		up->status = MP_UNPACKER_TYPE_ERROR;
		return 0;
	}

	/* check if we need to store data length */
	if (up->needs_length < 0) {
		switch (up->msg->data[0]) {
		case MP_BIN8:
		case MP_BIN16:
		case MP_BIN32:
		case MP_STR8:
		case MP_STR16:
		case MP_STR32:
			up->needs_length = 1;
			break;
		default:
			up->needs_length = 0;
			break;
		}
	}

	if (up->needs_length > 0) {
		/* reallocate enough memory for messages which need length */
		size_t fullsize = mp_message_full_size(up->msg);

		if (fullsize > up->msg->length) {
			up->msg->length = fullsize;
			up->msg->data = (uint8_t *)realloc(
				up->msg->data,
				fullsize);
			assert(up->msg->data);

			/* we don't need to find length anymore */
			up->needs_length = 0;

			/* copy remaining data (if available) */
			counter += mp_populate(
				up,
				data + counter,
				size - counter);
		}

		/* sometimes message data is empty, but we save the length */
		if (up->needs_length && up->pos == up->msg->length)
			up->needs_length = 0;
	}

	if (up->status != MP_UNPACKER_TYPE_ERROR) {
		if ((up->pos == up->msg->length) && up->needs_length == 0) {
			up->status = MP_UNPACKER_SUCCESS;
			up->needs_length = -1;
			up->pos = 0;
		} else {
			up->status = MP_UNPACKER_NEED_DATA;
		}
	}

	return counter;
}
