// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * Copyright (c) 2022 Richard Palethorpe <rpalethorpe@suse.com>
 * Copyright (c) 2023 Andrea Cervesato <andrea.cervesato@suse.com>
 */

#ifndef LTX_H
#define LTX_H

/* The current LTX version */
#define VERSION "0.1"

/* ltx supported messages */
enum
{
	LTX_NONE = 0xffff,
	LTX_WARNING = 0xfff,
	LTX_ERROR = 0xff,
	LTX_VERSION = 0x00,
	LTX_PING = 0x01,
	LTX_PONG = 0x02,
	LTX_GET_FILE = 0x03,
	LTX_SET_FILE = 0x04,
	LTX_ENV = 0x05,
	LTX_CWD = 0x06,
	LTX_EXEC = 0x07,
	LTX_RESULT = 0x08,
	LTX_LOG = 0x09,
	LTX_DATA = 0xa0,
	LTX_KILL = 0xa1,
};

/* ltx session abstract object to implement */
typedef struct ltx_session ltx_session;

/* Initialize ltx session with specific stdin/stdout */
struct ltx_session *ltx_session_init(const int stdin_fd, const int stdout_fd);

/* Stop current ltx session */
void ltx_session_stop(struct ltx_session *session);

/* Destroy current ltx session */
void ltx_session_destroy(struct ltx_session *session);

/* Start the main event loop */
void ltx_start_event_loop(struct ltx_session *session);

/* Set the debug file descriptor. STDERR_FILENO is used by default */
void ltx_set_debug_fd(struct ltx_session *session, const int fd);

/* Print a warning message on debug file descriptor */
void ltx_warning(struct ltx_session *session, const char *msg);

#endif /* LTX_H */
