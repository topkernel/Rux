// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * Copyright (c) 2022 Richard Palethorpe <rpalethorpe@suse.com>
 * Copyright (c) 2023 Andrea Cervesato <andrea.cervesato@suse.com>
 */

#define _GNU_SOURCE /* CLOCK_MONOTONIC */

#include <time.h>
#include <fcntl.h>
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
#include <assert.h>
#include <string.h>
#include <signal.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <sys/mman.h>
#include <sys/epoll.h>
#include <sys/signalfd.h>
#include <sys/statfs.h>
#include <linux/magic.h>
#include <linux/limits.h> /* PATH_MAX */

#include "ltx.h"
#include "msgpack/msgpack.h"

/* maximum number of epoll events */
#define MAX_EVENTS 128

/* maximum number of msgpack messages per ltx message */
#define MAX_MESSAGES 5

/* set when no slots are assigned */
#define SLOT_NONE 0xffff

/* maximum number of slots to execute commands */
#define MAX_SLOTS 128

/* used when we want to assign same evn/cwd to all slots */
#define ALL_SLOTS MAX_SLOTS

/* maximum number of environment variables */
#define MAX_ENVS 16

/* number of bytes for read(..) */
#define READ_BUFFER_SIZE 1024

/* maximum number char per string */
#define MAX_STRING_LEN 4096

/* Macro used to print messages */
#ifdef DEBUG
	#define LTX_PRINT_MESSAGE(x) mp_message_print(x, session->debug_fd)
#else
	#define LTX_PRINT_MESSAGE(x) {}
#endif

struct ltx_message
{
	/* type of ltx message (LTX_PING, LTX_ERROR, etc. )*/
	uint32_t type;

	/* current unpacking data position */
	size_t curr;

	/* expected number of msgpack messages */
	size_t length;

	/* messages which are building the ltx message */
	struct mp_message data[MAX_MESSAGES];
};

struct ltx_env
{
	/* environment key */
	char key[MAX_STRING_LEN];

	/* environment value */
	char value[MAX_STRING_LEN];
};

enum ltx_event_type
{
	/* received on stdin event */
	LTX_EVT_STDIN = 0xa0,

	/* received on stdout event */
	LTX_EVT_STDOUT,
};

struct ltx_event
{
	/* LTX event type */
	enum ltx_event_type type;

	/* slot id of the relative event */
	uint64_t slot_id;

	/* file descriptor associated with event */
	int fd;
};

struct ltx_slot
{
	/* used if 1, 0 otherwise */
	int used;

	/* process id */
	pid_t pid;

	/* environment variables */
	struct ltx_env env[MAX_ENVS];

	/* current working directory */
	char cwd[PATH_MAX];

	/* stdin buffer */
	uint8_t buffer[READ_BUFFER_SIZE];

	/* ltx event associated with this object */
	struct ltx_event event;
};

struct ltx_table
{
	/* executions slots */
	struct ltx_slot slots[MAX_SLOTS];

	/* global environment variables */
	struct ltx_env env[MAX_ENVS];
};

struct ltx_session
{
	/* current application PID */
	pid_t pid;

	/* stdin file descriptor */
	int stdin_fd;

	/* stdout file descriptor */
	int stdout_fd;

	/* debug file descriptor */
	int debug_fd;

	/* epoll file descriptor */
	int epoll_fd;

	/* application stdin buffer */
	uint8_t stdin_buffer[READ_BUFFER_SIZE];

	/* msgpack messages unpacker */
	struct mp_unpacker msg_unpacker;

	/* current ltx unpacking message */
	struct ltx_message ltx_message;

	/* current ltx execution table */
	struct ltx_table table;
};

/* stores information about error position inside the code */
struct ltx_error_pos
{
	const char *const file;
	const char *const func;
	const int line;
};

#define LTX_HANDLE_ERROR(session, str, show_errno) \
	ltx_handle_error( \
		((struct ltx_error_pos){ __FILE__, __func__, __LINE__ }), \
		session, \
		str, \
		show_errno)

/* it's 1 when we want to stop the event loop. zero otherwise */
static volatile int stop_loop = 0;

/* it's 1 when stdin/stdout pipes are broken. zero otherwise */
static volatile int broken_pipe = 0;

static void ltx_message_reserve_next(struct ltx_session *session)
{
	++(session->ltx_message.curr);
	assert(session->ltx_message.curr < MAX_MESSAGES);

	mp_unpacker_reserve(
		&session->msg_unpacker,
		session->ltx_message.data + session->ltx_message.curr);
}

static void ltx_message_reset(struct ltx_session *session)
{
	session->ltx_message.type = LTX_NONE;
	session->ltx_message.curr = 0;
	session->ltx_message.length = 0;

	mp_unpacker_reserve(
		&session->msg_unpacker,
		session->ltx_message.data);
}

static void ltx_send_message(
	struct ltx_session *session,
	struct mp_message *msg)
{
	LTX_PRINT_MESSAGE(msg);

	ssize_t pos = 0, ret;
	do {
		ret = write(session->stdout_fd, msg->data, msg->length);
		if (ret == -1)
			break;

		pos += ret;
	} while (pos < (ssize_t)msg->length);
}

static void ltx_send_messages(
	struct ltx_session *session,
	struct mp_message *const msgs,
	const int count)
{
	struct mp_message msg;

	mp_message_array(&msg, count);
	ltx_send_message(session, &msg);

	for (int i = 0; i < count; i++)
		ltx_send_message(session, msgs + i);
}

static void ltx_echo(struct ltx_session *session)
{
	ltx_send_messages(
		session,
		session->ltx_message.data,
		session->ltx_message.curr + 1);

	ltx_message_reset(session);
}

static void ltx_handle_error(
	struct ltx_error_pos pos,
	struct ltx_session *session,
	const char *const str,
	const int show_errno)
{
	assert(str);

	struct mp_message msgs[2];

	mp_message_uint(&msgs[0], LTX_ERROR);

	char* msg;
	int ret;

	if (show_errno) {
		ret = asprintf(&msg, "%s:%s:%d %s (%s)",
			pos.file,
			pos.func,
			pos.line,
			str,
			strerror(errno));
		assert(ret >= 0);
	} else {
		ret = asprintf(&msg, "%s:%s:%d %s",
			pos.file,
			pos.func,
			pos.line,
			str);
		assert(ret >= 0);
	}

	mp_message_str(&msgs[1], msg, (size_t)ret);

	ltx_send_messages(session, msgs, 2);
	ltx_message_reset(session);

	if (msg)
		free(msg);
}

static void ltx_read_string(
	struct ltx_session *session,
	struct mp_message *msg,
	char *str)
{
	size_t size;
	char *ptr;

	ptr = mp_message_read_str(msg, &size);

	if (size > MAX_STRING_LEN) {
		LTX_HANDLE_ERROR(session, "Maximum string length is 4096", 0);
		return;
	}

	strncpy(str, ptr, size);
	str[size] = '\0';
}

static void ltx_handle_version(struct ltx_session *session)
{
	struct mp_message msgs[2];

	mp_message_uint(&msgs[0], LTX_VERSION);
	mp_message_str(&msgs[1], VERSION, strlen(VERSION));

	ltx_send_messages(session, msgs, 2);
	ltx_message_reset(session);
}

static uint64_t ltx_gettime(void)
{
	struct timespec ts;
	clock_gettime(CLOCK_MONOTONIC, &ts);

	return ts.tv_sec * 1000000000 + ts.tv_nsec;
}

static void ltx_handle_ping(struct ltx_session *session)
{
	ltx_echo(session);

	struct mp_message msgs[2];
	struct timespec ts;

	clock_gettime(CLOCK_MONOTONIC, &ts);

	mp_message_uint(&msgs[0], LTX_PONG);
	mp_message_uint(&msgs[1], ltx_gettime());

	ltx_send_messages(session, msgs, 2);
	ltx_message_reset(session);
}

static int ltx_file_from_proc(struct ltx_session *session, const int fd)
{
	struct statfs fs;

	if (fstatfs(fd, &fs) == -1) {
		LTX_HANDLE_ERROR(session, "fstatfs() error", 1);
		return -1;
	}

	int is_proc = (fs.f_type == PROC_SUPER_MAGIC) ? 1 : 0;

	return is_proc;
}

static void ltx_handle_get_file(struct ltx_session *session)
{
	char path[MAX_STRING_LEN];
	ltx_read_string(session, session->ltx_message.data + 1, path);

	if (path[0] == '\0') {
		LTX_HANDLE_ERROR(session, "Empty given path", 0);
		return;
	}

	int fd = open(path, O_RDONLY);
	if (fd == -1) {
		LTX_HANDLE_ERROR(session, "open() error", 1);
		return;
	}

	int from_proc = ltx_file_from_proc(session, fd);
	if (from_proc == -1)
		return;

	struct stat st;
	if (fstat(fd, &st) == -1) {
		LTX_HANDLE_ERROR(session, "fstat() error", 1);
		return;
	}

	if (!S_ISREG(st.st_mode)) {
		LTX_HANDLE_ERROR(session, "Given path is not a file", 0);
		return;
	}

	if (st.st_size >= 0x7ffff000) {
		LTX_HANDLE_ERROR(session, "File is too large", 0);
		return;
	}

	struct mp_message msgs[2];
	size_t nread;

	if (from_proc) {
		/* read /proc files has zero length. We need to use getline() */
		FILE *stream = fdopen(fd, "r");
		if (!stream) {
			LTX_HANDLE_ERROR(session, "fdopen() error", 1);
			return;
		};

		char *line = NULL;
		while ((nread = (size_t)getline(&line, &nread, stream)) != (size_t)-1) {
			mp_message_uint(&msgs[0], LTX_DATA);
			mp_message_bin(&msgs[1], (uint8_t *)line, nread);

			ltx_send_messages(session, msgs, 2);
		}

		if (line)
			free(line);

		if (!feof(stream)) {
			LTX_HANDLE_ERROR(session, "getline() error", 1);
			return;
		}
	} else {
		/* regular files can use read() */
		uint8_t data[READ_BUFFER_SIZE];
		ssize_t pos = 0, nread;

		do {
			nread = read(fd, data, READ_BUFFER_SIZE);
			if (nread == -1) {
				LTX_HANDLE_ERROR(session, "read() error", 1);
				return;
			}
			pos += nread;

			mp_message_uint(&msgs[0], LTX_DATA);
			mp_message_bin(&msgs[1], data, nread);

			ltx_send_messages(session, msgs, 2);
		} while (pos < st.st_size);
	}

	if (close(fd) == -1) {
		LTX_HANDLE_ERROR(session, "close() error", 1);
		return;
	}

	mp_message_uint(&msgs[0], LTX_GET_FILE);
	mp_message_str(&msgs[1], path, strlen(path));

	ltx_send_messages(session, msgs, 2);
	ltx_message_reset(session);
}

static void ltx_handle_set_file(struct ltx_session *session)
{
	char path[MAX_STRING_LEN];
	ltx_read_string(session, session->ltx_message.data + 1, path);

	size_t size;
	void *data = mp_message_read_bin(session->ltx_message.data + 2, &size);

	if (path[0] == '\0') {
		LTX_HANDLE_ERROR(session, "Empty given path", 0);
		return;
	}

	int fd = open(path, O_RDWR | O_CREAT | O_TRUNC, 0600);
	if (fd == -1) {
		LTX_HANDLE_ERROR(session, "open() error", 1);
		return;
	}

	ssize_t pos = 0, ret;
	do {
		ret = write(fd, data, size);
		if (ret == -1) {
			LTX_HANDLE_ERROR(session, "write() error", 1);
			return;
		}
		pos += ret;
	} while (pos < (ssize_t)size);

	close(fd);

	struct mp_message msgs[2];

	mp_message_uint(&msgs[0], LTX_SET_FILE);
	mp_message_str(&msgs[1], path, strlen(path));

	ltx_send_messages(session, msgs, 2);
	ltx_message_reset(session);
}

static int ltx_env_set(
	struct ltx_session *session,
	struct ltx_env *exec_env,
	const char *key,
	const char *value)
{
	assert(exec_env);
	assert(key);
	assert(value);

	struct ltx_env *env;
	unsigned i = 0;

	if (strlen(value) > 0) {
		for (i = 0; i < MAX_ENVS; i++) {
			env = exec_env + i;
			if (!strcmp(env->key, key) || !strlen(env->key))
				break;
		}

		if (i >= MAX_ENVS) {
			LTX_HANDLE_ERROR(
				session,
				"Set too many environment variables", 0);
			return 1;
		}
	} else {
		for (i = 0; i < MAX_ENVS; i++) {
			env = exec_env + i;
			if (!strcmp(env->key, key))
				break;
		}

		if (i >= MAX_ENVS)
			env = NULL;
	}

	if (env) {
		strcpy(env->key, key);
		strcpy(env->value, value);
	}

	return 0;
}

static void ltx_handle_env(struct ltx_session *session)
{
	uint64_t slot_id = mp_message_read_uint(session->ltx_message.data + 1);
	if (slot_id > MAX_SLOTS) {
		LTX_HANDLE_ERROR(session, "Out of bound slot ID", 0);
		return;
	}

	char key[MAX_STRING_LEN];
	ltx_read_string(session, session->ltx_message.data + 2, key);

	char val[MAX_STRING_LEN];
	ltx_read_string(session, session->ltx_message.data + 3, val);

	struct ltx_slot *exec_slot;
	int error;

	if (slot_id == ALL_SLOTS) {
		for (unsigned i = 0; i < MAX_SLOTS; i++) {
			exec_slot = session->table.slots + i;
			error = ltx_env_set(
				session,
				exec_slot->env,
				key,
				val);
		}
	} else {
		exec_slot = session->table.slots + slot_id;
		error = ltx_env_set(
			session,
			exec_slot->env,
			key,
			val);
	}

	if (!error)
		ltx_echo(session);
}

static void ltx_handle_cwd(struct ltx_session *session)
{
	uint64_t slot_id = mp_message_read_uint(session->ltx_message.data + 1);
	if (slot_id > MAX_SLOTS) {
		LTX_HANDLE_ERROR(session, "Out of bound slot ID", 0);
		return;
	}

	char path[MAX_STRING_LEN];
	ltx_read_string(session, session->ltx_message.data + 2, path);

	struct stat sb;
	if (stat(path, &sb) || !S_ISDIR(sb.st_mode)) {
		LTX_HANDLE_ERROR(session, "CWD directory does not exist", 0);
		return;
	}

	struct ltx_slot *exec_slot;

	if (slot_id == ALL_SLOTS) {
		for (unsigned i = 0; i < MAX_SLOTS; i++) {
			exec_slot = session->table.slots + i;
			strcpy(exec_slot->cwd, path);
		}
	} else {
		exec_slot = session->table.slots + slot_id;
		strcpy(exec_slot->cwd, path);
	}

	ltx_echo(session);
}

static struct ltx_slot *ltx_slot_reserve(
	struct ltx_session *session,
	const uint64_t slot_id)
{
	assert(slot_id < MAX_SLOTS);

	struct ltx_slot *exec_slot = session->table.slots + slot_id;

	if (exec_slot->used) {
		LTX_HANDLE_ERROR(session, "Execution slot is reserved", 0);
		return NULL;
	}

	exec_slot->used = 1;

	return exec_slot;
}

static void ltx_slot_free(struct ltx_session *session, const uint64_t slot_id)
{
	assert(slot_id < MAX_SLOTS);

	struct ltx_slot *exec_slot;

	exec_slot = session->table.slots + slot_id;
	close(exec_slot->event.fd);

	memset(exec_slot, 0, sizeof(struct ltx_slot));
	exec_slot->pid = -1;
}

static int ltx_epoll_add(
	struct ltx_session *session,
	struct ltx_event *evt,
	const uint32_t events)
{
	struct epoll_event in_evt = {
		.events = events,
		.data.ptr = evt,
	};

	int ret = epoll_ctl(
		session->epoll_fd,
		EPOLL_CTL_ADD,
		evt->fd,
		&in_evt);

	if (ret == -1) {
		LTX_HANDLE_ERROR(session, "epoll_ctl() error", 1);
		return 1;
	}

	return 0;
}

static void ltx_run_command(struct ltx_session *session, const char *cmd)
{
	int ret;

	/* run with shell first */
	ret = execlp("sh", "sh", "-c", cmd, (char *) NULL);
	if (ret == -1 && errno != ENOENT) {
		LTX_HANDLE_ERROR(session, "execlp() error", 1);
		return;
	}

	/* if no shell is available, run without it */
	ret = execlp(cmd, cmd, (char *) NULL);
	if (ret == -1) {
		LTX_HANDLE_ERROR(session, "execlp() error", 1);
		return;
	}

	_exit(0);
}

static void ltx_handle_exec(struct ltx_session *session)
{
	/* read execution message */
	uint64_t slot_id = mp_message_read_uint(session->ltx_message.data + 1);
	if (slot_id >= MAX_SLOTS) {
		LTX_HANDLE_ERROR(session, "Out of bound slot ID", 0);
		return;
	}

	char cmd[MAX_STRING_LEN];
	ltx_read_string(session, session->ltx_message.data + 2, cmd);

	/* echo back the command */
	ltx_echo(session);

	/* reserve execution slot */
	struct ltx_slot *exec_slot = ltx_slot_reserve(
		session,
		slot_id);
	if (!exec_slot)
		return;

	/* create stdout pipe */
	int pipefd[2];

	if (pipe2(pipefd, O_CLOEXEC) == -1) {
		LTX_HANDLE_ERROR(session, "pipe2() error", 1);
		return;
	}

	/* setup event */
	exec_slot->event.type = LTX_EVT_STDOUT;
	exec_slot->event.slot_id = slot_id;
	exec_slot->event.fd = pipefd[0];

	assert(!ltx_epoll_add(session, &exec_slot->event, EPOLLIN));

	/* run the command */
	pid_t pid = fork();
	if (pid == -1) {
		LTX_HANDLE_ERROR(session, "fork() error", 1);
		return;
	}

	/* setup parent */
	if (pid) {
		close(pipefd[1]);
		exec_slot->pid = pid;
		return;
	}

	/* redirect stdout to pipe */
	if (dup2(pipefd[1], STDOUT_FILENO) == -1) {
		LTX_HANDLE_ERROR(session, "dup2() stdout", 1);
		exit(1);
	}

	if (dup2(pipefd[1], STDERR_FILENO) == -1) {
		LTX_HANDLE_ERROR(session, "dup2() stderr", 1);
		exit(1);
	}

	close(pipefd[0]);
	close(pipefd[1]);

	/* set global environment vars */
	for (unsigned i = 0; i < MAX_ENVS; i++) {
		struct ltx_env env = session->table.env[i];
		if (!strlen(env.key))
			continue;

		if (setenv(env.key, env.value, 1) == -1) {
			LTX_HANDLE_ERROR(
				session,
				"global setenv()",
				1);
			exit(1);
		}
	}

	/* set local environment vars */
	for (unsigned i = 0; i < MAX_ENVS; i++) {
		struct ltx_env env = exec_slot->env[i];
		if (!strlen(env.key))
			continue;

		if (setenv(env.key, env.value, 1) == -1) {
			LTX_HANDLE_ERROR(
				session,
				"local setenv()",
				1);
			exit(1);
		}
	}

	/* change directory */
	if (strlen(exec_slot->cwd) && chdir(exec_slot->cwd) == -1) {
		LTX_HANDLE_ERROR(session, "chdir() error", 1);
		exit(1);
	}

	/* execute the command */
	ltx_run_command(session, cmd);
}

static void ltx_handle_result(
	struct ltx_session *session,
	uint64_t slot_id,
	int ssi_code,
	int ssi_status)
{
	assert(slot_id < MAX_SLOTS);

	struct mp_message msgs[5];

	mp_message_uint(&msgs[0], LTX_RESULT);
	mp_message_uint(&msgs[1], slot_id);
	mp_message_uint(&msgs[2], ltx_gettime());
	mp_message_uint(&msgs[3], (uint64_t)ssi_code);
	mp_message_uint(&msgs[4], (uint64_t)ssi_status);

	ltx_send_messages(session, msgs, 5);
	ltx_message_reset(session);
}

static int ltx_check_stdout(struct ltx_session *session, const int slot_id)
{
	if (session->pid != getpid())
		return 0;

	assert(slot_id < MAX_SLOTS);

	struct ltx_slot *slot = session->table.slots + slot_id;

	ssize_t ret = read(slot->event.fd, slot->buffer, READ_BUFFER_SIZE);
	if (ret == -1) {
		if (errno != EAGAIN && errno != EWOULDBLOCK && errno != EINTR)
			LTX_HANDLE_ERROR(session, "read() log", 1);

		return 0;
	}

	if (!ret)
		return 0;

	struct mp_message msgs[4];

	mp_message_uint(&msgs[0], LTX_LOG);
	mp_message_uint(&msgs[1], slot_id);
	mp_message_uint(&msgs[2], ltx_gettime());
	mp_message_str(&msgs[3], (char *)slot->buffer, (size_t)ret);

	ltx_send_messages(session, msgs, 4);
	ltx_message_reset(session);

	return 1;
}

static void ltx_slots_waitpid(struct ltx_session *session)
{
	struct ltx_slot *slot;
	uint64_t slot_id;
	int ret;
	int status;
	int ssi_code;
	int ssi_status;

	for (slot_id = 0; slot_id < MAX_SLOTS; slot_id++) {
		slot = session->table.slots + slot_id;
		if (slot->pid == -1)
			continue;

		ret = waitpid(slot->pid, &status, WNOHANG);
		if (ret == -1) {
			LTX_HANDLE_ERROR(session, "waitpid() error", 1);
			return;
		}

		if (ret != slot->pid)
			continue;

		/* drain the child stdout before sending RESULT reply */
		while (ltx_check_stdout(session, slot_id)) {}

		if (WIFSIGNALED(status)) {
			ssi_code = CLD_KILLED;
			ssi_status = WTERMSIG(status);
		} else if (WIFSTOPPED(status)) {
			ssi_code = CLD_STOPPED;
			ssi_status = WSTOPSIG(status);
		} else {
			ssi_code = CLD_EXITED;
			ssi_status = WEXITSTATUS(status);
		}

		ltx_handle_result(
			session,
			slot_id,
			ssi_code,
			ssi_status);

		ltx_slot_free(session, slot_id);
	}
}

static void ltx_handle_log(struct ltx_session *session, struct ltx_event *evt)
{
	if (session->pid != getpid())
		return;

	ltx_check_stdout(session, evt->slot_id);
}

static void ltx_handle_kill(struct ltx_session *session)
{
	/* read message */
	uint64_t slot_id = mp_message_read_uint(session->ltx_message.data + 1);
	if (slot_id >= MAX_SLOTS) {
		LTX_HANDLE_ERROR(session, "Out of bound slot ID", 0);
		return;
	}

	struct ltx_slot *exec_slot = session->table.slots + slot_id;
	if (exec_slot->pid == -1)
		return;

	int ret = kill(exec_slot->pid, SIGKILL);
	if (ret == -1 && errno != ESRCH) {
		LTX_HANDLE_ERROR(session, "kill() error", 1);
		return;
	}

	/* echo back the command */
	ltx_echo(session);
}

static void ltx_process_msg(struct ltx_session *session)
{
	struct ltx_message *msg = &session->ltx_message;
	int reserve_next = 1;

	LTX_PRINT_MESSAGE(msg->data + msg->curr);

	if (msg->type == LTX_NONE) {
		if (!msg->length) {
			if (mp_message_type(msg->data) != MP_ARRAY) {
				LTX_HANDLE_ERROR(
					session,
					"Messages must be packed inside array",
					0);
				return;
			}

			/* read number messages inside the array */
			msg->length = mp_message_read_array_length(msg->data);

			/* array message is not carrying request information */
			reserve_next = 0;
		} else {
			/* handle requests composed by a single message */
			if (mp_message_type(msg->data) != MP_NUMERIC) {
				LTX_HANDLE_ERROR(
					session,
					"Message type must be a numeric",
					0);
				return;
			}

			/* numeric types are used to store commands type.
			* we save received command type and wait for next messages.
			*/
			msg->type = (uint32_t)mp_message_read_uint(msg->data);

			switch (msg->type) {
			/* handle commands with single message request */
			case LTX_VERSION:
				reserve_next = 0;
				ltx_handle_version(session);
				break;
			case LTX_PING:
				reserve_next = 0;
				ltx_handle_ping(session);
				break;
			/* handle commands which should never be sent */
			case LTX_PONG:
				reserve_next = 0;
				LTX_HANDLE_ERROR(
					session,
					"PONG should not be received",
					0);
				break;
			case LTX_ERROR:
				reserve_next = 0;
				LTX_HANDLE_ERROR(
					session,
					"ERROR should not be received",
					0);
				break;
			case LTX_DATA:
				reserve_next = 0;
				LTX_HANDLE_ERROR(
					session,
					"DATA should not be received",
					0);
				break;
			case LTX_RESULT:
				reserve_next = 0;
				LTX_HANDLE_ERROR(
					session,
					"RESULT should not be received",
					0);
				break;
			case LTX_LOG:
				reserve_next = 0;
				LTX_HANDLE_ERROR(
					session,
					"LOG should not be received",
					0);
				break;
			default:
				break;
			}
		}
	} else {
		/* handle requests composed by multiple messages */
		if (msg->curr == msg->length - 1) {
			reserve_next = 0;

			switch (msg->type) {
			case LTX_GET_FILE:
				ltx_handle_get_file(session);
				break;
			case LTX_SET_FILE:
				ltx_handle_set_file(session);
				break;
			case LTX_ENV:
				ltx_handle_env(session);
				break;
			case LTX_CWD:
				ltx_handle_cwd(session);
				break;
			case LTX_EXEC:
				ltx_handle_exec(session);
				break;
			case LTX_KILL:
				ltx_handle_kill(session);
				break;
			default:
				break;
			}
		}
	}

	if (reserve_next)
		ltx_message_reserve_next(session);
}

static int ltx_read_stdin(struct ltx_session *session, struct ltx_event *evt)
{
	ssize_t size = read(
		evt->fd,
		session->stdin_buffer,
		READ_BUFFER_SIZE);

	if (size == -1) {
		if (errno != EAGAIN && errno != EWOULDBLOCK)
			LTX_HANDLE_ERROR(session, "read() error", 1);

		return 0;
	}

	if (!size)
		return 0;

	int status;
	size_t offset = 0;

	do {
		offset += mp_unpacker_feed(
			&session->msg_unpacker,
			session->stdin_buffer + offset,
			size - offset);

		status = mp_unpacker_status(&session->msg_unpacker);

		switch (status) {
		case MP_UNPACKER_SUCCESS:
			ltx_process_msg(session);
			break;
		case MP_UNPACKER_TYPE_ERROR:
			LTX_HANDLE_ERROR(
				session,
				"Unsupported msgpack type",
				0);
			ltx_message_reset(session);
			break;
		default:
			break;
		}
	} while (offset && size > (ssize_t)offset);

	return 1;
}

static void ltx_signal_handler(int signo)
{
	switch (signo) {
	case SIGINT:
		stop_loop = 1;
		break;
	case SIGPIPE:
		broken_pipe = 1;
		break;
	}
}

struct ltx_session *ltx_session_init(const int stdin_fd, const int stdout_fd)
{
	assert(stdin_fd >= 0);
	assert(stdout_fd >= 0);

	struct ltx_session *session;

	session = (struct ltx_session *)mmap(
		NULL,
		sizeof(struct ltx_session),
		PROT_READ | PROT_WRITE,
		MAP_SHARED | MAP_ANONYMOUS,
		-1, 0);

	assert(session);
	memset(session, 0, sizeof(struct ltx_session));

	/* initialize application PID */
	session->pid = getpid();

	/* initialize file descriptors */
	session->stdin_fd = stdin_fd;
	session->stdout_fd = stdout_fd;
	session->debug_fd = STDERR_FILENO;

	/* reset ltx messages buffer */
	for (unsigned i = 0; i < MAX_MESSAGES; i++)
		mp_message_init(session->ltx_message.data + i);

	session->ltx_message.type = LTX_NONE;

	/* initialize message read/write buffers */
	mp_unpacker_init(&session->msg_unpacker);
	mp_unpacker_reserve(&session->msg_unpacker, session->ltx_message.data);

	/* initialize execution table */
	for (unsigned i = 0; i < MAX_SLOTS; i++)
		session->table.slots[i].pid = -1;

	/* handle ltx signals */
	struct sigaction action;
	memset(&action, 0, sizeof(action));
	action.sa_handler = ltx_signal_handler;

	sigaction(SIGINT, &action, NULL);
	sigaction(SIGPIPE, &action, NULL);

	return session;
}

void ltx_session_destroy(struct ltx_session *session)
{
	assert(session);

	ltx_session_stop(session);

	for (unsigned i = 0; i < MAX_MESSAGES; i++)
		mp_message_destroy(session->ltx_message.data + i);

	munmap(session, sizeof(struct ltx_session));
}

static void ltx_handle_loop_end(struct ltx_session *session)
{
	struct ltx_slot *exec_slot;
	int slot;
	int ret;

	for (slot = 0; slot < ALL_SLOTS; slot++) {
		exec_slot = session->table.slots + slot;
		if (exec_slot->pid == -1)
			continue;

		ret = kill(exec_slot->pid, SIGKILL);
		if (ret == -1) {
			if (errno != ESRCH)
				LTX_HANDLE_ERROR(session, "kill() error", 1);

			ltx_slot_free(session, slot);
			continue;
		}

		ret = waitpid(exec_slot->pid, NULL, 0);
		if (ret == -1)
			LTX_HANDLE_ERROR(session, "waitpid() error", 1);

		ltx_slot_free(session, slot);
	}

	ltx_message_reset(session);
}

static void ltx_event_loop(struct ltx_session *session)
{
	/* create epoll file descriptor */
	session->epoll_fd = epoll_create1(EPOLL_CLOEXEC);
	if (session->epoll_fd == -1) {
		LTX_HANDLE_ERROR(session, "epoll_create() error", 1);
		return;
	}

	/* setup stdin file handling */
	struct ltx_event evt_stdin = {
		.type = LTX_EVT_STDIN,
		.fd = session->stdin_fd,
		.slot_id = SLOT_NONE,
	};

	assert(fcntl(session->stdin_fd, F_SETFL, O_NONBLOCK) != -1);
	assert(!ltx_epoll_add(session, &evt_stdin, EPOLLIN | EPOLLET));

	/* loop through epoll events */
	struct epoll_event events[MAX_EVENTS];
	struct epoll_event *epoll_evt;
	struct ltx_event *ltx_evt;
	int i;
	int num;

	while (!(broken_pipe || stop_loop)) {
		num = epoll_wait(
			session->epoll_fd,
			events,
			MAX_EVENTS,
			1000);

		if (num == -1 && errno != EINTR) {
			LTX_HANDLE_ERROR(session, "epoll_wait() error", 1);
			goto exit;
		}

		for (i = 0; i < num; i++) {
			epoll_evt = events + i;
			ltx_evt = (struct ltx_event *) epoll_evt->data.ptr;

			if (epoll_evt->events & EPOLLERR) {
				char* msg;
				int ret = asprintf(
					&msg,
					"Catched EPOLLERR when polling %d file descriptor",
					ltx_evt->fd);
				assert(ret >= 0);

				LTX_HANDLE_ERROR(session, msg, 0);
				free(msg);

				goto exit;
			}

			switch (ltx_evt->type) {
			case LTX_EVT_STDIN:
				while (ltx_read_stdin(session, ltx_evt))
					;
				break;
			case LTX_EVT_STDOUT:
				ltx_handle_log(session, ltx_evt);
				break;
			default:
				break;
			}
		}

		ltx_slots_waitpid(session);
	}

exit:
	ltx_handle_loop_end(session);

	/* at this point we can close epoll file descriptor */
	close(session->epoll_fd);
}

void ltx_session_stop(struct ltx_session *session)
{
	assert(session);

	stop_loop = 1;
	while (fcntl(session->epoll_fd, F_GETFL) != -1 || errno != EBADF) {}
	stop_loop = 0;
}

void ltx_start_event_loop(struct ltx_session *session)
{
	assert(session);

	do {
		ltx_event_loop(session);

		if (broken_pipe) {
			ltx_warning(
				session,
				"Broken pipe. Restarting the event loop");
			broken_pipe = 0;
		}
	} while (!stop_loop);

	ltx_warning(session, "Event loop has been stopped");
}

void ltx_set_debug_fd(struct ltx_session *session, const int fd)
{
	session->debug_fd = fd;
}

void ltx_warning(struct ltx_session *session, const char *msg)
{
	if (strlen(msg) < 1)
		return;

	size_t len = strlen(msg);

	if (len > MAX_STRING_LEN)
		len = MAX_STRING_LEN;

	char data[len];
	strncpy(data, msg, len);

	struct mp_message msgs[2];

	mp_message_uint(&msgs[0], LTX_WARNING);
	mp_message_str(&msgs[1], data, len);

	ltx_send_messages(session, msgs, 2);
	ltx_message_reset(session);
}
