# Rux 内核启动顺序分析与优化

## 当前启动顺序（2025-02-09）

```
_start() [kernel/src/main.rs]
├── 1. console::init()                     // UART 控制台
├── 2. arch::arch_init()                   // 架构初始化
│   ├── boot::init()                       // 基础引导
│   └── mm::init()                         // MMU 初始化 (Sv39)
├── 3. trap::init()                        // 异常向量表
├── 4. init_syscall()                      // 系统调用
├── 5. init_heap()                         // 堆分配器
├── 6. sched::init()                       // 调度器
├── 7. vfs_init()                          // VFS
├── 8. drivers::init()                     // 设备驱动 (PLIC/CLINT)
├── 9. SMP boot                           // 启动次核
└── 10. IRQ enable                         // 使能 IRQ
```

## Linux RISC-V 启动顺序参考

基于 Linux 5.x 内核（arch/riscv/kernel/setup.c）：

```
start_kernel() [kernel/sched/core.c]
├── 1. setup_arch()                        // 架构初始化
│   ├── smp_setup_processor_id()          // CPU ID detection (hartid)
│   ├── setup_machine_fdt()               // Device Tree
│   ├── riscv_memblock_init()             // Early memory management
│   ├── paging_init()                     // MMU initialization (Sv39) ✓
│   └── bootmem_init()                    // Boot memory allocator
├── 2. trap_init()                         // Early exception handlers
├── 3. early_irq_init()                    // Early interrupt init (data only)
├── 4. init_IRQ()                          // Full interrupt controller init (PLIC) ✓
├── 5. sched_init()                        // Scheduler initialization
├── 6. mm_init()                           // Memory management init
│   ├── mem_init()                        // Memory allocator
│   └── kmem_cache_init()                 // Slab allocator
├── 7. early_init_irq_lock()              // Initialize IRQ locks
├── 8. rest_init()                         // Late init
│   ├── rcu_init()                        // RCU synchronization
│   ├── early SMP boot                    // Secondary CPUs (SBI)
│   └── late time init                    // Timer initialization (CLINT)
```

## 关键原则

### 1. MMU 必须在 PLIC 之前初始化
**原因**：
- PLIC 寄存器访问需要 MMU 映射
- Device memory 属性需要正确设置
- Linux: `paging_init()` → `init_IRQ()`

**当前状态**: ✅ 正确
```rust
arch_init() {
    boot::init();
    mm::init();  // MMU before PLIC ✓
}
// ... later ...
drivers::intc::init();  // PLIC after MMU ✓
```

### 2. PLIC 必须在 SMP 之前初始化
**原因**：
- 次核启动需要 IPI (Inter-Processor Interrupt)
- SBI 调用可能在次核上触发中断
- 次核需要 PLIC 来接收 SGI (Software Generated Interrupt)

**当前状态**: ✅ 正确
```rust
drivers::intc::init();  // PLIC first
// ... later ...
boot_secondary_cpus(); // SMP after PLIC ✓
```

### 3. 异常处理必须在 MMU 之后
**原因**：
- 异常向量表需要 MMU 映射
- stvec 写入需要在 MMU 启用后
- 异常处理可能访问虚拟内存

**当前状态**: ✅ 正确
```rust
arch_init() {      // Includes MMU init
    mm::init();
}
trap::init();       // After MMU ✓
```

### 4. IRQ 必须在所有初始化完成后才使能
**原因**：
- 防止早期中断处理未初始化的子系统
- 避免 interrupt storm
- Linux: 在 `rest_init()` 的最后才使能 IRQ

**当前状态**: ✅ 正确
```rust
// All init complete
unsafe { asm!("msr daifclr, #2"); }  // Enable IRQ last ✓
```

## 优化建议

### 🔴 严重问题：次核初始化顺序不正确

**当前问题**：
次核在 `secondary_entry` 中直接进入 WFI，但：
1. 没有初始化 per-CPU 运行队列
2. 没有设置 per-CPU 栈
3. 没有初始化 per-CPU 定时器

**建议修复**：
```rust
// arch/aarch64/smp.rs
pub unsafe extern "C" fn secondary_cpu_start() -> ! {
    let cpu_id = get_core_id();

    // 1. 设置 per-CPU 栈
    setup_per_cpu_stack(cpu_id);

    // 2. 初始化 per-CPU 运行队列
    crate::process::sched::init_per_cpu_rq(cpu_id as usize);

    // 3. 初始化 per-CPU 定时器
    // TODO: timer::init_per_cpu(cpu_id);

    // 4. 使能 per-CPU IRQ
    asm!("msr daifclr, #2");

    // 5. 进入空闲循环
    loop {
        asm!("wfi");
    }
}
```

### 🟡 中等问题：GIC 初始化时机

**当前代码**：
```rust
// 在 sched_init() 之后初始化 GIC
process::sched::init();
crate::fs::vfs_init();
drivers::intc::init();
```

**建议调整**：
```rust
// GIC 应该在更早的位置，但在 MMU 之后
arch_init();           // MMU
trap_init();           // Exception handling
init_syscall();       // System calls
drivers::intc::init(); // GIC ← 移到这里
init_heap();          // Heap
process::sched::init(); // Scheduler
```

**原因**：
- GIC 是基础硬件设施，应尽早初始化
- 但不依赖 heap 或 scheduler
- 参考 Linux: `trap_init()` → `init_IRQ()` → `sched_init()`

### 🟢 低优先级：初始化日志优化

**当前问题**：
- 混合使用 `println!` 和 `debug_println!`
- 启动信息不一致

**建议**：
```rust
// 使用统一的日志宏
log_info!("Initializing architecture...");
log_info!("MMU enabled");
log_info!("GIC initialized");
log_info!("SMP: {} CPUs online", active);
```

## 优化后的启动顺序

```
_start() [优化后]
├── 1. console::init()                     // UART (very early)
├── 2. arch::arch_init()                   // Architecture
│   ├── boot::init()                       // Boot setup, disable IRQ
│   └── mm::init()                         // MMU ✓
├── 3. trap::init()                        // Exception vectors
├── 4. init_syscall()                      // System calls
├── 5. drivers::intc::init()               // GIC ← 提前到这里
│   └── 保持 IRQ 禁用状态
├── 6. init_heap()                         // Heap allocator
├── 7. sched::init()                       // Scheduler (CPU 0 only)
│   └── init_per_cpu_rq(0)                 // Initialize CPU 0 runqueue
├── 8. vfs_init()                          // VFS
├── 9. SMP boot                            // Secondary CPUs
│   ├── SmpData::init(2)
│   └── boot_secondary_cpus()
│       └── secondary_cpu_start()         // 次核入口
│           ├── setup_per_cpu_stack()      // ← Per-CPU stack
│           ├── init_per_cpu_rq(cpu_id)   // ← Per-CPU runqueue
│           └── enable IRQ                // ← Per-CPU IRQ
└── 10. IRQ enable                         // CPU 0 enables IRQ
    └── asm!("msr daifclr, #2")
```

## 次核初始化详细步骤

```rust
// arch/aarch64/boot.S
secondary_entry:
    mrs     x1, mpidr_el1
    and     x1, x1, #0xFF        // Get CPU ID
    cbz     x1, __boot_start    // CPU 0 goes to normal boot

    // === 次核启动序列 ===
    // 1. 设置 per-CPU 栈
    mrs     x1, mpidr_el1
    and     x1, x1, #0xFF
    ldr     x2, =per_cpu_stacks
    lsl     x1, x1, #14          // Each stack = 16KB
    add     sp, x2, x1
    add     sp, sp, #0x4000      // Stack top

    // 2. 跳转到 Rust 初始化
    bl      secondary_cpu_init

spin_wait:
    wfe
    b       spin_wait

// arch/aarch64/smp.rs
#[no_mangle]
pub unsafe extern "C" fn secondary_cpu_init() {
    let cpu_id = get_core_id();

    // 3. 初始化 per-CPU 运行队列
    crate::process::sched::init_per_cpu_rq(cpu_id as usize);

    // 4. 初始化 per-CPU GIC (GICR)
    // TODO: gic::init_per_cpu(cpu_id);

    // 5. 使能本核 IRQ
    asm!("msr daifclr, #2", options(nomem, nostack));

    // 6. 标记为运行中
    SmpData::mark_cpu_running(cpu_id);

    // 7. 进入空闲循环
    loop {
        asm!("wfi", options(nomem, nostack));
        // TODO: 检查调度器是否有任务
    }
}
```

## 验证检查清单

- [ ] MMU 在 GIC 之前初始化
- [ ] GIC 在 SMP 之前初始化
- [ ] 异常处理在 MMU 之后
- [ ] IRQ 在所有初始化完成后使能
- [ ] 次核有独立的 per-CPU 栈
- [ ] 次核初始化 per-CPU 运行队列
- [ ] 次核初始化 per-CPU GIC (GICR)
- [ ] 内存屏障正确使用
- [ ] 次核正确进入空闲状态

## 参考资料

- Linux 内核: arch/arm64/kernel/setup.c
- Linux 内核: arch/arm64/kernel/smp.c
- ARMv8 Architecture Reference Manual
- GICv3 Specification (ARM IHI 0069)
