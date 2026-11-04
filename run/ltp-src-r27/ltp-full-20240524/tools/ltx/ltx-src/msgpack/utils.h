// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * Copyright (c) 2022 Richard Palethorpe <rpalethorpe@suse.com>
 * Copyright (c) 2023 Andrea Cervesato <andrea.cervesato@suse.com>
 */

#ifndef MSGPACK_H
#define MSGPACK_H

#if __BYTE_ORDER__ == __ORDER_LITTLE_ENDIAN__
 #define SWAP_BYTES(value) __builtin_bswap64(value)
#else
 #define SWAP_BYTES(value) value
#endif

#include <stdint.h>
#include <string.h>

union extract_uint64
{
	uint64_t value;
	uint8_t data[sizeof(uint64_t)];
};

static int mp_read_number_bytes(const uint64_t value)
{
	int len;

	if (value <= 0xff)
		len = 1;
	else if (value <= 0xffff)
		len = 2;
	else if (value <= 0xffffffff)
		len = 4;
	else
		len = 8;

	return len;
}

static uint64_t mp_read_number(uint8_t *const data, const int bytes)
{
	union extract_uint64 ext;

	memset(ext.data, 0, sizeof(uint64_t));
	memcpy(ext.data + sizeof(uint64_t) - bytes, data, bytes);

	uint64_t value = SWAP_BYTES(ext.value);
	return value;
}

static void mp_write_number(const uint64_t value, uint8_t *data, const int bytes)
{
	union extract_uint64 ext;

	memset(ext.data, 0, sizeof(uint64_t));

	ext.value = SWAP_BYTES(value);
	memcpy(data, ext.data + sizeof(uint64_t) - bytes, bytes);
}

#endif
