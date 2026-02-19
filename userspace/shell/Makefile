# Rux OS Shell Makefile
#
# 使用 musl libc 构建

# 工具链
CC = riscv64-linux-gnu-gcc

# musl libc 路径
MUSL_DIR = ../../toolchain/riscv64-rux-linux-musl
MUSL_INCLUDE = $(MUSL_DIR)/include
MUSL_LIB = $(MUSL_DIR)/lib

# 编译选项
CFLAGS = -static -Wall -Wextra -O2
CFLAGS += -I$(MUSL_INCLUDE)
CFLAGS += -fno-stack-protector -fno-builtin

# 链接选项 - 使用简化的链接脚本
LDFLAGS = -static -nostdlib
LDFLAGS += -T shell.ld
LDFLAGS += -L$(MUSL_LIB)

# musl libc 启动文件和库
MUSL_CRT = $(MUSL_LIB)/crt1.o
MUSL_LIBC = $(MUSL_LIB)/libc.a

# 目标文件
TARGET = shell

.PHONY: all clean

all: $(TARGET)

$(TARGET): src/shell.c
	@echo "Building shell with musl libc..."
	$(CC) $(CFLAGS) $(LDFLAGS) -o $@ $< $(MUSL_CRT) $(MUSL_LIBC) -lgcc
	@echo "Done: $(TARGET) ($$(stat -c%s $(TARGET) 2>/dev/null || echo ?) bytes)"

clean:
	rm -f $(TARGET)
