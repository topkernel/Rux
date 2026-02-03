# Rux 内核项目 Makefile
# 提供从项目根目录的快速访问

.PHONY: all build clean run test debug help

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

# 运行内核
run: build
	@$(MAKE) -C build run

# 测试
test: build
	@$(MAKE) -C build test

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
	@echo "  make run         - 运行内核"
	@echo "  make test        - 运行测试"
	@echo "  make debug       - 调试内核"
	@echo "  make menuconfig  - 配置内核"
	@echo "  make help        - 显示帮助"
	@echo ""
	@echo "目录结构:"
	@echo "  kernel/  - 内核源代码"
	@echo "  build/   - 构建和配置工具"
	@echo "  test/    - 测试脚本"
	@echo "  docs/    - 文档"
	@echo ""
	@echo "详细帮助: make -C build help"
