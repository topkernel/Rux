# RISC-V 64位架构实现文档

本文档详细记录 Rux 内核在 RISC-V 64位架构上的实现细节。

**最后更新**：2026-03-04
**状态**：✅ 完全实现，唯一支持的架构

---

## 目录

- [架构概述](#架构概述)
- [内存布局](#内存布局)
- [启动流程](#启动流程)
- [异常处理](#异常处理)
- [系统调用](#系统调用)
- [CPU 操作](#cpu-操作)
- [设备驱动](#设备驱动)
- [多核支持](#多核支持)
- [参考资料](#参考资料)

---

## 架构概述

### RISC-V 特权级

RISC-V 定义了三个特权级（从低到高）：

1. **U-mode (User)** - 用户应用程序
2. **S-mode (Supervisor)** - 操作系统内核
3. **M-mode (Machine)** - 固件/引导程序

**Rux 的实现**：
- **OpenSBI** 运行在 M-mode
- **Rux 内核** 运行在 S-mode
- **用户程序** 运行在 U-mode ✅

```
┌─────────────────────────────────────┐
│  OpenSBI (M-mode)                   │
│  0x80000000 - 0x801fffff            │
├─────────────────────────────────────┤
│  Rux Kernel (S-mode)                │
│  0x80200000+                        │
├─────────────────────────────────────┤
│  User Applications (U-mode)         │
│  Shell, Desktop, Toybox, etc.       │
└─────────────────────────────────────┘
```

### QEMU virt 平台

**硬件配置**：
- CPU: RV64GC (RV64I M A F D C) - 4核
- 内存: 2GB (0x80000000 - 0x88000000)
- UART: ns16550a @ 0x10000000
- CLINT: @ 0x02000000 ✅
- PLIC: @ 0x0c000000 ✅

---

## 内存布局

### 物理内存映射

```
地址范围              大小     用途
─────────────────────────────────────────
0x8000_0000 -       128KB    OpenSBI firmware
0x801f_ffff
0x8020_0000 -       ~2MB     Rux 内核代码
0x8040_0000
0x8040_0000 -       16MB     内核堆 (Buddy/Slab)
0x8140_0000
0x8140_0000 -       64MB     用户物理页池
0x8540_0000
```

### 虚拟内存布局 (Sv39)

```
虚拟地址范围           用途
─────────────────────────────────────────
0x0000_0000_0000 -   用户空间 (低 256GB)
0x0000_003f_ffff

0xffff_ffc0_0000 -   内核空间 (高 256GB)
0xffff_ffff_ffff
    ├── 0xffff_ffc0_8000_0000  内核代码映射
    ├── 0xffff_ffc0_8140_0000  用户物理页映射
    └── 0xffff_ffc8_0000_0000  MMIO 映射
```

### 链接器脚本

**文件**：`kernel/src/arch/riscv64/linker.ld`

```ld
MEMORY {
    /* 避开 OpenSBI 固件区域 */
    RAM : ORIGIN = 0x80200000, LENGTH = 126M
}

SECTIONS {
    .text : {
        *(.init.entry)
        *(.init)
        . = ALIGN(4);
        *(.tramp)       /* 异常向量表 */
        *(.text.*)
        *(.rodata .rodata.*)
    } > RAM

    .data : {
        *(.data .data.*)
    } > RAM

    .bss : {
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        __bss_end = .;
    } > RAM

    /* 栈空间 */
    .stack : {
        . = ALIGN(16);
        _stack_bottom = .;
        . += 16384; /* 16KB 栈 */
        _stack_top = .;
    } > RAM
}
```

---

## 启动流程

### 启动序列

**文件**：`kernel/src/arch/riscv64/boot.S`

```asm
.section .init.entry
.global _start

_start:
    # 1. 关闭中断
    csrw sie, zero

    # 2. 设置栈指针
    la sp, _stack_top

    # 3. 清零 BSS 段
    la t0, __bss_start
    la t1, __bss_end
1:
    sd zero, 0(t0)
    addi t0, t0, 8
    bne t0, t1, 1b

    # 4. 保存 DTB 指针 (通过 s0 callee-saved)
    mv s0, a1

    # 5. 跳转到 Rust 入口
    call rust_main

    # 6. 不应该返回
2:  wfi
    j 2b
```

### OpenSBI 集成

**OpenSBI 功能**：
- 初始化硬件（UART、CLINT、PLIC）
- 提供SBI调用接口
- 跳转到 S-mode 内核

**启动流程**：
```
1. QEMU 启动 → M-mode
2. OpenSBI 加载 (0x80000000)
3. OpenSBI 初始化硬件
4. OpenSBI 跳转到内核 (0x80200000)
5. 内核进入 S-mode (_start)
6. 内核初始化各子系统
7. 启动 init 进程 (PID 1)
```

**检查点输出**：
```
OpenSBI v0.9
...
Domain0 Next Address: 0x0000000080202b1c  ← 内核入口点
Domain0 Next Mode: S-mode                 ← 进入 S-mode

██████  ██    ██ ██   ██
██   ██ ██    ██  ██ ██
██████  ██    ██   ███
██   ██ ██    ██  ██ ██
██   ██  ██████  ██   ██

  [ RISC-V 64-bit | POSIX Compatible | v0.1.0 ]

Kernel starting...
```

---

## 异常处理

### CSR 寄存器

**S-mode 关键 CSR**：

| CSR | 名称 | 用途 |
|-----|------|------|
| `stvec` | Trap Vector | 异常向量表地址 |
| `sstatus` | Supervisor Status | 中断使能、状态标志 |
| `scause` | Supervisor Cause | 异常原因 |
| `sepc` | Supervisor Exception PC | 异常返回地址 |
| `stval` | Supervisor Trap Value | 异常相关信息 |
| `sie` | Supervisor Interrupt Enable | 中断使能 |
| `sip` | Supervisor Interrupt Pending | 中断挂起 |
| `sscratch` | Scratch Register | 用户/内核态检测 |

### sscratch 检测机制

**Linux 风格的 trap 来源检测**：

```asm
# 用户态运行时: sscratch = current_task, tp = user TLS
# 内核态运行时: sscratch = 0, tp = current_task

trap_entry:
    csrrw tp, sscratch, tp    # 原子交换 tp 和 sscratch
    bnez tp, .Lfrom_user      # tp != 0 表示来自用户态
    j .Lfrom_kernel           # tp == 0 表示来自内核态
```

### Trap 处理框架

**核心文件**：
- `kernel/src/arch/riscv64/trap.S` - Trap 入口/出口汇编代码
- `kernel/src/arch/riscv64/trap.rs` - Trap 处理 Rust 代码

**Trap 处理流程**：

```assembly
trap_entry:
    csrrw tp, sscratch, tp     # 检测来源并保存 tp

    # 来自用户态
.Lfrom_user:
    ld sp, TASK_TI_KERNEL_SP(tp)  # 加载进程内核栈
    addi sp, sp, -272             # 分配 TrapFrame

    # 保存通用寄存器
    sd x1, 8(sp)      # ra
    sd x5, 16(sp)     # t0
    # ... 其他寄存器 ...

    # 保存 CSR
    csrr t0, sstatus
    csrr t1, sepc
    csrr t2, scause
    csrr t3, stval
    sd t0, 216(sp)    # sstatus
    sd t1, 224(sp)    # sepc
    sd t2, 232(sp)    # scause
    sd t3, 240(sp)    # stval

    # 调用 Rust 处理函数
    mv a0, sp
    call trap_handler

    # 恢复并返回
    # ...

    sret
```

### 异常类型

**常见异常**：
- `0x2`: 非法指令
- `0x5`: 读取访问故障
- `0x7`: 写入访问故障
- `0x8`: 用户模式 ecall
- `0xd`: 页面故障 (Store/AMO)

---

## 系统调用

### 系统调用接口

**寄存器约定**（遵循 RISC-V Linux ABI）：
- `a7`: 系统调用号
- `a0-a5`: 参数
- `a0`: 返回值

### 已实现的系统调用 (80+)

**文件操作**：
| 系统调用号 | 名称 | 说明 |
|-----------|------|------|
| 56 | sys_openat | 打开文件 |
| 57 | sys_close | 关闭文件 |
| 63 | sys_read | 读文件 |
| 64 | sys_write | 写文件 |
| 62 | sys_lseek | 定位文件 |
| 80 | sys_fstat | 获取文件状态 |
| 35 | sys_unlinkat | 删除文件 |
| 34 | sys_mkdirat | 创建目录 |

**进程操作**：
| 系统调用号 | 名称 | 说明 |
|-----------|------|------|
| 93 | sys_exit | 退出进程 |
| 172 | sys_getpid | 获取进程 ID |
| 110 | sys_getppid | 获取父进程 ID |
| 220 | sys_clone | 创建进程/线程 |
| 221 | sys_execve | 执行程序 |
| 260 | sys_wait4 | 等待子进程 |

**内存操作**：
| 系统调用号 | 名称 | 说明 |
|-----------|------|------|
| 214 | sys_brk | 调整堆 |
| 222 | sys_mmap | 内存映射 |
| 215 | sys_munmap | 取消映射 |
| 226 | sys_mprotect | 修改保护 |

**网络操作**：
| 系统调用号 | 名称 | 说明 |
|-----------|------|------|
| 198 | sys_socket | 创建套接字 |
| 200 | sys_bind | 绑定地址 |
| 201 | sys_listen | 监听连接 |
| 202 | sys_accept | 接受连接 |
| 203 | sys_connect | 发起连接 |
| 206 | sys_sendto | 发送数据 |
| 207 | sys_recvfrom | 接收数据 |

**信号操作**：
| 系统调用号 | 名称 | 说明 |
|-----------|------|------|
| 129 | sys_kill | 发送信号 |
| 134 | sys_rt_sigaction | 设置信号处理 |
| 135 | sys_rt_sigprocmask | 信号掩码 |

### 系统调用分发

**文件**：`kernel/src/syscall/dispatch.rs`

```rust
pub fn dispatch_syscall(syscall_no: u64, args: &[u64; 6]) -> i64 {
    match syscall_no as usize {
        63 => sys_read(args[0], args[1] as *mut u8, args[2]),
        64 => sys_write(args[0], args[1] as *const u8, args[2]),
        93 => sys_exit(args[0] as i32),
        172 => sys_getpid(),
        220 => sys_clone(args),
        221 => sys_execve(args),
        // ... 80+ 系统调用
        _ => -ENOSYS,
    }
}
```

---

## CPU 操作

### 中断控制

**文件**：`kernel/src/arch/riscv64/mod.rs`

```rust
/// 使能中断
pub fn enable_irq() {
    unsafe {
        asm!("csrsi sstatus, 2"); // 设置 SIE 位
    }
}

/// 禁用中断
pub fn disable_irq() {
    unsafe {
        asm!("csrci sstatus, 2"); // 清除 SIE 位
    }
}
```

### CPU ID 读取

```rust
pub fn cpu_id() -> usize {
    // 从 tp 寄存器读取当前 task，然后获取 ti_cpu
    current_task().ti_cpu as usize
}

pub fn hart_id() -> usize {
    // 从 SBI 获取硬件线程 ID
    sbi_call(SBI_GET_HART_ID, 0, 0, 0).value as usize
}
```

### 计数器读取

```rust
pub fn read_counter() -> u64 {
    let time: u64;
    unsafe {
        asm!("csrr {}, time", out(reg) time);
    }
    time
}

pub fn get_counter_freq() -> u64 {
    // 通过 SBI 查询
    sbi_call(SBI_GET_TIME, 0, 0, 0).value
}
```

---

## 设备驱动

### UART 驱动

**文件**：`kernel/src/console.rs`

**硬件配置**：
```rust
const UART0_BASE: usize = 0x1000_0000;  // ns16550a
```

### VirtIO 驱动

**文件**：`kernel/src/drivers/virtio/`

**支持的设备**：
- ✅ **virtio-blk** - 块设备驱动 (ext4 文件系统)
- ✅ **virtio-net** - 网络设备驱动
- ✅ **virtio-gpu** - GPU 驱动 (帧缓冲)
- ✅ **virtio-input** - 输入设备驱动 (键盘/鼠标)

### 中断控制器

**PLIC (Platform-Level Interrupt Controller)**

**文件**：`kernel/src/drivers/intc/plic.rs`

```rust
/// PLIC 初始化
pub fn init() {
    // 设置优先级阈值
    write_priority_threshold(0);

    // 为每个 hart 启用所有中断
    for hart in 0..4 {
        enable_all_interrupts(hart);
    }
}

/// 外部中断处理
pub fn handle_external_irq() {
    let claim = claim_interrupt();
    // 处理中断...
    complete_interrupt(claim);
}
```

---

## 多核支持

### SMP 初始化

**文件**：`kernel/src/arch/riscv64/smp.rs`

```rust
/// 启动次核
pub fn start_secondary_harts() {
    for hart_id in 1..4 {
        // 通过 SBI HSM 启动次核
        sbi_hsm_hart_start(hart_id, SECONDARY_ENTRY, 0);
    }
}

/// 次核入口点
#[no_mangle]
pub extern "C" fn secondary_start(hart_id: usize) -> ! {
    // 初始化本地数据
    // 进入调度循环
    scheduler_main();
}
```

### IPI (核间中断)

**文件**：`kernel/src/arch/riscv64/ipi.rs`

```rust
/// 发送 IPI
pub fn send_ipi(target_hart: usize, msg: IpiMessage) {
    IPI_QUEUE[target_hart].push(msg);
    sbi_send_ipi(1 << target_hart);
}

/// 处理 IPI
pub fn handle_ipi() {
    while let Some(msg) = IPI_QUEUE[cpu_id()].pop() {
        match msg {
            IpiMessage::Reschedule => set_need_resched(),
            IpiMessage::Shutdown => halt(),
        }
    }
}
```

### Per-CPU 数据

```rust
pub struct PerCpu {
    pub run_queue: CfsRunQueue,
    pub current_task: Option<Arc<Task>>,
    pub idle_task: Arc<Task>,
}

static PER_CPU: [SpinLock<PerCpu>; 4] = [...];
```

---

## CFS 调度器

**文件**：`kernel/src/sched/cfs.rs`

```rust
/// CFS 运行队列
pub struct CfsRunQueue {
    tasks: BTreeMap<u64, Arc<Task>>,  // 按 vruntime 排序
    min_vruntime: u64,
    load_weight: u64,
}

/// 选择下一个任务
pub fn pick_next_task(&mut self) -> Option<Arc<Task>> {
    // 选择 vruntime 最小的任务
    self.tasks.first_key_value().map(|(_, task)| task.clone())
}

/// 更新 vruntime
pub fn update_vruntime(task: &mut Task, delta: u64) {
    let weight = task.load_weight;
    let vruntime_delta = (delta * NICE_0_LOAD) / weight;
    task.vruntime += vruntime_delta;
}
```

---

## COW (Copy-on-Write)

**文件**：`kernel/src/arch/riscv64/mm/base.rs`

```rust
/// COW 页表复制
pub fn copy_page_table(src_root: PhysAddr, dst_root: PhysAddr) -> Result<(), i32> {
    for vpn in 0..512 {
        let pte = read_pte(src_root, vpn);
        if pte & PTE_V != 0 && pte & PTE_W != 0 {
            // 标记为 COW：清除写权限，设置 COW 标志
            let cow_pte = (pte & !PTE_W) | PTE_COW;
            write_pte(src_root, vpn, cow_pte);
            write_pte(dst_root, vpn, cow_pte);

            // 增加引用计数
            inc_page_ref_count(pte_to_phys(pte));
        }
    }
    sfence_vma();
    Ok(())
}

/// COW 页面故障处理
pub fn handle_cow_fault(vaddr: VirtAddr) -> Result<PhysAddr, i32> {
    // 分配新页面并复制数据
    // 更新 PTE
    // 刷新 TLB
}
```

---

## 参考资料

### 官方规范
- [RISC-V 特权架构规范](https://riscv.org/technical/specifications/)
- [RISC-V 指令集手册](https://riscv.org/technical/specifications/)
- [RISC-V Linux ABI](https://github.com/riscv-non-isa/riscv-elf-psabi-doc)

### 开源项目
- [OpenSBI](https://github.com/riscv/opensbi)
- [Linux RISC-V 移植](https://kernel.org/doc/html/latest/riscv/index.html)

### QEMU 文档
- [QEMU RISC-V virt 平台](https://www.qemu.org/docs/master/system/riscv/virt.html)

---

**文档版本**：v2.0.0
**最后更新**：2026-03-04
**维护者**：Rux 开发团队
