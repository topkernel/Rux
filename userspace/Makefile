# Rux 用户程序 Makefile
#
# 构建用户空间程序并生成 ELF 二进制文件

.PHONY: all clean help list hello_world

# 设置环境变量来覆盖根目录的 cargo config
# 使用用户程序的链接器脚本而不是内核的链接器脚本
export CARGO_TARGET_RISCV64GC_UNKNOWN_NONE_ELF_RUSTFLAGS := \
	-Clink-arg=-T$(CURDIR)/user.ld \
	-Cforce-frame-pointers=yes \
	-Copt-level=z \
	-Clink-arg=--gc-sections

# 默认目标：构建所有用户程序
all: hello_world

# 列出所有用户程序
list:
	@echo "Rux 用户程序列表："
	@echo "  - hello_world: Hello World 示例程序"
	@echo ""
	@echo "构建命令："
	@echo "  make           - 构建所有程序"
	@echo "  make hello_world - 构建 hello_world 程序"
	@echo "  make clean     - 清理构建产物"

# 帮助信息
help: list
	@echo ""
	@echo "输出位置："
	@echo "  target/riscv64gc-unknown-none-elf/release/<program>"

# 构建 hello_world
hello_world:
	cargo build --release --bin hello_world --manifest-path $(CURDIR)/Cargo.toml
	@echo "✓ 构建完成: target/riscv64gc-unknown-none-elf/release/hello_world"
	@ls -lh target/riscv64gc-unknown-none-elf/release/hello_world

# 清理构建产物
clean:
	cargo clean --manifest-path $(CURDIR)/Cargo.toml
	@echo "✓ 清理完成"

# 显示构建信息
info:
	@echo "RISC-V 目标: riscv64gc-unknown-none-elf"
	@echo "工作空间成员: hello_world"
	@echo "链接器脚本: user.ld"
	@echo ""
	cargo --version
	rustc --version
