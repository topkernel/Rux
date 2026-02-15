# Rux OS - 用户态测试程序 Makefile
#
# 用法:
#   make            - 编译所有测试程序
#   make minimal    - 仅编译 minimal 测试
#   make clean      - 清理

CC = riscv64-linux-gnu-gcc
CFLAGS = -static -nostdlib -fno-builtin -Wall -Wextra
LDFLAGS = -T ../musl.ld

# 测试程序列表
TESTS = minimal

.PHONY: all clean $(TESTS)

all: $(TESTS)

minimal: minimal.c
	@echo "Building minimal test..."
	$(CC) $(CFLAGS) $(LDFLAGS) -o minimal minimal.c
	@echo "Done: minimal ($(shell stat -c%s minimal 2>/dev/null || stat -f%z minimal) bytes)"

clean:
	rm -f $(TESTS)
