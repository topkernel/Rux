// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * Copyright (c) 2022 Richard Palethorpe <rpalethorpe@suse.com>
 * Copyright (c) 2023 Andrea Cervesato <andrea.cervesato@suse.com>
 */

#include <unistd.h>
#include <stdio.h>
#include "ltx.h"

int main(void)
{
	struct ltx_session *session;

	session = ltx_session_init(STDIN_FILENO, STDOUT_FILENO);
	ltx_start_event_loop(session);
	ltx_session_destroy(session);

	return 0;
}
