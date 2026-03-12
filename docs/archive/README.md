# Debug Archive (Historical Documents)

This directory contains historical debug records from the project development process. These documents have been archived for reference only.

## Important Notice

**The ARM64 (aarch64) architecture has been removed and is not currently maintained.** Only RISC-V 64-bit architecture is currently supported.

ARM64-related archived documents (gic-smp.md, pscidebug.md, etc.) are for historical reference only.

## Archived Documents

### Memory Management Debugging

#### [MMU Debug Record](mmu-debug.md)
**Archived**: 2025-02-05
**Content**: Debug records during RISC-V Sv39 MMU enabling process
- Page table initialization
- MMU mapping issues
- Access exception debugging
- satp CSR configuration

**Status**: MMU successfully enabled and running

#### [virtio-blk Debug Record](virtio-blk-debugging-summary.md)
**Content**: VirtIO block device driver debugging process

### Interrupt and Multi-core Debugging

#### [GIC+SMP Debug Record](gic-smp.md) (ARM64 - Archived)
**Archived**: 2025-02-05
**Content**: ARM64 GICv3 interrupt controller and SMP debugging

**Status**: ARM64 removed

#### [PSCI Debug Record](pscidebug.md) (ARM64 - Archived)
**Archived**: 2025-02-05
**Content**: ARM64 PSCI debugging

**Status**: ARM64 removed

#### [IPI Test Record](ipi-testing.md)
**Archived**: 2025-02-05
**Content**: Inter-processor interrupt testing

**Status**: RISC-V IPI verified

### User Programs and Context Switching

#### [Linux-style User Program Implementation](linux-style-user-exec.md)
**Implementation Date**: 2025-02-09
**Content**: User program execution using Linux single page table method

**Status**: Implemented

#### [Context Switch Analysis](context-switch-analysis.md)
**Content**: Rux vs Linux context switch comparative analysis
- User/kernel mode detection mechanism
- Stack management strategy
- Kernel context switching

#### [Context Switch Plan](context-switch-plan.md)
**Content**: Context switch implementation plan

#### [Boot Sequence Comparison](boot-sequence-comparison.md)
**Content**: Rux vs Linux boot sequence comparison

### Others

#### [Collection Types Document](collections.md)
**Content**: Custom collection types such as SimpleArc, SimpleVec, etc.

## How to Use These Documents

### Learning Debug Methods

These documents record actual problem debugging processes, suitable for learning:

1. **Problem Localization Methods**
   - How to analyze exception information
   - How to use debugging tools
   - How to narrow down problem scope

2. **Solution Approaches**
   - Comparing with Linux kernel implementation
   - Referencing architecture manuals
   - Systematic verification steps

3. **Debugging Techniques**
   - Adding debug output
   - Using GDB
   - QEMU debug options

### Reference Value

Although these documents describe problems that have been resolved, they still have value:

- Understanding system internals
- Learning debugging methodology
- Understanding architecture details
- Reference for solving similar problems

## Notes

1. **Code may be outdated**: These documents record historical debugging processes, and related code may have been refactored
2. **Problems resolved**: The problems described in the documents have been fixed; do not use as reference for the current system
3. **ARM64 removed**: ARM64-related documents are for historical reference only
4. **For learning purposes only**: These documents are primarily for learning, not documentation for the current system

## Back to Main Documentation

- **[Documentation Home](../README.md)** - Return to documentation center
- **[Getting Started](../guides/getting-started.md)** - Current system usage guide
- **[Development Roadmap](../progress/roadmap.md)** - View latest development status

---

Last updated: 2026-03-04
