#!/bin/bash
# 编译用户程序的脚本

set -e

cd userspace

# 使用 RUSTFLAGS 环境变量来覆盖根目录的链接器配置
export RUSTFLAGS="-C link-arg=-Tuser.ld -C force-frame-pointers=yes"

echo "Building user programs..."
cargo build --release -p shell

echo "Done!"
