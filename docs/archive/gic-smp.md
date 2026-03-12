# GIC and SMP Debugging Summary

## Background

When implementing SMP (Symmetric Multi-Processing) support, the GIC (Generic Interrupt Controller) needs to be initialized to support IPI (Inter-Processor Interrupts).

## GICv3 Address Mapping

### QEMU virt Machine GIC Addresses
- **GICD (Distributor)**: 0x0800_0000
- **GICR (Redistributor)**: 0x0808_0000

### MMU Page Table Configuration

Added a third page table entry in [kernel/src/arch/aarch64/mm.rs](../kernel/src/arch/aarch64/mm.rs):

```rust
// Entry 2: Map 0x0800_0000 - 0x081F_FFFF (2MB, GIC interrupt controller)
let l2_gic_desc = ((0x0800_0000u64 >> 21) & 0x3FFFF_FFFF) << 21 |
                  (1 << 10) |  // AF
                  (3 << 8) |   // SH = Inner shareable
                  (0 << 6) |   // AP = EL1 RW
                  (1 << 2) |   // Device memory (AttrIndx = 1)
                  0b01;        // Block descriptor
(*l2_table).entries[2].value = l2_gic_desc;
```

**Page Table Entry Value**: `0x0000000008000705`
- [47:21] = 0x1000 -> PA = 0x0800_0000
- [10] = 1 -> AF (Access flag)
- [9:8] = 0b11 -> Inner shareable
- [7:6] = 0b00 -> EL1 RW
- [5:2] = 0b0001 -> AttrIndx = 1 (Device memory)
- [1:0] = 0b01 -> Block descriptor

## Issue: GICD Memory Access Causes Hang

### Symptoms
When attempting to read GICD registers (e.g., GICD_PIDR0 at 0x0800_0FFE), the system completely hangs:
- No exception output
- No error messages
- System stops responding

### Attempted Solutions

1. **Using `read_volatile()`**
   ```rust
   let pidr0 = gicd_ptr.add(0xFFE / 4).read_volatile();
   ```
   **Result**: System hangs

2. **Using inline assembly**
   ```rust
   let pidr0: u32;
   core::arch::asm!(
       "ldr {0:w}, [{1}]",
       out(reg) pidr0,
       in(reg) 0x0800_0FFEu32,
       options(nostack, nomem)
   );
   ```
   **Result**: System still hangs

### Possible Causes

1. **QEMU virt Configuration Issue**
   - QEMU virt may require a specific GIC initialization sequence
   - GICD may need to be enabled via system registers first

2. **MMU Memory Attribute Issue**
   - Device memory attribute (nGnRnE) may not be suitable for GIC access
   - May need to use a different memory type

3. **GIC Version/Type Mismatch**
   - Code assumes GICv3, but QEMU may use a different version
   - Need to check QEMU's actual GIC configuration

4. **Access Permission Issue**
   - Page table AP field may be incorrect
   - May need EL0 access permissions

## Solution: Using System Registers to Implement IPI

### Key Discovery
For basic IPI (SGI - Software Generated Interrupt) support, full GICD/GICR initialization is **not required**. GICv3 provides a system register interface:

### ICC_SGI1R_EL1 Register
Used to send SGI between CPUs:

```rust
// In ipi.rs
pub fn send_ipi(target_cpu: u64, ipi_type: IpiType) {
    let sgi = ipi_type.as_sgi();
    let aff0 = target_cpu as u64 & 0xFF;
    let aff1 = 0u64;
    let sgir = (1 << 40) |           // TARGET_LIST mode
               (aff1 << 16) |         // Aff1 value
               (1u64 << aff0) |       // Target CPU bitmask
               (sgi as u64);          // SGI interrupt number

    unsafe {
        core::arch::asm!(
            "msr ICC_SGI1R_EL1, {}",
            in(reg) sgir,
            options(nostack)
        );
    }
}
```

This system register access does not require MMU-mapped GICD addresses.

## Current Status

### Completed
- Dual-core boot (CPU 0 + CPU 1)
- MMU enabled (39-bit VA, 2MB page table blocks)
- GIC memory region mapped to page table (Entry 2)
- GIC minimal initialization (skipping GICD/GICR)
- IPI module framework (using ICC_SGI1R_EL1)

### Pending
- Test IPI send and receive
- Implement SGI interrupt handling
- Per-CPU run queues
- Scheduler multi-core optimization

### To Debug
- GICD memory access issue (hang)
  - Need to debug QEMU virt's GIC configuration
  - May need different memory attributes
  - May need to enable GIC through other means first

## Code Files

### Modified Files
- [kernel/src/arch/aarch64/mm.rs](../kernel/src/arch/aarch64/mm.rs) - Added GIC region mapping
- [kernel/src/drivers/intc/gicv3.rs](../kernel/src/drivers/intc/gicv3.rs) - Minimal initialization
- [kernel/src/main.rs](../kernel/src/main.rs) - Enable GIC initialization

### Related Files
- [kernel/src/arch/aarch64/ipi.rs](../kernel/src/arch/aarch64/ipi.rs) - IPI implementation
- [kernel/src/arch/aarch64/smp.rs](../kernel/src/arch/aarch64/smp.rs) - SMP framework

## Next Steps

### Short-term (Phase 3 Completion)
1. Implement complete IPI testing
2. Add SGI interrupt handling to trap.rs
3. Verify CPU 0 -> CPU 1 IPI communication

### Medium-term (Phase 2)
1. Modify scheduler to use per-CPU run queues
2. Implement CPU affinity
3. Add basic load balancing

### Long-term (Phase 4)
1. Complete scheduler multi-core optimization
2. Advanced load balancing strategies
3. NUMA support (if needed)

## Reference Documentation

- [ARM GICv3 Architecture Specification](https://developer.arm.com/documentation/ihi0069/latest/)
- [QEMU virt machine documentation](https://www.qemu.org/docs/master/system/arm/virt.html)
- [Linux kernel GIC driver](https://elixir.bootlin.com/linux/latest/source/drivers/irqchip/irq-gic-v3.c)

## Debug Log Example

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

## Summary

Although we could not fully initialize GICD/GICR (due to memory access issues), we successfully implemented:
1. **MMU Page Table Configuration**: Correctly mapped GIC physical addresses
2. **Minimal IPI Support**: Using system register interface
3. **Dual-core Operation**: Both CPUs are running normally

This lays the foundation for further multi-core development. The GICD access issue can be resolved in subsequent debugging using GDB or QEMU monitor to inspect actual hardware state.
