# Rux 内核项目 Makefile
# 提供从项目根目录的快速访问

.PHONY: all build clean run test debug help smp user rootfs gui

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

# 构建用户程序
user:
	@echo "Building user programs..."
	@./userspace/build.sh

# 创建 rootfs 镜像
rootfs: user
	@echo "Building rootfs image..."
	@./test/mkrootfs.sh

# 运行内核 (QEMU)
run:
	@echo "启动 QEMU..."
	@./test/run.sh

# 运行图形界面模式
gui:
	@echo "启动 QEMU (图形界面)..."
	@./test/run.sh gui

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
	@echo "  make build       - 编译内核"
	@echo "  make clean       - 清理构建"
	@echo "  make run         - 运行内核（控制台模式）"
	@echo "  make gui         - 运行内核（图形界面模式）"
	@echo "  make test        - 运行测试"
	@echo "  make user        - 构建用户程序"
	@echo "  make rootfs      - 创建 rootfs 镜像"
	@echo "  make debug       - 调试内核"
	@echo "  make menuconfig  - 配置内核"
	@echo "  make help        - 显示帮助"
	@echo ""
	@echo "目录结构:"
	@echo "  kernel/    - 内核源代码"
	@echo "  userspace/ - 用户程序"
	@echo "  build/     - 构建和配置工具"
	@echo "  test/      - 测试脚本"
	@echo "  docs/      - 文档"
	@echo ""
	@echo "Rootfs 工作流:"
	@echo "  1. make user       - 编译用户程序"
	@echo "  2. make rootfs     - 创建 rootfs 镜像"
	@echo "  3. make gui        - 运行图形界面（在 shell 中执行 /bin/desktop）"
	@echo ""
	@echo "测试脚本:"
	@echo "  ./test/mkrootfs.sh  - 创建 rootfs 镜像"
	@echo "  ./test/run.sh       - 运行内核 (支持 test/run 模式)"
	@echo ""
	@echo "详细帮助: make -C build help"
