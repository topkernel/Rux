# Rux 内核 Makefile
# 提供便捷的构建和配置命令

# 项目根目录
PROJECT_ROOT := $(shell pwd)/..
BUILD_DIR := $(shell pwd)

.PHONY: all config menuconfig build clean run help test

# 默认目标
all: build

# 显示配置信息
config:
	@echo "当前内核配置:"
	@echo "=================="
	@echo "内核名称: $$(grep '^name' $(PROJECT_ROOT)/Kernel.toml | head -1 | cut -d'"' -f2)"
	@echo "版本号: $$(grep '^version' $(PROJECT_ROOT)/Kernel.toml | head -1 | cut -d'"' -f2)"
	@echo "目标平台: $$(grep '^default_platform' $(PROJECT_ROOT)/Kernel.toml | head -1 | cut -d'"' -f2)"
	@echo ""
	@echo "内存配置:"
	@echo "  堆大小: $$(grep '^kernel_heap_size' $(PROJECT_ROOT)/Kernel.toml | cut -d= -f2) MB"
	@echo "  物理内存: $$(grep '^physical_memory' $(PROJECT_ROOT)/Kernel.toml | cut -d= -f2) MB"
	@echo ""
	@echo "生成的常量:"
	@echo "  KERNEL_NAME = $$(grep '^pub const KERNEL_NAME' $(PROJECT_ROOT)/kernel/src/config.rs | cut -d'"' -f2)"
	@echo "  KERNEL_VERSION = $$(grep '^pub const KERNEL_VERSION' $(PROJECT_ROOT)/kernel/src/config.rs | cut -d'"' -f2)"

# 交互式配置菜单
menuconfig:
	@echo "启动配置菜单..."
	@$(BUILD_DIR)/menuconfig.sh

# 编译内核
build:
	@echo "编译 Rux 内核..."
	@cd $(PROJECT_ROOT) && cargo build --target riscv64gc-unknown-none-elf --features debug_log

# Release构建（不包含debug日志）
build-release:
	@echo "编译 Rux 内核 (Release)..."
	@cd $(PROJECT_ROOT) && cargo build --target riscv64gc-unknown-none-elf --release

# 快速构建（仅编译，不输出信息）
build-quiet:
	@cd $(PROJECT_ROOT) && cargo build --target riscv64gc-unknown-none-elf --quiet

# 清理构建
clean:
	@echo "清理构建产物..."
	@cd $(PROJECT_ROOT) && cargo clean
	@rm -f $(PROJECT_ROOT)/kernel.bin
	@rm -f $(PROJECT_ROOT)/serial.log
	@rm -f $(PROJECT_ROOT)/virt.dtb
	@echo "清理完成"


# 调试内核
debug: build
	@echo "启动 QEMU (调试模式)..."
	@cd $(PROJECT_ROOT) && qemu-system-riscv64 -M virt -cpu rv64 -m 2G -nographic \
		-kernel target/riscv64gc-unknown-none-elf/debug/rux \
		-S -s & \
		echo "QEMU PID: $$!" ; \
		echo "使用 GDB 连接: riscv64-unknown-elf-gdb -ex 'target remote localhost:1234'"

# 生成内核二进制
bin: build
	@echo "生成内核二进制文件..."
	@cd $(PROJECT_ROOT) && rust-objcopy -O binary target/riscv64gc-unknown-none-elf/debug/rux kernel.bin
	@echo "二进制文件: $(PROJECT_ROOT)/kernel.bin"

# 显示项目信息
info:
	@echo "Rux 内核项目信息"
	@echo "=================="
	@echo "项目根目录: $(PROJECT_ROOT)"
	@echo "构建目录:   $(BUILD_DIR)"
	@echo ""
	@echo "目录结构:"
	@echo "  kernel/  - 内核源代码"
	@echo "  build/   - 构建和配置工具"
	@echo "  test/    - 测试脚本"
	@echo "  docs/    - 文档"
	@echo ""
	@echo "配置文件:"
	@ls -1 $(PROJECT_ROOT)/*.toml 2>/dev/null || echo "  (无)"

# 安装依赖
deps:
	@echo "检查构建依赖..."
	@which cargo >/dev/null || (echo "错误: 未安装 Rust" && exit 1)
	@which qemu-system-riscv64 >/dev/null || (echo "警告: 未安装 QEMU" && exit 0)
	@which rust-objcopy >/dev/null || (echo "警告: 未安装 rust-objcopy" && exit 0)
	@echo "依赖检查完成"

# 显示帮助
help:
	@echo "Rux 内核构建系统"
	@echo ""
	@echo "使用方法 (从项目根目录运行):"
	@echo "  cd .. && make -C build <目标>"
	@echo ""
	@echo "或者使用构建目录内的 Makefile:"
	@echo "  make <目标>"
	@echo ""
	@echo "可用目标:"
	@echo "  make config       - 显示当前配置"
	@echo "  make menuconfig   - 交互式配置菜单"
	@echo "  make build        - 编译内核"
	@echo "  make build-quiet  - 静默编译"
	@echo "  make clean        - 清理构建产物"
	@echo "  make run          - 运行内核 (QEMU)"
	@echo "  make test         - 运行测试套件"
	@echo "  make debug        - 调试内核 (QEMU + GDB)"
	@echo "  make bin          - 生成二进制文件"
	@echo "  make info         - 显示项目信息"
	@echo "  make deps         - 检查构建依赖"
	@echo "  make help         - 显示此帮助"
	@echo ""
	@echo "快速开始:"
	@echo "  1. make menuconfig  # 配置内核"
	@echo "  2. make build       # 编译内核"
	@echo "  3. make run         # 运行内核"
	@echo ""
	@echo "配置文件:"
	@echo "  Kernel.toml - 主配置文件"
	@echo ""
	@echo "文档:"
	@echo "  docs/CONFIG.md - 配置系统文档"
	@echo "  docs/DESIGN.md  - 设计文档"
	@echo "  TODO.md         - 任务列表"
