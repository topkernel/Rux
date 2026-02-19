# Rux 内核项目 Makefile
# 提供从项目根目录的快速访问

.PHONY: all build clean run test debug help smp user rootfs gui
.PHONY: shell toybox

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

# 构建 shell (musl libc)
shell:
	@echo "Building shell with musl libc..."
	@$(MAKE) -C userspace/shell

# 构建 toybox (200+ Linux 命令行工具)
toybox:
	@echo "Building toybox with musl libc..."
	@cd userspace/toybox && ./build-toybox.sh

# 构建用户程序 (Rust no_std) - 同时编译 debug 和 release
user:
	@echo "Building user programs (debug)..."
	@./userspace/build.sh debug
	@echo "Building user programs (release)..."
	@./userspace/build.sh release

# 创建 rootfs 镜像（包含 shell 和 toybox）
rootfs: user toybox
	@echo "Building rootfs image with shell and toybox..."
	@./test/mkrootfs.sh

# 运行内核 (QEMU) - 默认使用 shell
run:
	@echo "启动 QEMU (shell)..."
	@./test/run.sh console /bin/shell

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
	@echo "  make run             - 运行内核（shell）"
	@echo "  make gui             - 运行图形界面模式"
	@echo "  make test            - 运行测试"
	@echo "  make rootfs          - 创建 rootfs 镜像"
	@echo "  make debug           - 调试内核"
	@echo "  make menuconfig      - 配置内核"
	@echo ""
	@echo "构建用户程序:"
	@echo "  make user            - 构建所有用户程序 (shell, desktop 等)"
	@echo "  make shell           - 构建 shell (musl libc)"
	@echo "  make toybox          - 构建 toybox (200+ 命令行工具)"
	@echo ""
	@echo "目录结构:"
	@echo "  kernel/    - 内核源代码"
	@echo "  userspace/ - 用户程序"
	@echo "  build/     - 构建和配置工具"
	@echo "  test/      - 测试脚本"
	@echo "  docs/      - 文档"
