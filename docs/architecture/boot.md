# Rux 内核启动流程

本文档描述 Rux 内核从 OpenSBI 到用户态程序的完整启动流程。

**最后更新**：2026-03-04
**架构**：RISC-V 64位 (RV64GC)

---

## 启动流程概览

```
QEMU 启动
    │
    ▼
OpenSBI (M-mode)
    │  初始化硬件、提供 SBI 服务
    ▼
Rux Kernel (S-mode)
    │  内核初始化
    ▼
Init 进程 (U-mode)
    │  Shell / Desktop
    ▼
用户程序
```

---

## 1. OpenSBI 启动 (M-mode)

### 1.1 QEMU 配置

```bash
qemu-system-riscv64 \
    -M virt \
    -cpu rv64 \
    -m 2G \
    -nographic \
    -bios default \          # 使用 QEMU 内置 OpenSBI
    -kernel rux.elf
```

### 1.2 OpenSBI 功能

- 初始化 UART、CLINT、PLIC
- 设置 M-mode trap 处理
- 提供 SBI 调用接口
- 跳转到 S-mode 内核入口

### 1.3 OpenSBI 输出

```
OpenSBI v0.9
   ____                    _____ ____ _____
  / __ \                  / ____|  _ \_   _|
 | |  | |_ __   ___ _ __ | (___ | |_) || |
 | |  | | '_ \ / _ \ '_ \ \___ \|  _ < | |
 | |__| | |_) |  __/ | | |____) | |_) || |_
  \____/| .__/ \___|_| |_|_____/|____/_____|
        | |
        |_|

Platform Name             : riscv-virtio,qemu
Platform HART Count       : 4
Firmware Base             : 0x80000000
Firmware Size             : 128 KB
Domain0 Next Address      : 0x0000000080200000  ← 内核入口
Domain0 Next Mode         : S-mode
```

---

## 2. 内核启动 (S-mode)

### 2.1 汇编入口

**文件**：`kernel/src/arch/riscv64/boot.S`

```asm
.section .init.entry
.global _start

_start:
    # 1. 关闭所有中断
    csrw sie, zero

    # 2. 设置内核栈
    la sp, _stack_top

    # 3. 清零 BSS 段
    la t0, __bss_start
    la t1, __bss_end
1:
    sd zero, 0(t0)
    addi t0, t0, 8
    bne t0, t1, 1b

    # 4. 保存 DTB 指针 (a1 -> s0)
    mv s0, a1

    # 5. 跳转到 Rust 入口
    call rust_main

    # 6. 不应该返回
2:  wfi
    j 2b
```

### 2.2 Rust 主函数

**文件**：`kernel/src/main.rs`

```rust
#[no_mangle]
pub extern "C" fn rust_main(dtb_ptr: usize) -> ! {
    // 1. 控制台初始化
    console::init();

    // 2. 打印启动 Banner
    print_banner();

    // 3. 架构初始化
    arch::arch_init();

    // 4. Trap 初始化
    trap::init();

    // 5. 系统调用初始化
    syscall::init();

    // 6. 堆分配器初始化
    mm::init_heap();

    // 7. 调度器初始化
    sched::init();

    // 8. VFS 初始化
    fs::vfs_init();

    // 9. 设备驱动初始化
    drivers::init();

    // 10. SMP 多核启动
    smp::start_secondary_harts();

    // 11. 启动 init 进程
    init::start_init();

    // 12. 进入调度器主循环
    sched::scheduler_main();
}
```

### 2.3 各子系统初始化

| 步骤 | 模块 | 说明 |
|------|------|------|
| 1 | console | UART ns16550a 驱动 |
| 2 | arch | MMU、页表、CPU 检测 |
| 3 | trap | stvec、sscratch 设置 |
| 4 | syscall | 系统调用分发器 |
| 5 | heap | Buddy + Slab 分配器 |
| 6 | sched | CFS 调度器初始化 |
| 7 | vfs | ramfs、ext4、procfs、devfs |
| 8 | drivers | VirtIO-blk/net/gpu/input |
| 9 | smp | 次核启动 (SBI HSM) |
| 10 | init | 创建 init 进程 (PID 1) |

### 2.4 启动日志

```
██████  ██    ██ ██   ██
██   ██ ██    ██  ██ ██
██████  ██    ██   ███
██   ██ ██    ██  ██ ██
██   ██  ██████  ██   ██

  [ RISC-V 64-bit | POSIX Compatible | v0.1.0 ]

Kernel starting...

Module            Description                        Status
----------------  --------------------------------   --------
console:          UART ns16550a driver               [ok]
smp:              4 CPU(s) online                    [ok]
trap:             stvec handler installed            [ok]
trap:             ecall syscall handler              [ok]
mm:               Sv39 3-level page table            [ok]
mm:               satp CSR configured                [ok]
mm:               buddy allocator order 0-12         [ok]
mm:               heap region 16MB @ 0x80A00000      [ok]
mm:               slab allocator 1MB                 [ok]
boot:             FDT/DTB parsed                     [ok]
mm:               user frame allocator 64MB          [ok]
mm:               16384 page descriptors             [ok]
intc:             PLIC @ 0x0C000000                  [ok]
intc:             external IRQ routing               [ok]
ipi:              SSIP software IRQ                  [ok]
bio:              buffer cache layer                 [ok]
fs:               ext4 driver loaded                 [ok]
fs:               ramfs mounted /                    [ok]
fs:               procfs mounted /proc               [ok]
fs:               devfs mounted /dev                 [ok]
driver:           virtio-blk PCI x1                  [ok]
driver:           virtio-net x1                      [ok]
driver:           virtio-gpu x1                      [ok]
driver:           virtio-input x1                    [ok]
sched:            CFS scheduler v1                   [ok]
trap:             sie.SEIE enabled                   [ok]
init:             loading /bin/shell                 [ok]
init:             ELF loaded to user space           [ok]
init:             init task (PID 1) enqueued         [ok]
```

---

## 3. SMP 多核启动

### 3.1 次核启动流程

**文件**：`kernel/src/arch/riscv64/smp.rs`

```rust
pub fn start_secondary_harts() {
    for hart_id in 1..4 {
        // 使用 SBI HSM 扩展启动次核
        let result = sbi::hart_start(
            hart_id,
            SECONDARY_ENTRY as u64,  // 次核入口地址
            0,                        // 启动参数
        );

        if result.is_ok() {
            println!("smp: hart {} started", hart_id);
        }
    }

    // 等待所有次核就绪
    while SMP_DATA.online_count() < 4 {
        core::hint::spin_loop();
    }
}
```

### 3.2 次核入口

```rust
#[no_mangle]
pub extern "C" fn secondary_start(hart_id: usize) -> ! {
    // 1. 初始化本地数据
    arch::init_per_cpu(hart_id);

    // 2. 初始化 per-CPU 调度器
    sched::init_per_cpu(hart_id);

    // 3. 标记为在线
    SMP_DATA.mark_online(hart_id);

    // 4. 使能中断
    arch::enable_irq();

    // 5. 进入调度器主循环
    sched::scheduler_main();
}
```

---

## 4. Init 进程启动

### 4.1 Init 创建

**文件**：`kernel/src/init.rs`

```rust
pub fn start_init() {
    // 1. 从 ext4 加载 shell ELF
    let elf_data = fs::ext4::read_file("/bin/shell").expect("shell not found");

    // 2. 创建 init 进程
    let init_task = Task::new_user(
        "init",
        &elf_data,
        &["/bin/shell"],
        &[],
    ).expect("failed to create init");

    // 3. 设置 PID 为 1
    assert_eq!(init_task.pid, 1);

    // 4. 加入调度队列
    sched::enqueue(init_task);
}
```

### 4.2 首次用户态切换

**文件**：`kernel/src/arch/riscv64/usermode_asm.S`

```asm
# switch_to_user(entry, stack)
# 从内核态切换到用户态执行第一个用户程序

switch_to_user:
    mv t5, a0              # entry
    mv t6, a1              # user_stack

    # 设置 sstatus.SPP = 0 (返回 U-mode)
    csrr t1, sstatus
    li t0, ~0x100          # 清除 SPP
    and t1, t1, t0
    li t0, 0x20            # 设置 SPIE
    or t1, t1, t0
    csrw sstatus, t1

    # 设置入口点
    csrw sepc, t5

    # 刷新 TLB
    sfence.vma

    # 设置用户栈
    mv sp, t6

    # 返回用户态
    sret
```

---

## 5. 关键初始化顺序

### 5.1 必须遵守的顺序

| 顺序 | 前置条件 | 说明 |
|------|----------|------|
| MMU → PLIC | MMU 先初始化 | PLIC 寄存器需要 MMIO 映射 |
| PLIC → SMP | PLIC 先初始化 | 次核需要处理外部中断 |
| Trap → Scheduler | Trap 先初始化 | 调度器依赖上下文切换 |
| Heap → Scheduler | Heap 先初始化 | 进程结构体需要动态分配 |
| 所有初始化 → IRQ | 初始化完成 | 防止早期中断 |

### 5.2 当前顺序验证

```rust
// ✅ 正确的顺序
arch::arch_init();       // MMU
trap::init();            // Trap
syscall::init();         // 系统调用
mm::init_heap();         // 堆
sched::init();           // 调度器
drivers::init();         // PLIC、VirtIO
smp::start_secondary();  // SMP
init::start_init();      // Init 进程
```

---

## 6. 故障排查

### 6.1 启动失败

**症状**：无输出或立即崩溃

**检查**：
1. OpenSBI 是否正常加载
2. 内核入口地址是否正确 (0x80200000)
3. 栈指针是否有效

### 6.2 MMU 初始化失败

**症状**：Page fault 或非法指令

**检查**：
1. 页表是否正确对齐 (4KB)
2. satp 是否正确设置
3. 内存属性是否正确

### 6.3 SMP 启动失败

**症状**：只有主核工作

**检查**：
1. SBI HSM 是否支持
2. 次核入口地址是否正确
3. per-CPU 数据是否初始化

### 6.4 Init 进程失败

**症状**：无 shell 提示符

**检查**：
1. ext4 是否正确挂载
2. /bin/shell 是否存在
3. ELF 加载是否正确
4. 用户态切换是否成功

---

## 参考资料

- [RISC-V 特权架构规范](https://riscv.org/technical/specifications/)
- [OpenSBI 文档](https://github.com/riscv/opensbi)
- [Linux RISC-V 启动](https://kernel.org/doc/html/latest/riscv/boot.html)

---

**文档版本**：v2.0.0
**最后更新**：2026-03-04
