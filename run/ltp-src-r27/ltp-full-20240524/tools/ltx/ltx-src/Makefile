CC ?= gcc
CFLAGS ?= -pedantic -W -Wall
LDFLAGS ?= -shared -fPIC
INCLUDES = -Imsgpack
CFLAGS_DEBUG = -g -DDEBUG

# target build configuration
TARGET_SRCS = \
	msgpack/message.c \
	msgpack/unpack.c \
	ltx.c \
	main.c

TARGET = ltx

# library build configuration
LIB_SRCS = \
	msgpack/message.c \
	msgpack/unpack.c \
	ltx.c

LIBRARY = libltx.so

# tests build configuration
TESTS_DEPS = \
	msgpack/message.c \
	msgpack/unpack.c

TESTS_SRCS = \
	tests/test_message.c \
	tests/test_unpack.c \
	tests/test_utils.c

TESTS = $(TESTS_SRCS:%.c=%)

# make rules
.PHONY: $(TARGET) $(TESTS) clean debug shared shared-debug

all: $(TARGET_SRCS)
	$(CC) $(CFLAGS) $(INCLUDES) \
		$(TARGET_SRCS) -o $(TARGET)

debug: CFLAGS += $(CFLAGS_DEBUG)
debug: all

shared: $(LIB_SRCS)
	$(CC) $(CFLAGS) $(LDFLAGS) $(INCLUDES) \
		$(LIB_SRCS) -o $(LIBRARY)

shared-debug: CFLAGS += $(CFLAGS_DEBUG)
shared-debug: shared

test: $(TESTS)

$(TESTS): $(TESTS_DEPS)
	$(CC) $(CFLAGS) $(INCLUDES) \
		$(TESTS_DEPS) $@.c -o $@ \
		`pkg-config --cflags --libs check`

clean:
	$(RM) $(TARGET) $(LIBRARY) $(TESTS)
