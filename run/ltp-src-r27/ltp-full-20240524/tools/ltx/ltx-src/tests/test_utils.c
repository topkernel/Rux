// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * Copyright (c) 2023 Andrea Cervesato <andrea.cervesato@suse.com>
 */

#include <check.h>
#include <stdio.h>
#include "utils.h"

START_TEST(test_mp_big_endian)
{
	uint64_t exp_num = 11206657;

	uint8_t data[4];
	mp_write_number(exp_num, data, 4);

	uint8_t exp_data[4] = { 0x00, 0xab, 0x00, 0x01 };
	ck_assert_mem_eq(data, exp_data, 4);

	uint64_t val = mp_read_number(data, 4);
	ck_assert_double_eq(exp_num, val);
}
END_TEST

START_TEST(test_mp_write_read)
{
	int len;
	uint64_t number;
	uint8_t data[8];

	for (uint64_t value = 0; value < 0xffff; value++) {
		memset(data, 0, 8);
		len = mp_read_number_bytes(value);

		mp_write_number(value, data, len);
		number = mp_read_number(data, len);

		ck_assert_uint_eq(value, number);
	}
}
END_TEST

Suite *msgpack_utils_suite(void)
{
	Suite *s;
	TCase *tc;

	s = suite_create("msgpack_utils");

	tc = tcase_create("test_mp_big_endian");
	tcase_add_test(tc, test_mp_big_endian);
	suite_add_tcase(s, tc);

	tc = tcase_create("test_mp_write_read");
	tcase_set_timeout(tc, 180);
	tcase_add_test(tc, test_mp_write_read);
	suite_add_tcase(s, tc);

	return s;
}

int main(void)
{
	int number_failed;
	Suite *s;
	SRunner *sr;

	s = msgpack_utils_suite();
	sr = srunner_create(s);

	srunner_run_all(sr, CK_NORMAL);
	number_failed = srunner_ntests_failed(sr);
	srunner_free(sr);

	return number_failed != 0;
}
