# Rux OS 测试脚本

本目录包含 Rux OS 内核的测试、构建和调试脚本。

## 快速开始

```bash
# 编译用户程序
./build_user_programs.sh

# 创建包含 shell 的 rootfs 镜像
./build_rootfs.sh

# 运行带 rootfs 的内核
./run_with_rootfs.sh
```

## 脚本说明

### 构建脚本

| 脚本 | 说明 |
|------|------|
| `build_user_programs.sh` | 编译用户空间程序 (shell) |
| `build_rootfs.sh` | 创建包含 shell 的 ext4 rootfs 镜像 |

### 运行脚本

| 脚本 | 说明 |
|------|------|
| `run_with_rootfs.sh` | 运行内核并从 rootfs 加载 shell |
| `run_riscv64.sh` | 运行 RISC-V 内核（基础） |
| `run_ext4.sh` | 运行 ext4 文件系统测试 |

### 测试脚本

| 脚本 | 说明 |
|------|------|
| `all.sh` | 运行所有测试 |
| `quick_test.sh` | 快速测试（基本功能） |
| `test_shell.sh` | 测试 shell 功能 |
| `test_smp_boot.sh` | 测试 SMP 多核启动 |
| `run_unit_tests.sh` | 运行单元测试 |

### 调试脚本

| 脚本 | 说明 |
|------|------|
| `debug_riscv.sh` | 使用 GDB 调试 RISC-V 内核 |
| `debug_kernel.sh` | 调试内核问题 |

### 辅助脚本

| 脚本 | 说明 |
|------|------|
| `create_ext4_image.sh` | 创建测试用的 ext4 镜像 |

## QEMU 参数

### 运行带 rootfs 的内核

```bash
qemu-system-riscv64 \
    -M virt \
    -cpu rv64 \
    -m 2G \
    -nographic \
    -drive file=test/rootfs.img,if=none,format=raw,id=rootfs \
    -device virtio-blk-device,drive=rootfs \
    -kernel target/riscv64gc-unknown-none-elf/debug/rux \
    -append "root=/dev/vda rw init=/bin/sh"
```

### 关键参数说明

- `-M virt` - 使用 QEMU virt 机器类型
- `-cpu rv64` - RISC-V 64位 CPU
- `-m 2G` - 2GB 内存
- `-nographic` - 无图形界面，使用串口控制台
- `-drive file=...,if=none` - 指定磁盘镜像文件
- `-device virtio-blk-device` - 使用 VirtIO 块设备
- `-append "..."` - 内核命令行参数

## Rootfs 镜像

### 创建 rootfs

```bash
# 1. 编译用户程序
./test/build_user_programs.sh

# 2. 创建 rootfs 镜像（需要 sudo）
./test/build_rootfs.sh
```

### Rootfs 结构

```
test/rootfs.img (ext4, 32MB)
├── bin/
│   ├── sh       -> shell 二进制
│   └── shell    -> shell 二进制
├── dev/
│   ├── console  -> 字符设备 5:1
│   ├── null     -> 字符设备 1:3
│   └── zero     -> 字符设备 1:5
├── etc/
└── lib/
```

## 故障排查

### VirtIO-Blk 设备未检测到

确保 QEMU 命令包含：
```bash
-drive file=test/rootfs.img,if=none,format=raw,id=rootfs \
-device virtio-blk-device,drive=rootfs
```

### Shell 无法从 rootfs 加载

检查：
1. Rootfs 镜像是否创建成功：`ls -lh test/rootfs.img`
2. Shell 二进制是否存在：`ls userspace/target/riscv64gc-unknown-none-elf/release/shell`
3. 内核日志中的 VirtIO-Blk 初始化信息

### 调试技巧

```bash
# 查看完整启动日志
./test/run_with_rootfs.sh
cat /tmp/kernel_boot.log

# 使用 GDB 调试
./test/debug_riscv.sh

# 增加调试输出
make build FEATURES="--features debug-print"
```
