# Rux OS Shell Makefile
#
# Build with musl libc

# Toolchain
CC = riscv64-linux-gnu-gcc

# musl libc paths
MUSL_DIR = ../../toolchain/riscv64-rux-linux-musl
MUSL_INCLUDE = $(MUSL_DIR)/include
MUSL_LIB = $(MUSL_DIR)/lib

# Compile options
CFLAGS = -static -Wall -Wextra -O2
CFLAGS += -I$(MUSL_INCLUDE)
CFLAGS += -fno-stack-protector -fno-builtin

# Link options - use simplified linker script
LDFLAGS = -static -nostdlib
LDFLAGS += -T shell.ld
LDFLAGS += -L$(MUSL_LIB)

# musl libc startup files and libraries
MUSL_CRT = $(MUSL_LIB)/crt1.o
MUSL_LIBC = $(MUSL_LIB)/libc.a

# Target file
TARGET = shell

.PHONY: all clean

all: $(TARGET)

$(TARGET): src/shell.c
	@echo "Building shell with musl libc..."
	$(CC) $(CFLAGS) $(LDFLAGS) -o $@ $< $(MUSL_CRT) $(MUSL_LIBC) -lgcc
	@echo "Done: $(TARGET) ($$(stat -c%s $(TARGET) 2>/dev/null || echo ?) bytes)"

clean:
	rm -f $(TARGET)
