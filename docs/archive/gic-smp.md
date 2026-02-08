# GIC 和 SMP 调试总结

## 问题背景

在实现 SMP (对称多处理) 支持时，需要初始化 GIC (Generic Interrupt Controller) 以支持 IPI (核间中断)。

## GICv3 地址映射

### QEMU virt 机器的 GIC 地址
- **GICD (Distributor)**: 0x0800_0000
- **GICR (Redistributor)**: 0x0808_0000

### MMU 页表配置

在 [kernel/src/arch/aarch64/mm.rs](../kernel/src/arch/aarch64/mm.rs) 中添加了第三个页表条目：

```rust
// 条目 2：映射 0x0800_0000 - 0x081F_FFFF (2MB，GIC 中断控制器)
let l2_gic_desc = ((0x0800_0000u64 >> 21) & 0x3FFFF_FFFF) << 21 |
                  (1 << 10) |  // AF
                  (3 << 8) |   // SH = Inner shareable
                  (0 << 6) |   // AP = EL1 RW
                  (1 << 2) |   // Device memory (AttrIndx = 1)
                  0b01;        // Block descriptor
(*l2_table).entries[2].value = l2_gic_desc;
```

**页表条目值**: `0x0000000008000705`
- [47:21] = 0x1000 → PA = 0x0800_0000 ✓
- [10] = 1 → AF (Access flag) ✓
- [9:8] = 0b11 → Inner shareable ✓
- [7:6] = 0b00 → EL1 RW ✓
- [5:2] = 0b0001 → AttrIndx = 1 (Device memory) ✓
- [1:0] = 0b01 → Block descriptor ✓

## 问题：GICD 内存访问导致挂起

### 症状
当尝试读取 GICD 寄存器时（例如 GICD_PIDR0 at 0x0800_0FFE），系统完全挂起：
- 无异常输出
- 无错误信息
- 系统停止响应

### 尝试的方案

1. **使用 `read_volatile()`**
   ```rust
   let pidr0 = gicd_ptr.add(0xFFE / 4).read_volatile();
   ```
   **结果**: 系统挂起

2. **使用内联汇编**
   ```rust
   let pidr0: u32;
   core::arch::asm!(
       "ldr {0:w}, [{1}]",
       out(reg) pidr0,
       in(reg) 0x0800_0FFEu32,
       options(nostack, nomem)
   );
   ```
   **结果**: 系统仍然挂起

### 可能的原因

1. **QEMU virt 配置问题**
   - QEMU virt 可能需要特定的 GIC 初始化序列
   - GICD 可能需要先通过系统寄存器启用

2. **MMU 内存属性问题**
   - Device memory 属性 (nGnRnE) 可能不适合 GIC 访问
   - 可能需要使用不同的内存类型

3. **GIC 版本/类型不匹配**
   - 代码假设 GICv3，但 QEMU 可能使用不同版本
   - 需要检查 QEMU 的实际 GIC 配置

4. **访问权限问题**
   - 页表 AP 字段可能不正确
   - 可能需要 EL0 访问权限

## 解决方案：使用系统寄存器实现 IPI

### 关键发现
对于基本的 IPI (SGI - Software Generated Interrupt) 支持，**不需要**完整的 GICD/GICR 初始化。GICv3 提供了系统寄存器接口：

### ICC_SGI1R_EL1 寄存器
用于在 CPU 之间发送 SGI：

```rust
// 在 ipi.rs 中
pub fn send_ipi(target_cpu: u64, ipi_type: IpiType) {
    let sgi = ipi_type.as_sgi();
    let aff0 = target_cpu as u64 & 0xFF;
    let aff1 = 0u64;
    let sgir = (1 << 40) |           // TARGET_LIST 模式
               (aff1 << 16) |         // Aff1 值
               (1u64 << aff0) |       // 目标 CPU 位掩码
               (sgi as u64);          // SGI 中断号

    unsafe {
        core::arch::asm!(
            "msr ICC_SGI1R_EL1, {}",
            in(reg) sgir,
            options(nostack)
        );
    }
}
```

这个系统寄存器访问不需要 MMU 映射的 GICD 地址。

## 当前状态

### 已完成
- ✅ 双核启动 (CPU 0 + CPU 1)
- ✅ MMU 启用 (39-bit VA, 2MB 页表块)
- ✅ GIC 内存区域映射到页表 (Entry 2)
- ✅ GIC 最小初始化（跳过 GICD/GICR）
- ✅ IPI 模块框架 (使用 ICC_SGI1R_EL1)

### 待完成
- ⏳️ 测试 IPI 发送和接收
- ⏳️ 实现 SGI 中断处理
- ⏳️ Per-CPU 运行队列
- ⏳️ 调度器多核优化

### 待调试
- ❌ GICD 内存访问问题（挂起）
  - 需要调试 QEMU virt 的 GIC 配置
  - 可能需要不同的内存属性
  - 可能需要先通过其他方式启用 GIC

## 代码文件

### 修改的文件
- [kernel/src/arch/aarch64/mm.rs](../kernel/src/arch/aarch64/mm.rs) - 添加 GIC 区域映射
- [kernel/src/drivers/intc/gicv3.rs](../kernel/src/drivers/intc/gicv3.rs) - 最小初始化
- [kernel/src/main.rs](../kernel/src/main.rs) - 启用 GIC 初始化

### 相关文件
- [kernel/src/arch/aarch64/ipi.rs](../kernel/src/arch/aarch64/ipi.rs) - IPI 实现
- [kernel/src/arch/aarch64/smp.rs](../kernel/src/arch/aarch64/smp.rs) - SMP 框架

## 下一步工作

### 短期 (Phase 3 完成)
1. 实现完整的 IPI 测试
2. 添加 SGI 中断处理到 trap.rs
3. 验证 CPU 0 → CPU 1 IPI 通信

### 中期 (Phase 2)
1. 修改调度器为 per-CPU 运行队列
2. 实现 CPU 亲和性
3. 添加负载均衡基础

### 长期 (Phase 4)
1. 完整的调度器多核优化
2. 高级负载均衡策略
3. NUMA 支持（如需要）

## 参考文档

- [ARM GICv3 Architecture Specification](https://developer.arm.com/documentation/ihi0069/latest/)
- [QEMU virt machine documentation](https://www.qemu.org/docs/master/system/arm/virt.html)
- [Linux kernel GIC driver](https://elixir.bootlin.com/linux/latest/source/drivers/irqchip/irq-gic-v3.c)

## 调试日志示例

```
MM: L2 entry 2 value = 0x0000000008000705
...
GIC: Starting GICv3 initialization...
GIC: Skipping full GIC initialization (MMU access issue)
GIC: IPI uses ICC_SGI1R_EL1 system register (no GICD init needed)
GIC: Minimal init complete (IPI ready)
...
SMP: 2 CPUs online
```

## 总结

虽然无法完整初始化 GICD/GICR（由于内存访问问题），但我们成功实现了：
1. **MMU 页表配置**：正确映射 GIC 物理地址
2. **最小化 IPI 支持**：使用系统寄存器接口
3. **双核运行**：两个 CPU 都正常运行

这为进一步的多核开发奠定了基础。GICD 访问问题可以在后续调试中解决，使用 GDB 或 QEMU monitor 来检查实际的硬件状态。
