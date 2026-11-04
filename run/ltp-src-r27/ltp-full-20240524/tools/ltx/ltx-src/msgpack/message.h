// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * Copyright (c) 2022 Richard Palethorpe <rpalethorpe@suse.com>
 * Copyright (c) 2023 Andrea Cervesato <andrea.cervesato@suse.com>
 */

#ifndef MSGPACK_MESSAGE_H
#define MSGPACK_MESSAGE_H

#include <stdio.h>
#include <stdint.h>
#include <stddef.h>

/* A generic msgpack message. */
struct mp_message
{
	uint8_t *data;
	size_t length;
	uint8_t reserved;
};

/* msgpack supported formats (small set of all) */
enum
{
	MP_FIXINT0 = 0x00,
	MP_FIXINT127 = 0x7f,
	MP_FIXSTR0 = 0xa0,
	MP_FIXSTR31 = 0xbf,
	MP_BIN8 = 0xc4,
	MP_BIN16 = 0xc5,
	MP_BIN32 = 0xc6,
	MP_UINT8 = 0xcc,
	MP_UINT16 = 0xcd,
	MP_UINT32 = 0xce,
	MP_UINT64 = 0xcf,
	MP_STR8 = 0xd9,
	MP_STR16 = 0xda,
	MP_STR32 = 0xdb,
	MP_FIXARRAY0 = 0x90,
	MP_FIXARRAY15 = 0x9f,
	MP_ARRAY16 = 0xdc,
	MP_ARRAY32 = 0xdd,
};

/* Type sets for msgpack types */
enum
{
	MP_NUMERIC,
	MP_STRING,
	MP_BINARY,
	MP_ARRAY
};

/* Initialize message. */
void mp_message_init(struct mp_message *msg);

/* Release memory of the message. */
void mp_message_destroy(struct mp_message *msg);

/* Return the message type. It can be:
 * - MP_NUMERIC
 * - MP_STRING
 * - MP_BINARY
 * - MP_ARRAY
 * Useful when we need to know data type before processing.
 */
int mp_message_type(struct mp_message *msg);

/* Return size of message data.
 * In some cases we don't have enough information to know expected data
 * size (str/bin), so we just return number of bytes we need to save data
 * length information.
 */
size_t mp_message_base_size(const uint8_t type);

/* Return total amount of data expected to save message, according with
 * both type and length information.
 */
size_t mp_message_full_size(struct mp_message *msg);

/* Create a message storing unsigned integer. */
void mp_message_uint(struct mp_message *msg, const uint64_t value);

/* Return unsigned integer stored inside message. */
uint64_t mp_message_read_uint(struct mp_message *msg);

/* Create a message storing a string. */
void mp_message_str(struct mp_message *msg, const char *const str, const size_t size);

/* Return pointer to the first character of the string. */
char *mp_message_read_str(struct mp_message *msg, size_t *size);

/* Create a message storing binary data. */
void mp_message_bin(struct mp_message *msg, uint8_t *const data, const size_t size);

/* Return pointer to the first element of data. */
uint8_t *mp_message_read_bin(struct mp_message *msg, size_t *size);

/* Create an array of length. */
void mp_message_array(struct mp_message *msg, const size_t length);

/* Return number of elements inside the array. */
size_t mp_message_read_array_length(struct mp_message *msg);

/* Print message inside fd in JSON format */
void mp_message_print(struct mp_message *msg, const int fd);

#endif /* MSGPACK_MESSAGE_H */
