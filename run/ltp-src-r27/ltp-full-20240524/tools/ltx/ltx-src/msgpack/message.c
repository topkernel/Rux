// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * Copyright (c) 2022 Richard Palethorpe <rpalethorpe@suse.com>
 * Copyright (c) 2023 Andrea Cervesato <andrea.cervesato@suse.com>
 */

#include <stdio.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <assert.h>
#include "message.h"
#include "utils.h"

static void mp_message_alloc(struct mp_message *msg, const size_t bytes)
{
	msg->length = bytes;
	msg->data = (uint8_t *) realloc(msg->data, msg->length);
}

void mp_message_init(struct mp_message *msg)
{
	assert(msg);

	msg->length = 0;
	msg->data = NULL;
}

void mp_message_destroy(struct mp_message *msg)
{
	assert(msg);

	if (msg->data) {
		free(msg->data);
		msg->data = NULL;
	}
}

int mp_message_type(struct mp_message *msg)
{
	assert(msg);
	assert(msg->data);

	int type = -1;

	if (msg->data[0] <= MP_FIXINT127 || \
		msg->data[0] == MP_UINT8 ||
		msg->data[0] == MP_UINT16 ||
		msg->data[0] == MP_UINT32 ||
		msg->data[0] == MP_UINT64)
	{
		type = MP_NUMERIC;
	}
	else if ((msg->data[0] >= MP_FIXSTR0 && msg->data[0] <= MP_FIXSTR31) || \
		msg->data[0] == MP_STR8 ||
		msg->data[0] == MP_STR16 ||
		msg->data[0] == MP_STR32)
	{
		type = MP_STRING;
	}
	else if (msg->data[0] == MP_BIN8 ||
		msg->data[0] == MP_BIN16 ||
		msg->data[0] == MP_BIN32)
	{
		type = MP_BINARY;
	}
	else if ((msg->data[0] >= MP_FIXARRAY0 && msg->data[0] <= MP_FIXARRAY15) || \
		msg->data[0] == MP_ARRAY16 ||
		msg->data[0] == MP_ARRAY32)
	{
		type = MP_ARRAY;
	}

	assert(type != -1);

	return type;
}

size_t mp_message_base_size(const uint8_t type)
{
	size_t base;

	if (type <= MP_FIXINT127 || \
		(type >= MP_FIXARRAY0 && type <= MP_FIXARRAY15))
	{
		base = 1;
	}
	else if (type == MP_BIN8 || \
		type == MP_STR8 || \
		type == MP_UINT8)
	{
		base = 2;
	}
	else if (type == MP_BIN16 || \
		type == MP_STR16 || \
		type == MP_UINT16 || \
		type == MP_ARRAY16)
	{
		base = 3;
	}
	else if (type == MP_BIN32 || \
		type == MP_STR32 || \
		type == MP_UINT32 || \
		type == MP_ARRAY32)
	{
		base = 5;
	}
	else if (type == MP_UINT64) {
		base = 9;
	}
	else if (type >= MP_FIXSTR0 && type <= MP_FIXSTR31) {
		base = type - MP_FIXSTR0 + 1;
	}
	else {
		base = 0;
	}

	return base;
}

size_t mp_message_full_size(struct mp_message *msg)
{
	size_t size = 0;

	if (msg->data[0] == MP_UINT8 || \
		msg->data[0] == MP_UINT16 || \
		msg->data[0] == MP_UINT32 || \
		msg->data[0] == MP_UINT64 || \
		msg->data[0] <= MP_FIXINT127 || \
		(msg->data[0] >= MP_FIXSTR0 && msg->data[0] <= MP_FIXSTR31) || \
		(msg->data[0] >= MP_FIXARRAY0 && msg->data[0] <= MP_FIXARRAY15) || \
		(msg->data[0] >= MP_ARRAY16 && msg->data[0] <= MP_ARRAY32))
	{
		size += mp_message_base_size(msg->data[0]);
	}
	else if (msg->data[0] == MP_BIN8 || msg->data[0] == MP_STR8)
	{
		size += 2;
		size += mp_read_number(msg->data + 1, 1);
	}
	else if (msg->data[0] == MP_BIN16 || msg->data[0] == MP_STR16)
	{
		size += 3;
		size += mp_read_number(msg->data + 1, 2);
	}
	else if (msg->data[0] == MP_BIN32 || msg->data[0] == MP_STR32)
	{
		size += 5;
		size += mp_read_number(msg->data + 1, 4);
	}
	else
	{
		size = 0;
	}

	return size;
}

void mp_message_uint(struct mp_message *msg, const uint64_t value)
{
	assert(msg);

	mp_message_init(msg);

	if (value <= 127) {
		mp_message_alloc(msg, 1);
		msg->data[0] = (uint8_t) (value & 0xff);
		return;
	}

	int len = mp_read_number_bytes(value);
	uint8_t type;

	switch (len) {
	case 1:
		type = MP_UINT8;
		break;
	case 2:
		type = MP_UINT16;
		break;
	case 4:
		type = MP_UINT32;
		break;
	default:
		type = MP_UINT64;
		break;
	}

	mp_message_alloc(msg, 1 + len);

	msg->data[0] = type;
	mp_write_number(value, msg->data + 1, len);
}

uint64_t mp_message_read_uint(struct mp_message *msg)
{
	assert(msg);
	assert(msg->data);

	size_t size = mp_message_full_size(msg);
	assert(size > 0);

	if (size == 1)
		return (uint64_t) msg->data[0];

	uint64_t value = mp_read_number(msg->data + 1, size - 1);

	return value;
}

void mp_message_str(struct mp_message *msg, const char *const str, const size_t size)
{
	assert(msg);
	assert(str);
	assert(size <= 0xffffffff);

	mp_message_init(msg);

	int lensize = 0;
	uint8_t type;

	if (size < 32) {
		type = MP_FIXSTR0 + size;
	} else {
		lensize = mp_read_number_bytes(size);

		switch(lensize) {
		case 1:
			type = MP_STR8;
			break;
		case 2:
			type = MP_STR16;
			break;
		default:
			type = MP_STR32;
			break;
		}
	}

	mp_message_alloc(msg, 1 + lensize + size);

	msg->data[0] = type;
	if (lensize > 0)
		mp_write_number(size, msg->data + 1, lensize);

	memcpy(msg->data + 1 + lensize, str, size);
}

char *mp_message_read_str(struct mp_message *msg, size_t *size)
{
	assert(msg);
	assert(msg->data);

	if (msg->data[0] >= MP_FIXSTR0 && msg->data[0] <= MP_FIXSTR31) {
		*size = msg->data[0] - MP_FIXSTR0;
		return (char *)msg->data + 1;
	}

	size_t start = mp_message_base_size(msg->data[0]);
	assert(start > 0);

	*size = msg->length - start;

	return (char *)msg->data + start;
}

void mp_message_bin(
	struct mp_message *msg,
	uint8_t *const data,
	const size_t size)
{
	assert(msg);
	assert(data);
	assert(size > 0 && size <= 0xffffffff);

	mp_message_init(msg);

	int lensize = mp_read_number_bytes(size);
	uint8_t type;

	switch(lensize) {
	case 1:
		type = MP_BIN8;
		break;
	case 2:
		type = MP_BIN16;
		break;
	default:
		type = MP_BIN32;
		break;
	}

	mp_message_alloc(msg, 1 + lensize + size);

	msg->data[0] = type;
	mp_write_number((uint64_t)size, msg->data + 1, lensize);
	memcpy(msg->data + 1 + lensize, data, size);
}

uint8_t *mp_message_read_bin(struct mp_message *msg, size_t *size)
{
	assert(msg);
	assert(msg->data);
	assert(size);

	size_t start = mp_message_base_size(msg->data[0]);
	assert(start > 0);

	*size = msg->length - start;

	return msg->data + start;
}

void mp_message_array(struct mp_message *msg, const size_t length)
{
	assert(msg);
	assert(length >= 1);
	assert(length <= 0xffffffff);

	mp_message_init(msg);

	if (length <= MP_FIXARRAY15) {
		mp_message_alloc(msg, 1);
		msg->data[0] = MP_FIXARRAY0 | length;
		return;
	}

	int len;
	uint8_t type;

	if (length <= 0xffff) {
		len = 2;
		type = MP_ARRAY16;
	} else {
		len = 4;
		type = MP_ARRAY32;
	}

	mp_message_alloc(msg, 1 + len);

	msg->data[0] = type;
	mp_write_number(length, msg->data + 1, len);
}

size_t mp_message_read_array_length(struct mp_message *msg)
{
	assert(msg);
	assert(msg->data);

	if (msg->data[0] >= MP_FIXARRAY0 && msg->data[0] <= MP_FIXARRAY15)
		return (size_t)(msg->data[0] - MP_FIXARRAY0);

	int bytes = msg->data[0] == MP_ARRAY16 ? 2 : 4;
	uint64_t length = mp_read_number(msg->data + 1, bytes);

	return length;
}

void mp_message_print(struct mp_message *msg, const int fd)
{
	size_t size;
	char *str;
	size_t i;
	uint64_t val;
	uint8_t *data;

	dprintf(fd, "{'length': '%lu', ", msg->length);

	if (msg->data[0] <= MP_FIXINT127 || \
		msg->data[0] == MP_UINT8 ||
		msg->data[0] == MP_UINT16 ||
		msg->data[0] == MP_UINT32 ||
		msg->data[0] == MP_UINT64)
	{
		val = mp_message_read_uint(msg);
		dprintf(fd, "'type': 'int', 'data': '%lu'", val);
	}
	else if ((msg->data[0] >= MP_FIXSTR0 && msg->data[0] <= MP_FIXSTR31) || \
		msg->data[0] == MP_STR8 ||
		msg->data[0] == MP_STR16 ||
		msg->data[0] == MP_STR32)
	{
		str = mp_message_read_str(msg, &size);
		dprintf(fd, "'type': 'string', 'data': '");
		for (i = 0; i < size; i++)
			dprintf(fd, "%c", str[i]);
		dprintf(fd, "'");
	}
	else if (msg->data[0] == MP_BIN8 ||
		msg->data[0] == MP_BIN16 ||
		msg->data[0] == MP_BIN32)
	{
		data = mp_message_read_bin(msg, &size);
		dprintf(fd, "'type': 'binary', 'data': '");
		for (i = 0; i < size; i++)
			dprintf(fd, "0x%x ", data[i]);
		dprintf(fd, "'");
	}
	else if ((msg->data[0] >= MP_FIXARRAY0 && msg->data[0] <= MP_FIXARRAY15) || \
		msg->data[0] == MP_ARRAY16 ||
		msg->data[0] == MP_ARRAY32)
	{
		size = mp_message_read_array_length(msg);
		dprintf(fd, "'type': 'array', 'data': '%lu'", size);
	}
	else
	{
		dprintf(fd, "'type': 'unkown', 'data': '");
		for (i = 0; i < msg->length; i++)
			dprintf(fd, "0x%x ", msg->data[i]);
		dprintf(fd, "'");
	}

	dprintf(fd, "}\n");
}
