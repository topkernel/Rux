// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * Copyright (c) 2023 Andrea Cervesato <andrea.cervesato@suse.com>
 */

#define _GNU_SOURCE

#include <check.h>
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
#include <fcntl.h>
#include <sys/mman.h>
#include "message.h"
#include "utils.h"

START_TEST(test_mp_message_uint8)
{
	struct mp_message msg;

	mp_message_init(&msg);
	mp_message_uint(&msg, 0xff);

	ck_assert_ptr_nonnull(msg.data);
	ck_assert_mem_eq(msg.data, ((uint8_t []){MP_UINT8, 0xff}), 2);

	mp_message_destroy(&msg);
}
END_TEST

START_TEST(test_mp_message_uint16)
{
	struct mp_message msg;

	mp_message_init(&msg);
	mp_message_uint(&msg, 0xffff);

	ck_assert_ptr_nonnull(msg.data);
	ck_assert_mem_eq(msg.data, ((uint8_t []){
		MP_UINT16,
		0xff, 0xff,
	}), 3);

	mp_message_destroy(&msg);
}
END_TEST

START_TEST(test_mp_message_uint32)
{
	struct mp_message msg;

	mp_message_init(&msg);
	mp_message_uint(&msg, 0xffffffff);

	ck_assert_ptr_nonnull(msg.data);
	ck_assert_mem_eq(msg.data, ((uint8_t []){
		MP_UINT32,
		0xff, 0xff, 0xff, 0xff,
	}), 5);

	mp_message_destroy(&msg);
}
END_TEST

START_TEST(test_mp_message_uint64)
{
	struct mp_message msg;

	mp_message_init(&msg);
	mp_message_uint(&msg, 0xffffffffffffffff);

	ck_assert_ptr_nonnull(msg.data);
	ck_assert_mem_eq(msg.data, ((uint8_t []){
		MP_UINT64,
		0xff, 0xff, 0xff, 0xff,
		0xff, 0xff, 0xff, 0xff,
	}), 9);

	mp_message_destroy(&msg);
}
END_TEST

START_TEST(test_mp_message_read_uint)
{
	struct mp_message msg;
	uint64_t value = 0xffffffffffffffff;

	mp_message_init(&msg);
	mp_message_uint(&msg, value);

	ck_assert_ptr_nonnull(msg.data);
	ck_assert_uint_eq(mp_message_type(&msg), MP_NUMERIC);
	ck_assert_uint_eq(mp_message_read_uint(&msg), value);

	mp_message_destroy(&msg);
}
END_TEST

void test_mp_message_str(size_t len)
{
	char *data = (char*) malloc(len);
	memset(data, 'a', len);

	int length_bytes = mp_read_number_bytes(len);
	int type;

	switch (length_bytes) {
	case 1:
		type = MP_STR8;
		break;
	case 2:
		type = MP_STR16;
		break;
	case 4:
		type = MP_STR32;
		break;
	default:
		break;
	}

	uint8_t length[length_bytes];
	mp_write_number(len, length, length_bytes);

	struct mp_message msg;
	mp_message_str(&msg, data, len);

	ck_assert_ptr_nonnull(msg.data);
	ck_assert_uint_eq(msg.data[0], type);
	ck_assert_mem_eq(msg.data + 1, length, length_bytes);
	ck_assert_mem_eq(msg.data + 1 + length_bytes, data, len);

	mp_message_destroy(&msg);
}

START_TEST(test_mp_message_fixstr)
{
	struct mp_message msg;
	char data[] = {'c', 'i', 'a', 'o'};

	mp_message_init(&msg);
	mp_message_str(&msg, data, 4);

	ck_assert_ptr_nonnull(msg.data);
	ck_assert_uint_eq(msg.data[0], MP_FIXSTR0 + 4);
	ck_assert_mem_eq(msg.data + 1, data, 4);

	mp_message_destroy(&msg);
}
END_TEST

START_TEST(test_mp_message_str8)
{
	test_mp_message_str(0xff);
}
END_TEST

START_TEST(test_mp_message_str16)
{
	test_mp_message_str(0xfff);
}
END_TEST

START_TEST(test_mp_message_str32)
{
	test_mp_message_str(0xfffff);
}
END_TEST

START_TEST(test_mp_message_read_str)
{
	struct mp_message msg;
	char data[] = {'c', 'i', 'a', 'o'};
	char *pos;

	mp_message_init(&msg);
	mp_message_str(&msg, data, 4);

	size_t size;
	pos = mp_message_read_str(&msg, &size);

	ck_assert_ptr_nonnull(msg.data);
	ck_assert_mem_eq(pos, data, 4);
	ck_assert_double_eq(size, 4);

	mp_message_destroy(&msg);
}
END_TEST

void test_mp_message_bin(const size_t len)
{
	uint8_t *data;

	data = (uint8_t *)malloc(len);
	memset(data, 'x', len);

	int length_bytes = mp_read_number_bytes(len);
	int type;

	switch (length_bytes) {
	case 1:
		type = MP_BIN8;
		break;
	case 2:
		type = MP_BIN16;
		break;
	case 4:
		type = MP_BIN32;
		break;
	default:
		break;
	}

	uint8_t length[length_bytes];
	mp_write_number(len, length, length_bytes);

	struct mp_message msg;
	mp_message_bin(&msg, data, len);

	ck_assert_ptr_nonnull(msg.data);
	ck_assert_uint_eq(msg.data[0], type);
	ck_assert_mem_eq(msg.data + 1, length, length_bytes);
	ck_assert_mem_eq(msg.data + 1 + length_bytes, data, len);

	free(data);
	mp_message_destroy(&msg);
}

START_TEST(test_mp_message_bin8)
{
	test_mp_message_bin(0xff);
}
END_TEST

START_TEST(test_mp_message_bin16)
{
	test_mp_message_bin(0xfff);
}
END_TEST

START_TEST(test_mp_message_bin32)
{
	test_mp_message_bin(0xfffff);
}
END_TEST

START_TEST(test_mp_message_read_bin)
{
	struct mp_message msg;
	uint8_t data[] = {0x55, 0x55, 0x55, 0x55, 0x55};
	uint8_t *pos;
	size_t size;

	mp_message_init(&msg);
	mp_message_bin(&msg, data, 5);

	pos = mp_message_read_bin(&msg, &size);

	ck_assert_ptr_nonnull(msg.data);
	ck_assert_uint_eq(size, 5);
	ck_assert_mem_eq(pos, data, size);

	mp_message_destroy(&msg);
}
END_TEST

static int write_message_and_read(struct mp_message *msg, uint8_t *buf, size_t buf_size)
{
	int fd = memfd_create("msgpack_test", 0);
	int num;

	ck_assert_int_ne(fd, -1);
	ck_assert_int_ne(ftruncate(fd, buf_size), -1);

	write(fd, msg->data, msg->length);

	lseek(fd, 0, SEEK_SET);
	fsync(fd);

	num = read(fd, buf, buf_size);
	close(fd);

	return num;
}

void test_mp_write_uint(uint64_t number)
{
	int num_bytes = mp_read_number_bytes(number);
	int type;

	switch (num_bytes) {
	case 1:
		type = MP_UINT8;
		break;
	case 2:
		type = MP_UINT16;
		break;
	case 4:
		type = MP_UINT32;
		break;
	case 8:
		type = MP_UINT64;
		break;
	default:
		break;
	}

	uint8_t num_data[num_bytes];
	mp_write_number(number, num_data, num_bytes);;

	struct mp_message msg;
	mp_message_uint(&msg, number);

	ck_assert_ptr_nonnull(msg.data);
	ck_assert_uint_eq(msg.data[0], type);
	ck_assert_mem_eq(msg.data + 1, num_data, num_bytes);

	mp_message_destroy(&msg);
}

START_TEST(test_mp_write_uint8)
{
	test_mp_write_uint(0xff);
}
END_TEST

START_TEST(test_mp_write_uint16)
{
	test_mp_write_uint(0xffff);
}
END_TEST

START_TEST(test_mp_write_uint32)
{
	test_mp_write_uint(0xffffffff);
}
END_TEST

START_TEST(test_mp_write_uint64)
{
	test_mp_write_uint(0xffffffffffffffff);
}
END_TEST

static void test_mp_write_data(uint8_t type, size_t data_size, int binary)
{
	struct mp_message msg;
	size_t buf_size;
	uint8_t *buf;
	int pos = 0;
	char *str;
	int num;

	buf_size = data_size + 6;

	buf = (uint8_t *)malloc(sizeof(uint8_t) * buf_size);
	memset(buf, 0, buf_size);

	str = (char *)malloc(sizeof(char) * data_size);
	memset(str, 'a', data_size);

	mp_message_init(&msg);

	if (binary)
		mp_message_bin(&msg, (uint8_t *)str, data_size);
	else
		mp_message_str(&msg, str, data_size);

	num = write_message_and_read(&msg, buf, buf_size);

	ck_assert_int_ne(num, -1);

	switch (type) {
	case MP_FIXSTR0:
		ck_assert_uint_eq(buf[0], type + data_size);
		pos = 1;
		break;
	case MP_STR8:
	case MP_BIN8:
		ck_assert_uint_eq(buf[0], type);
		ck_assert_uint_eq(mp_read_number(buf + 1, 1), data_size);
		pos = 2;
		break;
	case MP_STR16:
	case MP_BIN16:
		ck_assert_uint_eq(buf[0], type);
		ck_assert_uint_eq(mp_read_number(buf + 1, 2), data_size);
		pos = 3;
		break;
	case MP_STR32:
	case MP_BIN32:
		ck_assert_uint_eq(buf[0], type);
		ck_assert_uint_eq(mp_read_number(buf + 1, 4), data_size);
		pos = 5;
		break;
	default:
		break;
	}

	ck_assert_mem_eq((buf + pos), str, data_size);

	free(str);
	mp_message_destroy(&msg);
}

START_TEST(test_mp_write_fixstr)
{
	test_mp_write_data(MP_FIXSTR0, 31, 0);
}
END_TEST

START_TEST(test_mp_write_str8)
{
	test_mp_write_data(MP_STR8, 0xff, 0);
}
END_TEST

START_TEST(test_mp_write_str16)
{
	test_mp_write_data(MP_STR16, 0xffff, 0);
}
END_TEST

START_TEST(test_mp_write_str32)
{
	test_mp_write_data(MP_STR32, 0xfffffff, 0);
}
END_TEST

START_TEST(test_mp_write_bin8)
{
	test_mp_write_data(MP_BIN8, 0xff, 1);
}
END_TEST

START_TEST(test_mp_write_bin16)
{
	test_mp_write_data(MP_BIN16, 0xffff, 1);
}
END_TEST

START_TEST(test_mp_write_bin32)
{
	test_mp_write_data(MP_BIN32, 0xfffffff, 1);
}
END_TEST

START_TEST(test_mp_message_fixarray)
{
	struct mp_message msg;

	mp_message_init(&msg);

	for (size_t i = 1; i < 16; i++) {
		mp_message_array(&msg, i);

		ck_assert_ptr_nonnull(msg.data);
		ck_assert_double_eq(msg.data[0], MP_FIXARRAY0 + i);
	}

	mp_message_destroy(&msg);
}
END_TEST

START_TEST(test_mp_message_array16)
{
	struct mp_message msg;
	uint8_t data[2];
	uint64_t val = 0xfffa;

	mp_message_init(&msg);
	mp_message_array(&msg, val);

	ck_assert_ptr_nonnull(msg.data);

	mp_write_number(val, data, 2);
	ck_assert_mem_eq(((uint8_t []) { 0xff, 0xfa }), data, 2);

	mp_message_destroy(&msg);
}
END_TEST

START_TEST(test_mp_message_array32)
{
	struct mp_message msg;
	uint8_t data[4];
	uint64_t val = 0xfffffafa;

	mp_message_init(&msg);
	mp_message_array(&msg, val);

	ck_assert_ptr_nonnull(msg.data);

	mp_write_number(val, data, 4);
	ck_assert_mem_eq(((uint8_t []) { 0xff, 0xff, 0xfa, 0xfa }), data, 4);

	mp_message_destroy(&msg);
}
END_TEST

Suite *msgpack_message_suite(void)
{
	Suite *s;
	TCase *tc;

	s = suite_create("msgpack_message");

	/* message initialisation tests. 32 bits length is not tested */
	tc = tcase_create("test_mp_message_uint8");
	tcase_add_test(tc, test_mp_message_uint8);
	suite_add_tcase(s, tc);

	tc = tcase_create("test_mp_message_uint16");
	tcase_add_test(tc, test_mp_message_uint16);
	suite_add_tcase(s, tc);

	tc = tcase_create("test_mp_message_uint32");
	tcase_add_test(tc, test_mp_message_uint32);
	suite_add_tcase(s, tc);

	tc = tcase_create("test_mp_message_uint64");
	tcase_add_test(tc, test_mp_message_uint64);
	suite_add_tcase(s, tc);

	tc = tcase_create("test_mp_message_read_uint");
	tcase_add_test(tc, test_mp_message_read_uint);
	suite_add_tcase(s, tc);

	tc = tcase_create("test_mp_message_fixstr");
	tcase_add_test(tc, test_mp_message_fixstr);
	suite_add_tcase(s, tc);

	tc = tcase_create("test_mp_message_str8");
	tcase_add_test(tc, test_mp_message_str8);
	suite_add_tcase(s, tc);

	tc = tcase_create("test_mp_message_str16");
	tcase_add_test(tc, test_mp_message_str16);
	suite_add_tcase(s, tc);

	tc = tcase_create("test_mp_message_str32");
	tcase_add_test(tc, test_mp_message_str32);
	suite_add_tcase(s, tc);

	tc = tcase_create("test_mp_message_read_str");
	tcase_add_test(tc, test_mp_message_read_str);
	suite_add_tcase(s, tc);

	tc = tcase_create("test_mp_message_bin8");
	tcase_add_test(tc, test_mp_message_bin8);
	suite_add_tcase(s, tc);

	tc = tcase_create("test_mp_message_bin16");
	tcase_add_test(tc, test_mp_message_bin16);
	suite_add_tcase(s, tc);

	tc = tcase_create("test_mp_message_bin32");
	tcase_add_test(tc, test_mp_message_bin32);
	suite_add_tcase(s, tc);

	tc = tcase_create("test_mp_message_read_bin");
	tcase_add_test(tc, test_mp_message_read_bin);
	suite_add_tcase(s, tc);

	/* message writing tests */
	tc = tcase_create("test_mp_write_uint8");
	tcase_add_test(tc, test_mp_write_uint8);
	suite_add_tcase(s, tc);

	tc = tcase_create("test_mp_write_uint16");
	tcase_add_test(tc, test_mp_write_uint16);
	suite_add_tcase(s, tc);

	tc = tcase_create("test_mp_write_uint32");
	tcase_add_test(tc, test_mp_write_uint32);
	suite_add_tcase(s, tc);

	tc = tcase_create("test_mp_write_uint64");
	tcase_add_test(tc, test_mp_write_uint64);
	suite_add_tcase(s, tc);

	tc = tcase_create("test_mp_write_fixstr");
	tcase_add_test(tc, test_mp_write_fixstr);
	suite_add_tcase(s, tc);

	tc = tcase_create("test_mp_write_str8");
	tcase_add_test(tc, test_mp_write_str8);
	suite_add_tcase(s, tc);

	tc = tcase_create("test_mp_write_str16");
	tcase_add_test(tc, test_mp_write_str16);
	suite_add_tcase(s, tc);

	tc = tcase_create("test_mp_write_str32");
	tcase_add_test(tc, test_mp_write_str32);
	suite_add_tcase(s, tc);

	tc = tcase_create("test_mp_write_bin8");
	tcase_add_test(tc, test_mp_write_bin8);
	suite_add_tcase(s, tc);

	tc = tcase_create("test_mp_write_bin16");
	tcase_add_test(tc, test_mp_write_bin16);
	suite_add_tcase(s, tc);

	tc = tcase_create("test_mp_write_bin32");
	tcase_add_test(tc, test_mp_write_bin32);
	suite_add_tcase(s, tc);

	tc = tcase_create("test_mp_message_fixarray");
	tcase_add_test(tc, test_mp_message_fixarray);
	suite_add_tcase(s, tc);

	tc = tcase_create("test_mp_message_array16");
	tcase_add_test(tc, test_mp_message_array16);
	suite_add_tcase(s, tc);

	tc = tcase_create("test_mp_message_array32");
	tcase_add_test(tc, test_mp_message_array32);
	suite_add_tcase(s, tc);

	return s;
}

int main(void)
{
	int number_failed;
	Suite *s;
	SRunner *sr;

	s = msgpack_message_suite();
	sr = srunner_create(s);

	srunner_run_all(sr, CK_NORMAL);
	number_failed = srunner_ntests_failed(sr);
	srunner_free(sr);

	return number_failed != 0;
}
