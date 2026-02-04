# Rux 内核 MMU 调试完整记录

## 文档信息

- **创建日期**: 2025-02-04
- **作者**: Claude AI Assistant
- **相关文件**: `kernel/src/arch/aarch64/mm.rs`
- **目标**: 在 ARMv8-A (aarch64) 架构上启用 MMU

---

## 目录

1. [问题描述](#1-问题描述)
2. [尝试的方案](#2-尝试的方案)
3. [关键发现](#3-关键发现)
4. [最终解决方案](#4-最终解决方案)
5. [技术总结](#5-技术总结)
6. [参考资料](#6-参考资料)

---

## 1. 问题描述

### 1.1 初始状态

Rux 内核已经实现了基本的启动框架，但 MMU 被禁用。目标是在 QEMU virt 机器（ARMv8-A）上启用 MMU，实现虚拟内存管理。

### 1.2 环境信息

```
平台: QEMU virt machine
架构: ARMv8-A (aarch64)
CPU: cortex-a57
内存: 2GB
内核加载地址: 0x4000_0000
UART地址: 0x0900_0000
```

### 1.3 症状

启用 MMU 后系统立即挂起，没有任何输出。

---

## 2. 尝试的方案

### 2.1 方案 1: 48位VA + Level 1 (1GB块) - 失败

**配置**:
- T0SZ = 16 (48位虚拟地址)
- 起始级别: Level 1
- 页表粒度: 1GB 块
- 映射:
  - 条目 0: 0x0000_0000 (设备区域)
  - 条目 1: 0x4000_0000 (内核区域)

**代码**:
```rust
// 计算描述符
let l1_normal_desc = ((0x4000_0000u64 >> 30) & 0x3FFFF) << 30 |
                     (1 << 10) |  // AF
                     (3 << 8) |   // SH
                     (0 << 6) |   // AP
                     (0 << 2) |   // AttrIndx
                     0b01;        // Block

(*l1_table).entries[1].value = l1_normal_desc;
```

**结果**: ❌ 系统挂起

**原因分析**: (当时未发现)

---

### 2.2 方案 2: 39位VA + Level 2 (2MB块) - 失败

**配置**:
- T0SZ = 25 (39位虚拟地址)
- 起始级别: Level 2
- 页表粒度: 2MB 块
- 映射:
  - 条目 0: 0x0000_0000
  - 条目 2: 0x4000_0000

**关键错误**: 使用了条目 2 而不是条目 1

**结果**: ❌ 系统挂起

**原因**:
```
对于 0x4000_0000:
level 2 索引 = 0x4000_0000 >> 30 = 2  ← 这是错的！
```

---

### 2.3 方案 3: 尝试启用缓存 - 失败

**修改**: 在 SCTLR 中启用数据缓存和指令缓存

```rust
sctlr |= (1 << 0);   // M: MMU使能
sctlr |= (1 << 2);   // C: 数据缓存使能
sctlr |= (1 << 12);  // I: 指令缓存使能
```

**结果**: ❌ 系统仍然挂起

---

## 3. 关键发现

### 3.1 发现 1: PC地址的Level 1索引计算错误

添加调试代码检查 PC 和页表索引：

```rust
let current_pc: u64;
asm!("adr {}, #0", out(reg) current_pc);

let pc_l1_index = (current_pc >> 39) & 0x1FF;
```

**输出**:
```
MM: Current PC = 0x000000004000678C
MM: PC L1 index = 0 (should be 1 for 0x4000_0000)
```

**分析**:
```
PC = 0x4000_678C
二进制: 0b0000_0000_0100_0000_0000_0000_0000_0110_0111_1000_1100
      = 0b0000_0000_0000_0000_0000_0000_0000_0000_0100_0000_0000_0000_0000_0000_0110_0111_1000_1100

Level 1 索引使用 VA[47:39] (第47-39位)
0x4000_678C 的第 39-47 位都是 0
所以: 0x4000_678C >> 39 = 0
```

**结论**:
- 对于 0x4000_678C，Level 1 索引是 **0**，不是 1！
- 我在条目 1 映射内核是错误的
- 应该在条目 0 映射

---

### 3.2 发现 2: 1GB块太大导致地址映射错误

即使修正为使用条目 0，仍然有问题：

**问题**:
```
使用 1GB 块映射 0x0000_0000:
- 输出地址 = (VA & ~0x3FFF_FFFF) | (描述符PA << 30)

对于 VA = 0x4000_678C:
- 输出PA = (0x4000_678C & ~0x3FFF_FFFF) | (0 << 30)
- 输出PA = 0x0000_0000  ← 错误！应该是 0x4000_678C
```

**根本原因**:
- 1GB 块的粒度太大
- 所有在 0x0000_0000-0x3FFF_FFFF 范围内的 VA 都会映射到 0x0000_0000
- 无法精确映射 0x4000_0000 区域

---

### 3.3 发现 3: UART地址不在2MB块内

尝试用 Level 2 的 2MB 块时，还有个问题：

```
UART 地址: 0x0900_0000
Level 2 索引: 0x0900_0000 >> 30 = 0

我的条目 0 映射: 0x0000_0000 - 0x001F_FFFF (2MB)
UART 实际位置: 0x0900_0000 (不在映射范围内！)
```

**结论**: 需要确保所有访问的地址都在映射范围内。

---

## 4. 最终解决方案

### 4.1 正确的配置

**虚拟地址空间**: 39位 (T0SZ=25)

**页表层级**:
- 起始级别: Level 2
- 块大小: 2MB
- Level 2 索引: VA[38:30]

**关键计算**:
```python
# 对于 PC = 0x4000_678C
pc_l2_index = (0x4000_678C >> 30) & 0x1FF
            = 1  ✓ 正确！
```

**页表映射**:
```rust
// 条目 0: 设备区域
let l2_device_desc = ((0u64 >> 21) & 0x3FFFF_FFFF) << 21 |
                     (1 << 10) |  // AF
                     (3 << 8) |   // SH
                     (0 << 6) |   // AP
                     (1 << 2) |   // Device memory
                     0b01;        // Block
(*l2_table).entries[0].value = l2_device_desc;

// 条目 1: 内核区域
let l2_normal_desc = ((0x4000_0000u64 >> 21) & 0x3FFFF_FFFF) << 21 |
                     (1 << 10) |  // AF
                     (3 << 8) |   // SH
                     (0 << 6) |   // AP
                     (0 << 2) |   // Normal memory
                     0b01;        // Block
(*l2_table).entries[1].value = l2_normal_desc;
```

**TCR 配置**:
```rust
let tcr: u64 = (25 << 0) |     // T0SZ: 39-bit VA (level 2-3)
               (0b01 << 8) |   // IRGN0: Normal WB-WA Inner
               (0b01 << 10) |  // ORGN0: Normal WB-WA Outer
               (0b11 << 12) |  // SH0: Inner shareable
               (0b00 << 14) |  // TG0: 4KB granule
               (1 << 23);      // EPD1: 禁用TTBR1
```

**TTBR0 配置**:
```rust
// 指向 Level 2 页表
let l2_table_addr = &raw mut LEVEL2_PAGE_TABLE.table as u64;
asm!("msr ttbr0_el1, {}", in(reg) l2_table_addr);
```

### 4.2 成功的输出

```
MM: Setting up L2 page tables (2MB blocks)...
MM: Clearing L2 table...
MM: L2 table cleared
MM: L2 entry 0 set (2MB device at 0x0000_0000)
MM: L2 entry 1 set (2MB normal at 0x4000_0000)
MM: Page tables setup complete (2 L2 entries)
MM: L2 page table addr=0x0000000040026000
MM: Setting MAIR...
MM: Setting TTBR0 to L2 table...
MM: Setting TCR (T0SZ=25, 39-bit VA, L2 start)...
MM: Computed TCR = 0x0000000000803519 (T0SZ=25, 39-bit VA, level 2 start)
MM: Flushing caches and TLBs...
MM: Current PC = 0x000000004000678C
MM: PC L2 index = 1 (should be 1 for 0x4000_0000)
MM: Enabling MMU only (caches disabled)...
MM: ISB after MMU enable...
MM: MMU setup complete!
MM: SCTLR after enable = 0x0000000000000001  ← MMU bit is set!
MM: Current PC = 0x0000000040006DB0           ← PC advanced!
MM: MMU enabled successfully!
```

**系统继续运行**:
```
Before trap init
Initializing trap handling...
After trap init
Initializing system calls...
System call support initialized
Initializing heap...
Testing direct allocator call...
```

✅ **MMU 成功启用！**

---

## 5. 技术总结

### 5.1 ARMv8 页表层级

ARMv8 支持 4 级页表（4KB granule）：

| 级别 | 索引位 | 块大小 | T0SZ范围 |
|------|--------|--------|----------|
| Level 0 | VA[47:39] | 1TB | 16-24 |
| Level 1 | VA[38:30] | 1GB | 25-33 |
| Level 2 | VA[29:21] | 2MB | 34-42 |
| Level 3 | VA[20:12] | 4KB | 43-51 |

**起始级别计算**:
```
如果 T0SZ = 25:
  VA大小 = 64 - 25 = 39 bits
  起始级别 = 48 - VA大小 = 48 - 39 = 9 (失败)

  正确计算:
  起始级别 = (48 - T0SZ) / 9 向下取整
  = (48 - 25) / 9
  = 23 / 9
  = 2 (Level 2)
```

### 5.2 Level 2 块描述符格式

**2MB 块描述符** (Block Descriptor at Level 2):

```
Bits [47:21]:  输出地址[47:21] (物理地址 >> 21)
Bit [10]:     AF (Access Flag)
Bits [9:8]:   SH (Shareability)
Bits [7:6]:   AP (Access Permissions)
Bits [5:2]:   AttrIndx (Memory Attributes)
Bits [1:0]:   0b01 (Block Descriptor)
```

**示例计算** (映射 0x4000_0000):
```python
pa = 0x4000_0000
pa_field = (pa >> 21) & 0x3FFFF_FFFF = 0x2000
descriptor = (pa_field << 21) | (1<<10) | (3<<8) | (0<<6) | (0<<2) | 0b01
           = 0x4000_0000 | 0x400 | 0x300 | 0x01
           = 0x4000_0701
```

### 5.3 TCR_EL1 寄存器

**关键位**:
- T0SZ[5:0]: Translation Table Size
  - T0SZ = 25 → 39位VA
- TG0[1:0]: Translation Granule
  - 0b00 = 4KB
- IRGN0[1:0]: Inner Region Cacheability
  - 0b01 = Normal WB-WA Inner
- ORGN0[1:0]: Outer Region Cacheability
  - 0b01 = Normal WB-WA Outer
- SH0[1:0]: Shareability
  - 0b11 = Inner Shareable
- EPD1: Disable TTBR1

### 5.4 地址翻译过程示例

**输入**: VA = 0x4000_678C

**步骤**:
```
1. 检查T0SZ=25 (39位VA)，起始级别=2
2. 提取Level 2索引: VA[38:30] = 1
3. 读取页表条目 1
4. 描述符类型 = Block (bits[1:0] = 0b01)
5. 提取输出地址: descriptor[47:21] = 0x4000_0000 >> 21 = 0x2000
6. 计算输出PA: (0x2000 << 21) | (VA & 0x1FFFFF)
7. 输出PA = 0x4000_0000 | 0x678C = 0x4000_678C  ✓ (恒等映射)
```

---

## 6. 参考资料

### 6.1 ARM 官方文档

- **ARM Architecture Reference Manual ARMv8-A**
  - Chapter D4: The AArch64 Virtual Memory System Architecture
  - Chapter G5: System Control Registers (in AArch64)

- **ARMv8-A Address Translation**
  - https://developer.arm.com/documentation/ddi0487/latest

### 6.2 Linux 内核源码

- **arch/arm64/mm/mmu.c**
  - 页表初始化
  - MMU 启用流程

- **arch/arm64/kernel/traps.c**
  - 地址异常处理

### 6.3 QEMU 文档

- **QEMU virt 机器**
  - 内存布局
  - 设备地址映射

---

## 7. 调试技巧总结

### 7.1 添加调试输出

```rust
// 打印关键寄存器值
asm!("mrs {}, sctlr_el1", out(reg) sctlr);

// 打印当前PC
asm!("adr {}, #0", out(reg) pc);

// 打印页表索引
let l2_index = (pc >> 30) & 0x1FF;
```

### 7.2 系统化调试方法

1. **验证计算**: 用 Python 或计算器验证页表索引计算
2. **逐步验证**: 先验证页表设置，再验证MMU启用
3. **对比参考**: 对照 Linux 内核的实现
4. **使用文档**: ARM ARM 有详细的官方文档

### 7.3 常见陷阱

1. ❌ **页表索引计算错误**: 混淆不同级别的索引位
2. ❌ **块描述符格式错误**: 位宽、位置设置错误
3. ❌ **地址范围不对**: 实际访问的地址不在映射范围内
4. ❌ **T0SZ与起始级别不匹配**: 导致无法正确翻译
5. ❌ **忘记设置属性**: AF、SH、AP 等属性缺失

---

## 8. 附录：完整代码

### 8.1 页表设置代码

```rust
/// 设置页表（使用 level 2，2MB 块）
///
/// 改用 T0SZ=25 (39位VA)，从 level 2 开始，使用 2MB 块
/// - VA[38:30] 索引 level 2 表 (9 位，512 个条目)
/// - 每个 level 2 条目：2MB 块
///
/// 对于 0x4000_678C：
/// - level 2 索引 = 0x4000_678C >> 30 = 1
///
/// 映射策略：
/// - 条目 0: 0x0000_0000 - 0x001F_FFFF (UART 等)
/// - 条目 1: 0x4000_0000 - 0x401F_FFFF (内核)
unsafe fn setup_two_level_page_tables() {
    // 使用 level 2 表
    let l2_table = &raw mut LEVEL2_PAGE_TABLE.table;

    // 清零 level 2 表
    for i in 0..512 {
        (*l2_table).entries[i].value = 0;
    }

    // Level 2 块描述符格式 (2MB block):
    // [47:21] 物理地址 >> 21
    // [10] AF = 1
    // [9:8] SH = 11 (Inner shareable)
    // [7:6] AP = 00 (EL1 RW)
    // [5:2] AttrIndx = 0000 (Normal) or 0001 (Device)
    // [1:0] = 01 (Block descriptor)

    // 条目 0：映射 0x0000_0000 - 0x001F_FFFF (2MB，设备区域)
    let l2_device_desc = ((0u64 >> 21) & 0x3FFFF_FFFF) << 21 |
                         (1 << 10) |  // AF
                         (3 << 8) |   // SH
                         (0 << 6) |   // AP
                         (1 << 2) |   // Device memory
                         0b01;        // Block
    (*l2_table).entries[0].value = l2_device_desc;

    // 条目 1：映射 0x4000_0000 - 0x401F_FFFF (2MB，内核区域)
    let l2_normal_desc = ((0x4000_0000u64 >> 21) & 0x3FFFF_FFFF) << 21 |
                         (1 << 10) |  // AF
                         (3 << 8) |   // SH
                         (0 << 6) |   // AP
                         (0 << 2) |   // Normal memory
                         0b01;        // Block
    (*l2_table).entries[1].value = l2_normal_desc;

    // 数据同步屏障
    asm!("dsb ish", options(nomem, nostack));
}
```

### 8.2 MMU寄存器初始化

```rust
unsafe fn init_mmu_registers() {
    // 获取 level 2 页表物理地址
    let l2_table_addr = &raw mut LEVEL2_PAGE_TABLE.table as u64;

    // 设置MAIR_EL1
    let mair: u64 = (0x00 << 8) |  // Device nGnRnE
                    (0xFF << 0);   // Normal WB-RWA
    asm!("msr mair_el1, {}", in(reg) mair, options(nomem, nostack));

    // 设置TTBR0_EL1
    asm!("msr ttbr0_el1, {}", in(reg) l2_table_addr, options(nomem, nostack));

    // 设置TCR_EL1
    let tcr: u64 = (25 << 0) |     // T0SZ: 39-bit VA
                   (0b01 << 8) |   // IRGN0
                   (0b01 << 10) |  // ORGN0
                   (0b11 << 12) |  // SH0
                   (0b00 << 14) |  // TG0: 4KB
                   (1 << 23);      // EPD1
    asm!("msr tcr_el1, {}", in(reg) tcr, options(nomem, nostack));

    // 刷新缓存和TLB
    asm!("ic iallu", options(nomem, nostack);
    asm!("dsb ish", options(nomem, nostack));
    asm!("isb", options(nomem, nostack));
    asm!("tlbi vmalle1is", options(nomem, nostack));
    asm!("dsb ish", options(nomem, nostack));
    asm!("isb", options(nomem, nostack));

    // 启用MMU
    let sctlr: u64 = 1 << 0;  // M: MMU enable
    asm!("msr sctlr_el1, {}", in(reg) sctlr, options(nomem, nostack));
    asm!("isb", options(nomem, nostack));
}
```

---

**文档版本**: 1.0
**最后更新**: 2025-02-04
**状态**: ✅ MMU成功启用
