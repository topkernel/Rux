#!/bin/bash
# 快速配置演示 - 展示配置系统如何工作

# 获取项目根目录
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

cd "$PROJECT_ROOT"

echo "=========================================="
echo "  Rux 内核配置系统演示"
echo "=========================================="
echo ""

echo "1. 当前配置信息:"
echo "----------------------------------------"
echo "内核名称: $(grep '^name' Kernel.toml | head -1 | cut -d'"' -f2)"
echo "版本号: $(grep '^version' Kernel.toml | head -1 | cut -d'"' -f2)"
echo "目标平台: $(grep '^default_platform' Kernel.toml | head -1 | cut -d'"' -f2)"
echo ""

echo "2. 修改配置..."
echo "----------------------------------------"

# 临时修改配置
sed -i 's/^name = "Rux"/name = "RuxOS Demo"/' Kernel.toml
sed -i 's/^version = "0.1.0"/version = "0.2.0"/' Kernel.toml

echo "✓ 已将名称改为: RuxOS Demo"
echo "✓ 已将版本改为: 0.2.0"
echo ""

echo "3. 重新编译内核..."
echo "------------------------------------------"
cargo build --target aarch64-unknown-none 2>&1 | grep -E "(Compiling|Finished)" | tail -2
echo ""

echo "4. 查看生成的配置代码:"
echo "------------------------------------------"
echo "内核名称常量: $(grep '^pub const KERNEL_NAME' kernel/src/config.rs | cut -d'"' -f2)"
echo "版本常量: $(grep '^pub const KERNEL_VERSION' kernel/src/config.rs | cut -d'"' -f2)"
echo ""

echo "5. 恢复原始配置..."
echo "------------------------------------------"
sed -i 's/^name = "RuxOS Demo"/name = "Rux"/' Kernel.toml
sed -i 's/^version = "0.2.0"/version = "0.1.0"/' Kernel.toml
cargo build --target aarch64-unknown-none >/dev/null 2>&1
echo "✓ 配置已恢复"
echo ""

echo "=========================================="
echo "  配置系统演示完成！"
echo "=========================================="
echo ""
echo "使用方法:"
echo "  1. 编辑 Kernel.toml 文件"
echo "  2. 运行: cargo build --target aarch64-unknown-none"
echo "  3. 配置会自动编译到内核中"
echo ""
echo "交互式配置:"
echo "  运行: cd build && make menuconfig"
echo ""
