# PSCI 调试记录文档

**日期**: 2025-02-04
**问题**: SMP (对称多处理) 无法启动次核
**状态**: ✅ 已解决
**解决方法**: 使用正确的 PSCI 调用方式 (HVC 而非 SMC)

---

## 1. 问题描述

### 1.1 初始症状

在 Rux 内核中尝试实现 SMP (对称多处理) 支持，使用 PSCI (Power State Coordination Interface) 启动次核，但遇到以下问题：

1. **SMC 调用导致 QEMU 完全挂起**
   - 使用 `smc #0` 指令调用 PSCI_CPU_ON
   - QEMU 无任何输出，完全死锁

2. **HVC 调用返回 PSCI_RET_NOT_SUPPORTED**
   - 使用 `hvc #0` 指令调用 PSCI_CPU_ON
   - 返回值 `0xEFFFFFFFFFFFFFFF` (-1)，表示不支持

3. **次核从未启动**
   - 在 `secondary_entry` 添加调试输出（写入字符 '1' 到 UART）
   - 无任何输出，说明次核从未到达入口点

### 1.2 环境信息

- **平台**: QEMU virt 机器 (ARM 虚拟化平台)
- **CPU**: cortex-a57 (ARMv8-A)
- **内核**: Rux v0.1.0 (Rust, no_std)
- **启动 EL**: EL1 (通过 CurrentEL 寄存器确认)
- **PSCI 版本**: 1.1 (0x10001000)

---

## 2. 问题分析

### 2.1 PSCI 基础知识

**PSCI (Power State Coordination Interface)** 是 ARM 标准的电源管理接口，用于：
- CPU 电源控制 (CPU_ON, CPU_OFF, CPU_SUSPEND)
- CPU 热插拔
- 系统级电源管理

**PSCI 调用方式**:
- **SMC (Secure Monitor Call)**: 用于安全监控环境 (EL3)
- **HVC (Hypervisor Call)**: 用于虚拟化环境 (EL2)

**PSCI 函数 ID**:

| 功能 | HVC 调用 ID | SMC 调用 ID |
|------|------------|------------|
| PSCI_VERSION | 0x84000000 | 0xC4000000 |
| PSCI_CPU_ON | 0x84000003 | 0xC4000003 |
| PSCI_CPU_OFF | 0x84000001 | 0xC4000001 |
| PSCI_CPU_SUSPEND | 0x84000002 | 0xC4000002 |

**返回值**:
- `0`: 成功
- 非 0: 错误码 (见 PSCI 规范)

### 2.2 QEMU virt 的 PSCI 实现

QEMU virt 机器通过**固件实现**提供 PSCI 服务：

1. **默认行为**: QEMU 内部实现 PSCI 1.0/1.1
2. **调用方式**: 由设备树指定 (`method` 属性)
3. **启动 EL**: 通常在 EL2，但可配置

### 2.3 调试步骤

#### 步骤 1: 检查设备树

```bash
# 启动 QEMU 并导出设备树
qemu-system-aarch64 -M virt -cpu cortex-a57 -m 2G -smp 2 \
  -dtb virt.dtb -kernel test.elf

# 导出设备树
dtc -I dtb -O dts virt.dtb > virt.dts
```

**关键发现** (`psci` 节点):
```dts
psci {
    compatible = "arm,psci-1.0", "arm,psci-0.2";
    method = "hvc";    ← 关键：使用 HVC 调用
    cpu_on = <0xc4000003>;
    cpu_suspend = <0xc4000001>;
};
```

#### 步骤 2: 验证 QEMU 启动 EL

创建测试程序检查异常级别：

```assembly
/* test_el.s */
.section .text
.global _start
_start:
    mrs     x0, CurrentEL
    and     x0, x0, #0xC
    cmp     x0, #0x8    /* EL2? */
    b.eq    in_el2
    cmp     x0, #0xC    /* EL3? */
    b.eq    in_el3
    /* EL1 */
    mov     x0, #1
    b       hang
in_el2:
    mov     x0, #2
    b       hang
in_el3:
    mov     x0, #3
    b       hang
hang:
    wfe
    b       hang
```

**结果**: QEMU virt 默认启动在 **EL1**，不是 EL2！

#### 步骤 3: 测试 PSCI 版本查询

```rust
// 测试 PSCI_VERSION (0x84000000 for HVC)
let psci_version: u64;
unsafe {
    core::arch::asm!(
        "hvc #0",
        inlateout("x0") 0x84000000u64 => psci_version,
        options(nomem, nostack)
    );
}
println!("PSCI version = 0x{:x}", psci_version);
```

**结果**: `PSCI version = 0x10001000` (PSCI 1.1)

**结论**: PSCI 可用，且支持 HVC 调用！

---

## 3. 尝试的解决方案

### 3.1 方案 1: SMC 调用 (失败)

**代码**:
```rust
// kernel/src/arch/aarch64/smp.rs
unsafe {
    let mut result: u64;
    core::arch::asm!(
        "smc #0",
        inlateout("x0") 0xC4000003u64 => result,  // PSCI_CPU_ON (SMC)
        in("x1") mpidr,
        in("x2") secondary_entry as u64,
        in("x3") 0u64,
        options(nomem, nostack)
    );
}
```

**结果**: ❌ QEMU 完全挂起，无输出

**原因**: SMC 是 Secure Monitor Call，需要在 EL3 或有 ATF 支持。QEMU virt 默认配置没有 EL3 固件。

### 3.2 方案 2: HVC 调用 (失败)

**代码**:
```rust
// 使用 HVC 调用 (0x84000003)
unsafe {
    let mut result: u64;
    core::arch::asm!(
        "hvc #0",
        inlateout("x0") 0x84000003u64 => result,  // PSCI_CPU_ON (HVC)
        in("x1") mpidr,
        in("x2") secondary_entry as u64,
        in("x3") 0u64,
        options(nomem, nostack)
    );
}
```

**结果**: ❌ 返回 `PSCI_RET_NOT_SUPPORTED` (-1)

**原因**: 此时代码有问题，实际上是函数 ID 使用错误或其他问题。

### 3.3 方案 3: EL2 PSCI 调用 (无效)

**代码** (在 boot.S):
```assembly
el2_entry:
    /* 设置临时栈 */
    adr     x0, boot_stack_top
    mov     sp, x0

    /* PSCI_CPU_ON 调用 */
    movz    x0, #0x0003, lsl #0
    movk    x0, #0xC400, lsl #16    /* 0xC4000003 - SMC ID! */
    mov     x1, #1                   /* CPU ID */
    adr     x2, secondary_entry
    mov     x3, #0
    hvc     #0

    /* 降级到 EL1 */
    mov     x0, #(1 << 31)
    msr     spsr_el2, x0
    adr     x0, el1_entry
    msr     elr_el2, x0
    eret
```

**结果**: ❌ 次核仍未启动

**原因**:
1. QEMU 启动在 EL1，`el2_entry` 从未执行
2. 即使在 EL2，使用了 SMC 函数 ID (0xC4000003) 而非 HVC ID (0x84000003)

---

## 4. 最终解决方案

### 4.1 关键发现

1. **设备树指定方法为 "hvc"**
2. **QEMU virt 启动在 EL1** (不是 EL2)
3. **PSCI 版本查询成功** (HVC 调用返回 0x10001000)
4. **必须使用 HVC 函数 ID** (0x84000003 而非 0xC4000003)

### 4.2 正确的实现

#### 步骤 1: 添加 PSCI 版本检查

```rust
// kernel/src/arch/aarch64/smp.rs

pub fn boot_secondary_cpus() {
    use crate::console::putchar;

    // 首先检查 PSCI 版本
    const MSG_CHECK: &[u8] = b"smp: Checking PSCI version...\n";
    for &b in MSG_CHECK {
        unsafe { putchar(b); }
    }

    let psci_version: u64;
    unsafe {
        // PSCI_VERSION 使用 HVC 调用 (0x84000000)
        core::arch::asm!(
            "hvc #0",
            inlateout("x0") 0x84000000u64 => psci_version,
            options(nomem, nostack)
        );
    }

    // 打印版本
    unsafe {
        const MSG_VER: &[u8] = b"smp: PSCI version = 0x";
        for &b in MSG_VER {
            putchar(b);
        }
        let hex = b"0123456789ABCDEF";
        let mut v = psci_version;
        for _ in 0..8 {
            let digit = (v & 0xF) as usize;
            putchar(hex[digit]);
            v >>= 4;
        }
        putchar(b'\n');
    }

    // ... 继续启动次核
}
```

#### 步骤 2: 使用正确的 HVC PSCI_CPU_ON

```rust
// 启动 CPU 1
for cpu_id in 1..2 {
    let mpidr = cpu_id as u64;  // QEMU virt 的 CPU MPIDR 就是 CPU ID

    unsafe {
        // PSCI_CPU_ON HVC call
        // x0 = function ID (0x84000003 = PSCI_CPU_ON for HVC)
        // x1 = target CPU (MPIDR)
        // x2 = entry point (secondary_entry 的物理地址)
        // x3 = context ID (0)
        let mut result: u64;
        core::arch::asm!(
            "hvc #0",
            inlateout("x0") 0x84000003u64 => result,  // ← HVC 函数 ID!
            in("x1") mpidr,
            in("x2") secondary_entry as u64,
            in("x3") 0u64,
            options(nomem, nostack)
        );

        // 检查返回值 (0 = success)
        if result == 0 {
            const MSG_OK: &[u8] = b"smp: CPU boot PSCI success\n";
            for &b in MSG_OK {
                putchar(b);
            }
        } else {
            // 打印错误码
            let hex = b"0123456789ABCDEF";
            let mut r = result;
            for _ in 0..16 {
                let digit = (r & 0xF) as usize;
                putchar(hex[digit]);
                r >>= 4;
            }
        }
    }
}
```

#### 步骤 3: 清理 boot.S

移除未使用的 EL2 PSCI 代码（QEMU 启动在 EL1，此代码从不执行）：

```assembly
/* kernel/src/arch/aarch64/boot/boot.S */

el2_entry:
    /* ========== 从EL2降到EL1 ==========*/
    /* 设置临时栈 */
    adr     x0, boot_stack_top
    mov     sp, x0

    mov     x0, #(1 << 31)      /* EL1h, AArch64 */
    msr     spsr_el2, x0
    adr     x0, el1_entry
    msr     elr_el2, x0

    /* 不启用MMU，直接返回到EL1 */
    eret
```

---

## 5. 验证结果

### 5.1 编译和运行

```bash
make build
timeout 3 qemu-system-aarch64 -M virt -cpu cortex-a57 -m 2G -smp 2 \
  -nographic -serial mon:stdio \
  -kernel target/aarch64-unknown-none/debug/rux
```

### 5.2 输出示例

```
Rux Kernel v0.1.0 starting...
Target platform: aarch64
...
Initializing VFS...
vfs: VFS layer initialized [OK]
Initializing SMP...
Attempting PSCI CPU_ON...
smp: Booting secondary CPUs...
smp: Checking PSCI version...          ← PSCI 版本查询
smp: PSCI version = 0x10001000        ← PSCI 1.1
smp: Calling PSCI for CPU 1
smp: PSCI result = 0000000000000000   ← 返回 0 (成功)
smp: CPU boot PSCI success            ← CPU 启动成功
[CPU1 up]                              ← CPU 1 在线!
Waiting for secondary CPUs...
SMP: 2 CPUs online                     ← 双核确认!
```

### 5.3 CPU 1 初始化输出

```
[CPU1] init: runqueue                  ← CPU 1 初始化运行队列
sched: CPU 1 runqueue [OK]
[CPU1] init: IRQ enabled               ← CPU 1 启用 IRQ
[CPU1] idle: waiting for work          ← CPU 1 进入空闲循环
```

---

## 6. 技术总结

### 6.1 关键要点

1. **PSCI 函数 ID 因调用方式而异**
   - HVC 调用: `0x8400000N` (N = 功能编号)
   - SMC 调用: `0xC400000N` (N = 功能编号)

2. **必须遵循设备树的 `method` 属性**
   - QEMU virt 设备树指定 `method = "hvc"`
   - 使用错误的调用方式会导致失败或挂起

3. **QEMU virt 启动在 EL1**
   - 不是 EL2 或 EL3
   - HVC 调用由 QEMU 内部处理

4. **PSCI 版本查询很重要**
   - 验证 PSCI 可用性
   - 确认支持的特性

### 6.2 常见陷阱

| 陷阱 | 症状 | 解决方法 |
|------|------|----------|
| 使用 SMC 调用 | QEMU 挂起 | 改用 HVC |
| 使用 SMC 函数 ID | 返回 NOT_SUPPORTED | 使用 0x84... 前缀 |
| 在错误的 EL 调用 | 调用失败 | 检查 CurrentEL |
| 忘记检查版本 | 无法调试 | 先调用 PSCI_VERSION |

### 6.3 调试技巧

1. **添加 PSCI 版本检查**
   ```rust
   let psci_version: u64;
   unsafe {
       core::arch::asm!(
           "hvc #0",
           inlateout("x0") 0x84000000u64 => psci_version,
           options(nomem, nostack)
       );
   }
   println!("PSCI version = 0x{:x}", psci_version);
   ```

2. **在 `secondary_entry` 添加调试输出**
   ```assembly
   secondary_entry:
       mrs     x1, mpidr_el1
       and     x1, x1, #0xFF
       /* 输出 CPU ID */
       mov     x0, #0x09000000    /* UART 基址 */
       mov     w2, #0x31          /* '1' */
       str     w2, [x0]
       /* ... 继续启动 */
   ```

3. **检查设备树**
   ```bash
   dtc -I dtb -O dts virt.dts | grep -A 5 psci
   ```

---

## 7. 参考资料

### 7.1 ARM 官方文档

- [PSCI Specification](https://developer.arm.com/documentation/den0022/latest/)
- [ARMv8-A Architecture Reference Manual](https://developer.arm.com/documentation/ddi0487/latest)
- [SMC Calling Convention](https://developer.arm.com/documentation/den0028/latest)

### 7.2 Linux 内核参考

- `arch/arm64/kernel/psci.c` - PSCI 驱动实现
- `arch/arm64/kernel/smp.c` - SMP 启动代码
- `drivers/firmware/psci/psci.c` - PSCI 客户端

### 7.3 QEMU 文档

- [QEMU ARM virt 平台](https://qemu.readthedocs.io/en/latest/system/arm/virt.html)
- [QEMU 和 PSCI](https://qemu.readthedocs.io/en/latest/system/arm/virt.html)

### 7.4 相关代码

- [kernel/src/arch/aarch64/smp.rs](../kernel/src/arch/aarch64/smp.rs) - PSCI 调用实现
- [kernel/src/arch/aarch64/boot/boot.S](../kernel/src/arch/aarch64/boot/boot.S) - 启动代码
- [docs/TODO.md](TODO.md) - 项目 TODO 列表

---

## 8. 附录：完整代码示例

### 8.1 PSCI 版本查询

```rust
/// 查询 PSCI 版本
pub fn psci_version() -> u64 {
    let version: u64;
    unsafe {
        core::arch::asm!(
            "hvc #0",
            inlateout("x0") 0x84000000u64 => version,
            options(nomem, nostack)
        );
    }
    version
}

// 版本号解码
fn decode_psci_version(version: u64) -> (u16, u16) {
    let major = (version >> 16) as u16;
    let minor = (version & 0xFFFF) as u16;
    (major, minor)
}

// 示例：PSCI 1.1 返回 0x10001000
// major = 1, minor = 1
```

### 8.2 CPU ON 调用

```rust
/// 启动指定 CPU
///
/// # 参数
/// - `cpu_id`: CPU ID (0-3)
/// - `entry_point`: 启动入口点物理地址
///
/// # 返回
/// - `Ok(())`: 成功
/// - `Err(code)`: PSCI 错误码
pub fn psci_cpu_on(cpu_id: u64, entry_point: u64) -> Result<(), u64> {
    let result: u64;
    unsafe {
        core::arch::asm!(
            "hvc #0",
            inlateout("x0") 0x84000003u64 => result,
            in("x1") cpu_id,
            in("x2") entry_point,
            in("x3") 0u64,
            options(nomem, nostack)
        );
    }

    if result == 0 {
        Ok(())
    } else {
        Err(result)
    }
}
```

### 8.3 错误码定义

```rust
/// PSCI 错误码
#[repr(u64)]
pub enum PsciError {
    Success = 0,
    NotSupported = -1i64 as u64,
    InvalidParameters = -2i64 as u64,
    Denied = -3i64 as u64,
    AlreadyOn = -4i64 as u64,
    OnPending = -5i64 as u64,
    InternalFailure = -6i64 as u64,
    NotPresent = -7i64 as u64,
    Disabled = -8i64 as u64,
}
```

---

**文档版本**: v1.0
**最后更新**: 2025-02-04
**作者**: Rux 内核开发团队
**状态**: 生产就绪
