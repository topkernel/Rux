# Rux 内核项目 Makefile
# 提供从项目根目录的快速访问

.PHONY: all build clean run test debug help smp user rootfs gui
.PHONY: cshell rust-shell toybox run-shell run-cshell run-rust-shell

# 默认目标：转发到 build/Makefile
all:
	@$(MAKE) -C build all

# 编译内核
build:
	@$(MAKE) -C build build

# 清理
clean:
	@$(MAKE) -C build clean

# 配置相关
config:
	@$(MAKE) -C build config

menuconfig:
	@$(MAKE) -C build menuconfig

# 构建 C shell (musl libc)
cshell:
	@echo "Building C shell with musl libc..."
	@$(MAKE) -C userspace/cshell

# 构建 Rust shell (std)
rust-shell:
	@echo "Building Rust shell with std..."
	@cd userspace/rust-shell && cargo build --release --target riscv64gc-unknown-linux-musl

# 构建 toybox (200+ Linux 命令行工具)
toybox:
	@echo "Building toybox with musl libc..."
	@cd userspace/toybox && ./build-toybox.sh

# 构建用户程序 (Rust)
user:
	@echo "Building user programs..."
	@./userspace/build.sh

# 创建 rootfs 镜像（包含所有 shell 和 toybox）
rootfs: cshell rust-shell user toybox
	@echo "Building rootfs image with all shells and toybox..."
	@./test/mkrootfs.sh

# 运行内核 (QEMU) - 默认使用 /bin/sh
run:
	@echo "启动 QEMU (默认 shell)..."
	@./test/run.sh console /bin/sh

# 运行默认 shell
run-shell:
	@echo "启动 QEMU (默认 shell)..."
	@./test/run.sh console /bin/shell

# 运行 C shell
run-cshell:
	@echo "启动 QEMU (C shell)..."
	@./test/run.sh console /bin/cshell

# 运行 Rust std shell
run-rust-shell:
	@echo "启动 QEMU (Rust std shell)..."
	@./test/run.sh console /bin/rust-shell

# 运行图形界面模式
gui:
	@echo "启动 QEMU (图形界面)..."
	@./test/run.sh gui /bin/sh

# 运行内核测试脚本
test:
	@./test/run.sh test

# SMP 测试
smp: build
	@echo "SMP 测试已移除，请使用 test.sh 进行单元测试"

# 调试
debug: build
	@$(MAKE) -C build debug

# 生成二进制
bin:
	@$(MAKE) -C build bin

# 项目信息
info:
	@$(MAKE) -C build info

# 依赖检查
deps:
	@$(MAKE) -C build deps

# 帮助
help:
	@echo "Rux 内核项目"
	@echo ""
	@echo "快速命令 (从项目根目录):"
	@echo "  make build           - 编译内核"
	@echo "  make clean           - 清理构建"
	@echo "  make run             - 运行内核（默认 shell）"
	@echo "  make run-shell       - 运行默认 no_std Rust shell"
	@echo "  make run-cshell      - 运行 C + musl shell"
	@echo "  make run-rust-shell  - 运行 Rust std shell"
	@echo "  make gui             - 运行图形界面模式"
	@echo "  make test            - 运行测试"
	@echo "  make rootfs          - 创建 rootfs 镜像"
	@echo "  make debug           - 调试内核"
	@echo "  make menuconfig      - 配置内核"
	@echo ""
	@echo "构建 shell:"
	@echo "  make user            - 构建 no_std 用户程序"
	@echo "  make cshell          - 构建 C shell (musl)"
	@echo "  make rust-shell      - 构建 Rust std shell"
	@echo "  make toybox          - 构建 toybox (200+ 命令行工具)"
	@echo ""
	@echo "目录结构:"
	@echo "  kernel/    - 内核源代码"
	@echo "  userspace/ - 用户程序"
	@echo "  build/     - 构建和配置工具"
	@echo "  test/      - 测试脚本"
	@echo "  docs/      - 文档"
