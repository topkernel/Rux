# Rux Kernel Configuration System Guide

**Last Updated**: 2026-03-04

## Overview

The Rux kernel provides a flexible configuration system that supports configuring kernel options through the `Kernel.toml` file. The configuration system parses configuration at compile time and generates code constants, implementing zero runtime overhead configuration management.

## Configuration Methods

### Method 1: Directly Edit Kernel.toml

```toml
[general]
name = "Rux"          # Kernel name
version = "0.1.0"     # Version number

[platform]
default_platform = "riscv64"  # Target platform (default and only supported)

[memory]
kernel_heap_size = 16         # Kernel heap size (MB)
physical_memory = 2048        # Physical memory (MB)
page_size = 4096              # Page size

[smp]
enable_smp = true             # Enable multi-core support
max_cpus = 4                  # Maximum CPU count
```

After modification, run:
```bash
cargo build --package rux --features riscv64
```

### Method 2: Use Interactive Configuration Menu (make menuconfig)

```bash
make menuconfig
```

This will launch a TUI (Text User Interface) configuration menu:

```
+---------------------------------------------+
|     Rux Kernel Configuration                |
+---------------------------------------------+
|                                             |
|  Select configuration category:             |
|                                             |
|  1. Memory Management    7. Boot Options    |
|  2. SMP Multi-core       8. Debug Options   |
|  3. Scheduler            9. Performance     |
|  4. Network             10. Security        |
|  5. Sub-features        11. View Config     |
|  6. Drivers             12. Save and Exit   |
|                                             |
|  <OK>              <Cancel>                 |
+---------------------------------------------+
```

**Dependencies**: Requires `whiptail` package
```bash
# Ubuntu/Debian
sudo apt-get install whiptail

# RHEL/CentOS
sudo yum install newt
```

**Usage Instructions**:
- Arrow keys: Select options
- Tab: Switch buttons
- Enter: Confirm
- Esc: Cancel/Back

## Configuration Categories Detailed

### 1. General (Basic Information)

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `name` | string | "Rux" | Kernel name |
| `version` | string | "0.1.0" | Version number |
| `authors` | array | ["Rux Developers"] | Developer information |

### 2. Platform

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `default_platform` | string | "riscv64" | Target platform |
| `enable_riscv64` | bool | true | Enable RISC-V 64-bit support |
| `enable_aarch64` | bool | true | Enable ARM 64-bit support (removed) |
| `enable_x86_64` | bool | false | Enable x86 64-bit support (not implemented) |

**Note**: Currently only RISC-V 64-bit platform is fully supported and enabled by default.

### 3. Memory (Memory Management)

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `kernel_heap_size` | integer | 16 | Kernel heap size (MB) |
| `physical_memory` | integer | 2048 | Physical memory size (MB) |
| `page_size` | integer | 4096 | Page size (bytes) |
| `user_stack_size` | integer | 8 | User stack size (MB) |
| `max_page_tables` | integer | 256 | Maximum page table count |

### 4. SMP (Multi-core Support)

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `enable_smp` | bool | true | Enable multi-core support (SMP) |
| `max_cpus` | integer | 4 | Maximum CPU count |

**Related Constants**: `MAX_CPUS`, `ENABLE_SMP`

### 5. Scheduler

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `enable_scheduler` | bool | true | Enable scheduler |
| `default_time_slice_ms` | integer | 100 | Default time slice (milliseconds) |
| `time_slice_ticks` | integer | 10 | Time slice ticks |

**Related Constants**: `ENABLE_SCHEDULER`, `DEFAULT_TIME_SLICE_MS`, `TIME_SLICE_TICKS`

### 6. Network (Network Stack)

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `enable_network` | bool | true | Enable network stack |
| `eth_mtu` | integer | 1500 | Ethernet MTU |
| `tcp_socket_table_size` | integer | 64 | TCP socket table size |
| `udp_socket_table_size` | integer | 64 | UDP socket table size |
| `arp_cache_size` | integer | 64 | ARP cache size |
| `route_table_size` | integer | 64 | Route table size |
| `ip_default_ttl` | integer | 64 | IPv4 default TTL |

**Related Constants**: `ENABLE_NETWORK`, `ETH_MTU`, `TCP_SOCKET_TABLE_SIZE`, `UDP_SOCKET_TABLE_SIZE`, `ARP_CACHE_SIZE`, `ROUTE_TABLE_SIZE`, `IP_DEFAULT_TTL`

### 7. Features (Sub-feature Enablement)

Network stack sub-features:
| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `enable_tcp` | bool | true | TCP protocol |
| `enable_udp` | bool | true | UDP protocol |
| `enable_arp` | bool | true | ARP protocol |
| `enable_ipv4` | bool | true | IPv4 protocol |
| `enable_ethernet` | bool | true | Ethernet |

System features:
| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `enable_signal` | bool | true | Signal handling |
| `enable_vm` | bool | true | Virtual memory |

File system features:
| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `enable_vfs` | bool | true | VFS |
| `enable_pipe` | bool | true | Pipe |

### 8. Drivers

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `enable_uart` | bool | true | UART driver |
| `enable_timer` | bool | true | Timer driver |
| `enable_gic` | bool | false | GIC interrupt controller (ARM) |
| `enable_virtio` | bool | false | VirtIO device driver |
| `enable_pci` | bool | false | PCI device driver |
| `enable_virtio_net_probe` | bool | true | VirtIO network device probe |

### 9. Boot (Boot Options)

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `graphics` | bool | false | Enable graphics output |
| `early_debug` | bool | true | Enable early debug output |
| `self_test` | bool | false | Enable self-test |

### 10. Debug

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `debug_output` | bool | true | Enable debug output |
| `profiling` | bool | false | Enable profiling |
| `memory_trace` | bool | false | Enable memory tracing |
| `irq_trace` | bool | false | Enable interrupt tracing |
| `log_level` | string | "info" | Log level: error, warn, info, debug, trace |

### 11. Performance (Performance Tuning)

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `opt_level` | integer | 3 | Optimization level (0-3) |
| `lto` | bool | true | Enable LTO (Link Time Optimization) |
| `codegen_units` | integer | 1 | Code generation units (1 = better optimization) |
| `strip` | bool | true | Enable native symbols |

### 12. Security (Security Options)

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `stack_protector` | bool | false | Enable stack protection |
| `bounds_check` | bool | true | Enable bounds checking |
| `overflow_check` | bool | true | Enable overflow checking |

## Workflow

```
+-------------+
| Kernel.toml |  <- Edit configuration file
+------+------+
       |
       v
+-------------+
|  build.rs   |  <- Parse TOML, generate Rust code
+------+------+
       |
       v
+-------------+
| config.rs   |  <- Auto-generated configuration constants (do not edit manually)
+------+------+
       |
       v
+-------------+
| Build Kernel|
+-------------+
```

## Quick Start

### 1. View Current Configuration
```bash
cat Kernel.toml
```

### 2. Modify Configuration
```bash
# Edit directly
vim Kernel.toml
```

### 3. Build Kernel
```bash
# Method 1: Use Make
make build

# Method 2: Use Cargo
cargo build --package rux --features riscv64
```

### 4. Run Kernel
```bash
# Use test script
make run
# Or
./test/quick_test.sh
```

## Configuration Examples

### Minimal Configuration (Embedded System)
```toml
[memory]
kernel_heap_size = 4           # 4MB kernel heap
physical_memory = 128          # 128MB physical memory
user_stack_size = 4            # 4MB user stack
page_size = 4096

[smp]
enable_smp = false             # Single-core system
max_cpus = 1

[scheduler]
enable_scheduler = true
default_time_slice_ms = 100
time_slice_ticks = 10

[network]
enable_network = false         # Disable network

[features]
# Disable all optional features
enable_tcp = false
enable_udp = false
enable_arp = false
enable_signal = false
enable_vfs = false
enable_pipe = false

[debug]
log_level = "error"            # Only output errors
debug_output = false
```

### Full Configuration (Desktop/Server System)
```toml
[memory]
kernel_heap_size = 32          # 32MB kernel heap
physical_memory = 4096         # 4GB physical memory
user_stack_size = 16           # 16MB user stack
max_page_tables = 512

[smp]
enable_smp = true
max_cpus = 8                   # Support 8 cores

[scheduler]
enable_scheduler = true
default_time_slice_ms = 50     # Shorter time slice
time_slice_ticks = 5

[network]
enable_network = true
eth_mtu = 9000                 # Jumbo frames
tcp_socket_table_size = 256    # Larger socket tables
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

### Development Configuration
```toml
[memory]
kernel_heap_size = 16
physical_memory = 2048

[smp]
enable_smp = true
max_cpus = 2                   # Dual-core testing

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
# Enable all features for testing
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
log_level = "trace"            # Verbose logging
debug_output = true
memory_trace = true
irq_trace = true
```

## Using Configuration in Code

The configuration system generates a `kernel/src/config.rs` file containing all configuration constants:

```rust
use crate::config::*;

// Use SMP configuration
if ENABLE_SMP {
    println!("SMP enabled, MAX_CPUS = {}", MAX_CPUS);
}

// Use network configuration
if ENABLE_NETWORK {
    println!("TCP table size: {}", TCP_SOCKET_TABLE_SIZE);
}

// Use scheduler configuration
if ENABLE_SCHEDULER {
    println!("Time slice: {}ms", DEFAULT_TIME_SLICE_MS);
}
```

## Configuration Constants Reference

### Memory Related
- `KERNEL_HEAP_SIZE` - Kernel heap size (bytes)
- `PHYS_MEMORY_SIZE` - Physical memory size (bytes)
- `PAGE_SIZE` - Page size
- `PAGE_SHIFT` - Page size shift
- `USER_STACK_SIZE` - User stack size (bytes)
- `USER_STACK_TOP` - User stack top address
- `MAX_PAGE_TABLES` - Maximum page table count

### SMP Related
- `ENABLE_SMP` - Whether multi-core support is enabled
- `MAX_CPUS` - Maximum CPU count

### Scheduler Related
- `ENABLE_SCHEDULER` - Whether scheduler is enabled
- `DEFAULT_TIME_SLICE_MS` - Default time slice (milliseconds)
- `TIME_SLICE_TICKS` - Time slice ticks

### Network Related
- `ENABLE_NETWORK` - Whether network stack is enabled
- `ETH_MTU` - Ethernet MTU
- `TCP_SOCKET_TABLE_SIZE` - TCP socket table size
- `UDP_SOCKET_TABLE_SIZE` - UDP socket table size
- `ARP_CACHE_SIZE` - ARP cache size
- `ROUTE_TABLE_SIZE` - Route table size
- `IP_DEFAULT_TTL` - IPv4 default TTL

### Sub-feature Enablement
- `ENABLE_TCP` - TCP protocol
- `ENABLE_UDP` - UDP protocol
- `ENABLE_ARP` - ARP protocol
- `ENABLE_IPV4` - IPv4 protocol
- `ENABLE_ETHERNET` - Ethernet
- `ENABLE_SIGNAL` - Signal handling
- `ENABLE_VM` - Virtual memory
- `ENABLE_VFS` - VFS
- `ENABLE_PIPE` - Pipe

## Notes

1. **Configuration File Path**: `Kernel.toml` must be in the project root directory
2. **Auto-generated**: `kernel/src/config.rs` is auto-generated, **do not edit manually**
3. **Build Trigger**: Modifying `Kernel.toml` will automatically trigger recompilation
4. **Type Safety**: All configuration values have type checking, invalid values will be rejected
5. **Default Values**: All configuration items have reasonable default values, no need to specify all

## Troubleshooting

### Configuration Not Taking Effect
```bash
# Clean and rebuild
cargo clean
cargo build --package rux --features riscv64
```

### View Generated Configuration
```bash
# View full configuration
cat kernel/src/config.rs

# View specific configuration
grep "MAX_CPUS\|USER_STACK" kernel/src/config.rs
```

### Verify Configuration Values
```bash
# Print configuration values in code
println!("MAX_CPUS = {}", MAX_CPUS);
println!("TCP_SOCKET_TABLE_SIZE = {}", TCP_SOCKET_TABLE_SIZE);
```

### Configuration Errors
If the configuration file has syntax errors, build.rs will report:
```
Error: Configuration file parsing failed
```

Check TOML syntax:
- Ensure all strings are wrapped in quotes
- Boolean values use `true`/`false`
- Integers do not need quotes
- Ensure proper bracket matching
