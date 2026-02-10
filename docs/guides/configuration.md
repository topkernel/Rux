# Rux 内核配置系统使用指南

## 概述

Rux 内核提供了灵活的配置系统，支持通过 `Kernel.toml` 文件配置内核选项。配置系统在编译时解析配置并生成代码常量，实现零运行时开销的配置管理。

## 配置方式

### 方式 1: 直接编辑 Kernel.toml

```toml
[general]
name = "Rux"          # 内核名称
version = "0.1.0"     # 版本号

[platform]
default_platform = "riscv64"  # 目标平台（默认且仅支持）

[memory]
kernel_heap_size = 16         # 内核堆大小 (MB)
physical_memory = 2048        # 物理内存 (MB)
page_size = 4096              # 页大小

[smp]
enable_smp = true             # 启用多核支持
max_cpus = 4                  # 最大 CPU 数量
```

修改后运行：
```bash
cargo build --package rux --features riscv64
```

### 方式 2: 使用交互式配置菜单 (make menuconfig)

```bash
make menuconfig
```

这将启动一个 TUI（文本用户界面）配置菜单：

```
┌─────────────────────────────────────────────┐
│     Rux 内核配置                             │├─────────────────────────────────────────────┤
│                                             │
│  选择配置类别:                               │
│                                             │
│  1. 内存管理      7. 启动选项               │
│  2. SMP 多核      8. 调试选项               │
│  3. 调度器        9. 性能调优               │
│  4. 网络         10. 安全选项               │
│  5. 子功能       11. 查看配置               │
│  6. 驱动         12. 保存退出               │
│                                             │
│  <确定>          <取消>                      │
└─────────────────────────────────────────────┘
```

**依赖**: 需要 `whiptail` 包
```bash
# Ubuntu/Debian
sudo apt-get install whiptail

# RHEL/CentOS
sudo yum install newt
```

**使用说明**:
- 方向键: 选择选项
- Tab: 切换按钮
- Enter: 确认
- Esc: 取消/返回

## 配置类别详解

### 1. General（基本信息）

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `name` | string | "Rux" | 内核名称 |
| `version` | string | "0.1.0" | 版本号 |
| `authors` | array | ["Rux Developers"] | 开发者信息 |

### 2. Platform（平台）

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `default_platform` | string | "riscv64" | 目标平台 |
| `enable_riscv64` | bool | true | 启用 RISC-V 64位支持 |
| `enable_aarch64` | bool | true | 启用 ARM 64位支持（已移除） |
| `enable_x86_64` | bool | false | 启用 x86 64位支持（未实现） |

**注意**: 当前仅 RISC-V 64 位平台完全支持并默认启用。

### 3. Memory（内存管理）

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `kernel_heap_size` | integer | 16 | 内核堆大小 (MB) |
| `physical_memory` | integer | 2048 | 物理内存大小 (MB) |
| `page_size` | integer | 4096 | 页大小 (字节) |
| `user_stack_size` | integer | 8 | 用户栈大小 (MB) |
| `max_page_tables` | integer | 256 | 最大页表数量 |

### 4. SMP（多核支持）

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `enable_smp` | bool | true | 启用多核支持 (SMP) |
| `max_cpus` | integer | 4 | 最大 CPU 数量 |

**相关常量**: `MAX_CPUS`, `ENABLE_SMP`

### 5. Scheduler（调度器）

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `enable_scheduler` | bool | true | 启用调度器 |
| `default_time_slice_ms` | integer | 100 | 默认时间片 (毫秒) |
| `time_slice_ticks` | integer | 10 | 时间片滴答数 |

**相关常量**: `ENABLE_SCHEDULER`, `DEFAULT_TIME_SLICE_MS`, `TIME_SLICE_TICKS`

### 6. Network（网络协议栈）

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `enable_network` | bool | true | 启用网络协议栈 |
| `eth_mtu` | integer | 1500 | 以太网 MTU |
| `tcp_socket_table_size` | integer | 64 | TCP 套接字表大小 |
| `udp_socket_table_size` | integer | 64 | UDP 套接字表大小 |
| `arp_cache_size` | integer | 64 | ARP 缓存大小 |
| `route_table_size` | integer | 64 | 路由表大小 |
| `ip_default_ttl` | integer | 64 | IPv4 默认 TTL |

**相关常量**: `ENABLE_NETWORK`, `ETH_MTU`, `TCP_SOCKET_TABLE_SIZE`, `UDP_SOCKET_TABLE_SIZE`, `ARP_CACHE_SIZE`, `ROUTE_TABLE_SIZE`, `IP_DEFAULT_TTL`

### 7. Features（子功能使能）

网络协议栈子功能：
| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `enable_tcp` | bool | true | TCP 协议 |
| `enable_udp` | bool | true | UDP 协议 |
| `enable_arp` | bool | true | ARP 协议 |
| `enable_ipv4` | bool | true | IPv4 协议 |
| `enable_ethernet` | bool | true | 以太网 |

系统功能：
| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `enable_signal` | bool | true | 信号处理 |
| `enable_vm` | bool | true | 虚拟内存 |

文件系统功能：
| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `enable_vfs` | bool | true | VFS |
| `enable_pipe` | bool | true | 管道 |

### 8. Drivers（驱动）

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `enable_uart` | bool | true | UART 驱动 |
| `enable_timer` | bool | true | 定时器驱动 |
| `enable_gic` | bool | false | GIC 中断控制器（ARM） |
| `enable_virtio` | bool | false | VirtIO 设备驱动 |
| `enable_pci` | bool | false | PCI 设备驱动 |
| `enable_virtio_net_probe` | bool | true | VirtIO 网络设备探测 |

### 9. Boot（启动选项）

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `graphics` | bool | false | 启用图形输出 |
| `early_debug` | bool | true | 启用早期调试输出 |
| `self_test` | bool | false | 启用自检 |

### 10. Debug（调试）

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `debug_output` | bool | true | 启用调试输出 |
| `profiling` | bool | false | 启用性能分析 |
| `memory_trace` | bool | false | 启用内存跟踪 |
| `irq_trace` | bool | false | 启用中断跟踪 |
| `log_level` | string | "info" | 日志级别: error, warn, info, debug, trace |

### 11. Performance（性能调优）

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `opt_level` | integer | 3 | 优化级别 (0-3) |
| `lto` | bool | true | 启用 LTO (链接时优化) |
| `codegen_units` | integer | 1 | 代码生成单元 (1 = 更好的优化) |
| `strip` | bool | true | 启用本地符号 |

### 12. Security（安全选项）

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `stack_protector` | bool | false | 启用栈保护 |
| `bounds_check` | bool | true | 启用边界检查 |
| `overflow_check` | bool | true | 启用溢出检查 |

## 工作流程

```
┌─────────────┐
│ Kernel.toml │  ← 编辑配置文件
└──────┬──────┘
       │
       ▼
┌─────────────┐
│  build.rs   │  ← 解析 TOML，生成 Rust 代码
└──────┬──────┘
       │
       ▼
┌─────────────┐
│ config.rs   │  ← 自动生成的配置常量（不要手动编辑）
└──────┬──────┘
       │
       ▼
┌─────────────┐
│   编译内核   │
└─────────────┘
```

## 快速开始

### 1. 查看当前配置
```bash
cat Kernel.toml
```

### 2. 修改配置
```bash
# 直接编辑
vim Kernel.toml
```

### 3. 编译内核
```bash
# 方法 1: 使用 Make
make build

# 方法 2: 使用 Cargo
cargo build --package rux --features riscv64
```

### 4. 运行内核
```bash
# 使用测试脚本
make run
# 或
./test/quick_test.sh
```

## 配置示例

### 最小配置（嵌入式系统）
```toml
[memory]
kernel_heap_size = 4           # 4MB 内核堆
physical_memory = 128          # 128MB 物理内存
user_stack_size = 4            # 4MB 用户栈
page_size = 4096

[smp]
enable_smp = false             # 单核系统
max_cpus = 1

[scheduler]
enable_scheduler = true
default_time_slice_ms = 100
time_slice_ticks = 10

[network]
enable_network = false         # 禁用网络

[features]
# 禁用所有可选功能
enable_tcp = false
enable_udp = false
enable_arp = false
enable_signal = false
enable_vfs = false
enable_pipe = false

[debug]
log_level = "error"            # 仅输出错误
debug_output = false
```

### 完整配置（桌面/服务器系统）
```toml
[memory]
kernel_heap_size = 32          # 32MB 内核堆
physical_memory = 4096         # 4GB 物理内存
user_stack_size = 16           # 16MB 用户栈
max_page_tables = 512

[smp]
enable_smp = true
max_cpus = 8                   # 支持 8 核

[scheduler]
enable_scheduler = true
default_time_slice_ms = 50     # 更短的时间片
time_slice_ticks = 5

[network]
enable_network = true
eth_mtu = 9000                 # Jumbo 帧
tcp_socket_table_size = 256    # 更大的套接字表
udp_socket_table_size = 256
arp_cache_size = 128
route_table_size = 128
ip_default_ttl = 64

[features]
enable_tcp = true
enable_udp = true
enable_arp = true
enable_ipv4 = true
enable_ethernet = true
enable_signal = true
enable_vm = true
enable_vfs = true
enable_pipe = true

[drivers]
enable_uart = true
enable_timer = true
enable_virtio_net_probe = true

[debug]
log_level = "debug"
debug_output = true
profiling = true
```

### 开发配置
```toml
[memory]
kernel_heap_size = 16
physical_memory = 2048

[smp]
enable_smp = true
max_cpus = 2                   # 双核测试

[scheduler]
enable_scheduler = true
default_time_slice_ms = 100
time_slice_ticks = 10

[network]
enable_network = true
eth_mtu = 1500
tcp_socket_table_size = 64
udp_socket_table_size = 64
arp_cache_size = 64
route_table_size = 64
ip_default_ttl = 64

[features]
# 启用所有功能进行测试
enable_tcp = true
enable_udp = true
enable_arp = true
enable_ipv4 = true
enable_ethernet = true
enable_signal = true
enable_vm = true
enable_vfs = true
enable_pipe = true

[debug]
log_level = "trace"            # 详细日志
debug_output = true
memory_trace = true
irq_trace = true
```

## 在代码中使用配置

配置系统会生成 `kernel/src/config.rs` 文件，包含所有配置常量：

```rust
use crate::config::*;

// 使用 SMP 配置
if ENABLE_SMP {
    println!("SMP enabled, MAX_CPUS = {}", MAX_CPUS);
}

// 使用网络配置
if ENABLE_NETWORK {
    println!("TCP table size: {}", TCP_SOCKET_TABLE_SIZE);
}

// 使用调度器配置
if ENABLE_SCHEDULER {
    println!("Time slice: {}ms", DEFAULT_TIME_SLICE_MS);
}
```

## 配置常量参考

### 内存相关
- `KERNEL_HEAP_SIZE` - 内核堆大小（字节）
- `PHYS_MEMORY_SIZE` - 物理内存大小（字节）
- `PAGE_SIZE` - 页大小
- `PAGE_SHIFT` - 页大小位移
- `USER_STACK_SIZE` - 用户栈大小（字节）
- `USER_STACK_TOP` - 用户栈顶地址
- `MAX_PAGE_TABLES` - 最大页表数量

### SMP 相关
- `ENABLE_SMP` - 是否启用多核支持
- `MAX_CPUS` - 最大 CPU 数量

### 调度器相关
- `ENABLE_SCHEDULER` - 是否启用调度器
- `DEFAULT_TIME_SLICE_MS` - 默认时间片（毫秒）
- `TIME_SLICE_TICKS` - 时间片滴答数

### 网络相关
- `ENABLE_NETWORK` - 是否启用网络协议栈
- `ETH_MTU` - 以太网 MTU
- `TCP_SOCKET_TABLE_SIZE` - TCP 套接字表大小
- `UDP_SOCKET_TABLE_SIZE` - UDP 套接字表大小
- `ARP_CACHE_SIZE` - ARP 缓存大小
- `ROUTE_TABLE_SIZE` - 路由表大小
- `IP_DEFAULT_TTL` - IPv4 默认 TTL

### 子功能使能
- `ENABLE_TCP` - TCP 协议
- `ENABLE_UDP` - UDP 协议
- `ENABLE_ARP` - ARP 协议
- `ENABLE_IPV4` - IPv4 协议
- `ENABLE_ETHERNET` - 以太网
- `ENABLE_SIGNAL` - 信号处理
- `ENABLE_VM` - 虚拟内存
- `ENABLE_VFS` - VFS
- `ENABLE_PIPE` - 管道

## 注意事项

1. **配置文件路径**: `Kernel.toml` 必须在项目根目录
2. **自动生成**: `kernel/src/config.rs` 是自动生成的，**不要手动编辑**
3. **编译触发**: 修改 `Kernel.toml` 后会自动触发重新编译
4. **类型安全**: 所有配置值都有类型检查，无效值会被拒绝
5. **默认值**: 所有配置项都有合理的默认值，无需全部指定

## 故障排查

### 配置未生效
```bash
# 清理并重新编译
cargo clean
cargo build --package rux --features riscv64
```

### 查看生成的配置
```bash
# 查看完整配置
cat kernel/src/config.rs

# 查看特定配置
grep "MAX_CPUS\|USER_STACK" kernel/src/config.rs
```

### 验证配置值
```bash
# 在代码中打印配置值
println!("MAX_CPUS = {}", MAX_CPUS);
println!("TCP_SOCKET_TABLE_SIZE = {}", TCP_SOCKET_TABLE_SIZE);
```

### 配置错误
如果配置文件有语法错误，build.rs 会报错：
```
Error: 配置文件解析失败
```

检查 TOML 语法：
- 确保所有字符串用引号包裹
- 布尔值使用 `true`/`false`
- 整数不需要引号
- 确保正确的括号匹配
