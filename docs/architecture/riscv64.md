# RISC-V 64位架构实现文档

本文档详细记录 Rux 内核在 RISC-V 64位架构上的实现细节。

**最后更新**：2025-02-06
**状态**：✅ 完全实现并设为默认平台

---

## 目录

- [架构概述](#架构概述)
- [内存布局](#内存布局)
- [启动流程](#启动流程)
- [异常处理](#异常处理)
- [系统调用](#系统调用)
- [CPU 操作](#cpu-操作)
- [设备驱动](#设备驱动)
- [与 ARM 对比](#与-arm-对比)
- [已知限制](#已知限制)
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
- **用户程序** 运行在 U-mode（待实现）

```
┌─────────────────────────────────────┐
│  OpenSBI (M-mode)                   │
│  0x80000000 - 0x8001ffff           │
├─────────────────────────────────────┤
│  Rux Kernel (S-mode)                │
│  0x80200000+                        │
├─────────────────────────────────────┤
│  User Applications (U-mode)         │
│  (待实现)                            │
└─────────────────────────────────────┘
```

### QEMU virt 平台

**硬件配置**：
- CPU: RV64GC (RV64I M A F D C)
- 内存: 2GB (0x80000000 - 0x88000000)
- UART: ns16550a @ 0x10000000
- CLINT: @ 0x02000000 (待实现)
- PLIC: @ 0x0c000000 (待实现)

---

## 内存布局

### 物理内存映射

```
地址范围              大小     用途
─────────────────────────────────────────
0x8000_0000 -       128KB    OpenSBI firmware
0x8001_ffff
0x8020_0000 -       ~2MB     Rux 内核代码
0x8040_0000
0x801F_C000          16KB     内核栈（向下增长）
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

**文件**：`kernel/src/arch/riscv64/boot.rs`

```rust
#[no_mangle]
pub extern "C" fn _start() -> ! {
    // 1. 设置栈指针
    unsafe {
        core::arch::asm!(
            "li sp, {stack_base}",
            stack_base = const 0x801F_C000u64,
            options(nostack, nomem)
        );
    }

    // 2. 设置 trap 向量
    unsafe {
        let trap_addr = simple_trap_entry as *const () as u64;
        core::arch::asm!(
            "csrw stvec, {}",
            in(reg) trap_addr,
            options(nostack)
        );
    }

    // 3. 清零 BSS 段
    unsafe {
        let bss_start = &BSS_START as *const u64 as usize;
        let bss_end = &BSS_END as *const u64 as usize;
        let mut bss_ptr = bss_start as *mut u64;

        while bss_ptr < bss_end as *mut u64 {
            *bss_ptr = 0;
            bss_ptr = bss_ptr.offset(1);
        }
    }

    // 4. 调用内核主函数
    unsafe {
        main();
    }

    // 5. 主函数不应该返回
    loop {
        unsafe { core::arch::asm!("wfi", options(nomem, nostack)); }
    }
}
```

### OpenSBI 集成

**OpenSBI 功能**：
- 初始化硬件（UART、CLINT、PLIC）
- 提供SBI调用接口（可选使用）
- 跳转到 S-mode 内核

**启动流程**：
```
1. QEMU 启动 → M-mode
2. OpenSBI 加载 (0x80000000)
3. OpenSBI 初始化硬件
4. OpenSBI 跳转到内核 (0x80200000)
5. 内核进入 S-mode (_start)
```

**检查点输出**：
```
OpenSBI v0.9
...
Domain0 Next Address: 0x0000000080202b1c  ← 内核入口点
Domain0 Next Mode: S-mode                 ← 进入 S-mode
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

### Trap 入口

**文件**：`kernel/src/arch/riscv64/trap.rs`

**汇编入口**（global_asm）：
```asm
.text
.align 2
.global trap_entry

trap_entry:
    // 保存调用者寄存器
    addi sp, sp, -256

    sw x1, 0(sp)      // ra
    sw x5, 4(sp)      // t0
    sw x6, 8(sp)      // t1
    // ... 保存 x5-x31

    // 保存 S-mode CSR
    csrrs x5, sstatus, x5
    csrrs x6, sepc, x6
    csrrs x7, stval, x7
    sw x5, 104(sp)
    sw x6, 108(sp)
    sw x7, 112(sp)

    // 调用 Rust trap 处理函数
    addi x10, sp, 0
    tail trap_handler

    // 恢复寄存器
    lw x5, 104(sp)
    lw x6, 108(sp)
    lw x7, 112(sp)
    csrrw x5, sstatus, x5
    csrrw x6, sepc, x6
    csrrw x7, stval, x7

    // ... 恢复通用寄存器

    addi sp, sp, 256
    sret               // S-mode 返回
```

### 异常处理函数

**Rust 函数**：
```rust
#[no_mangle]
pub extern "C" fn trap_handler(frame: *mut TrapFrame) {
    unsafe {
        // 读取 scause
        let scause: u64;
        asm!("csrr {}, scause", out(reg) scause);

        // 读取 stval
        let stval: u64;
        asm!("csrr {}, stval", out(reg) stval);

        // 检查异常类型
        let exception_code = scause & 0xFF;
        let is_interrupt = (scause >> 63) != 0;

        if is_interrupt {
            // 中断处理
            handle_interrupt(exception_code);
        } else {
            // 异常处理
            handle_exception(exception_code, stval);
        }
    }
}
```

### 异常类型

**常见异常**：
- `0x2`: 非法指令
- `0x3`: 断点
- `0x5`: 读取访问故障
- `0x7`: 写入访问故障
- `0x8`: 用户模式 ecall
- `0x9`: 监管者模式 ecall

---

## 系统调用

### 系统调用接口

**寄存器约定**（遵循 RISC-V Linux ABI）：
- `a7`: 系统调用号
- `a0-a6`: 参数
- `a0`: 返回值

**系统调用示例**：
```rust
// 用户代码
let ret = syscall(SYS_write, fd, buf, count);

// 编译为 ecall 指令
li a7, SYS_write
mv a0, fd
mv a1, buf
mv a2, count
ecall
```

### Trap 处理框架

**状态**：✅ 完全实现（2025-02-09）

**核心文件**：
- `kernel/src/arch/riscv64/trap.S` - Trap 入口/出口汇编代码
- `kernel/src/arch/riscv64/trap.rs` - Trap 处理 Rust 代码
- `kernel/src/arch/riscv64/syscall.rs` - 系统调用分发和实现

**Trap 处理流程**：

```assembly
# Trap 入口 (kernel/src/arch/riscv64/trap.S)
trap_entry:
    mv t0, sp                      # 1. 保存当前 sp
    csrrw sp, sscratch, sp          # 2. 交换 sp 和 sscratch（切换到内核栈）
    addi sp, sp, -272              # 3. 分配 TrapFrame 空间
    sd t0, 0(sp)                   # 4. 保存原始 sp

    # 5. 保存调用者寄存器
    sd x1, 8(sp)    # ra
    sd x5, 16(sp)   # t0
    # ... 省略其他寄存器 ...
    sd x31, 208(sp) # t6

    # 6. 保存 CSR 寄存器
    csrr t0, sstatus
    csrr t1, sepc
    csrr t2, stval
    sd t0, 216(sp)  # sstatus
    sd t1, 224(sp)  # sepc
    sd t2, 232(sp)  # stval

    # 7. 调用 Rust trap 处理函数
    addi a0, sp, 8  # 跳过原始 sp，与 TrapFrame 对齐
    call trap_handler

    # 8. 恢复 CSR 寄存器
    ld t0, 216(sp)
    ld t1, 224(sp)
    ld t2, 232(sp)
    csrw sstatus, t0
    csrw sepc, t1
    csrw stval, t2

    # 9. 恢复调用者寄存器
    ld x1, 8(sp)
    # ... 省略其他寄存器 ...
    ld x31, 208(sp)

    # 10. 恢复原始 sp 并切换回
    ld t0, 0(sp)                   # Load original sp
    addi sp, sp, 272               # Deallocate trap frame
    csrr t1, sscratch              # Read kernel stack pointer
    mv sp, t0                      # Restore original sp
    csrw sscratch, t1              # Restore kernel stack pointer to sscratch

    sret                            # 返回异常处理
```

**TrapFrame 结构**（`kernel/src/arch/riscv64/trap.rs`）：

```rust
#[repr(C)]
pub struct TrapFrame {
    pub_x0: u64,      // +0:  原始 sp（保存在内核栈）
    pub_x1: u64,      // +8:  ra
    pub_x5: u64,      // +16: t0
    // ... 省略其他寄存器 ...
    pub_x31: u64,     // +208: t6
    pub_sstatus: u64, // +216: S-mode 状态寄存器
    pub_sepc: u64,    // +224: 异常程序计数器
    pub_stval: u64,   // +232: 异常相关信息
}
```

### 系统调用处理

**文件**：`kernel/src/arch/riscv64/syscall.rs`

**系统调用分发**：

```rust
#[no_mangle]
pub extern "C" fn syscall_handler(frame: &mut SyscallFrame) {
    let syscall_no = frame.a7;
    let args = [frame.a0, frame.a1, frame.a2, frame.a3, frame.a4, frame.a5];

    // 根据系统调用号分发
    frame.a0 = match syscall_no as u32 {
        63 => sys_read(args),
        64 => sys_write(args),
        56 => sys_openat(args),
        57 => sys_close(args),
        93 => sys_exit(args),      // ✅ 已实现
        172 => sys_getpid(args),   // ✅ 已实现
        110 => sys_getppid(args),  // ✅ 已实现
        // ... 其他系统调用
        _ => {
            // 未知系统调用
            frame.a0 = -38; // ENOSYS
        }
    };
}
```

**已实现的系统调用**：

| 系统调用号 | 名称 | 状态 | 说明 |
|-----------|------|------|------|
| 93 | sys_exit | ✅ | 退出当前进程 |
| 172 | sys_getpid | ✅ | 获取进程 ID |
| 110 | sys_getppid | ✅ | 获取父进程 ID |
| 63 | sys_read | ⏳ | 读文件（部分实现） |
| 64 | sys_write | ⏳ | 写文件（部分实现） |
| 56 | sys_openat | ⏳ | 打开文件（部分实现） |
| 57 | sys_close | ⏳ | 关闭文件（部分实现） |

### 用户模式支持

**状态**：✅ 完全实现（2025-02-09）

**用户模式切换**：`kernel/src/arch/riscv64/usermode_asm.S`

```assembly
# switch_to_user_linux_asm(entry, user_stack)
# Linux 风格的用户模式切换 - 单页表，不切换 satp

switch_to_user_linux_asm:
    mv t5, a0              # t5 = entry (用户程序入口点)
    mv t6, a1              # t6 = user_stack (用户栈指针)

    # 1. 设置 sstatus.SPP = 0 (从 U-mode 返回)
    #    sstatus.SPIE = 1 (在 U-mode 使能中断)
    #    sstatus.UXL = 2 (64-bit user mode)
    csrr t1, sstatus       # 读取当前 sstatus
    li t0, 0x100           # SPP 位掩码
    not t0, t0             # 取反
    and t1, t1, t0         # 清除 SPP 位
    li t0, 0x20            # SPIE 位掩码
    or t1, t1, t0          # 设置 SPIE
    li t0, 0x200000000     # UXL = 2
    or t1, t1, t0          # 设置 UXL
    csrw sstatus, t1       # 写入 sstatus

    # 2. 设置用户程序入口点
    csrw sepc, t5          # sepc = entry

    # 3. 刷新指令缓存和 TLB
    fence.i
    sfence.vma

    # 4. 确保 sscratch 指向内核栈
    csrr t1, sscratch      # 读取内核栈指针

    # 5. 设置用户栈指针
    mv sp, t6              # sp = user_stack

    # 6. 返回用户模式
    sret                    # 返回用户模式
```

**用户程序编译**：`userspace/hello_world/`

```rust
#![no_std]
#![no_main]

use core::arch::asm;

// 系统调用包装函数
pub unsafe fn syscall1(n: u64, a0: u64) -> u64 {
    let mut ret: u64;
    asm!(
        "ecall",
        inlateout("a7") n => _,
        inlateout("a0") a0 => ret,
        lateout("a1") _,
        // ... 其他 clobbered 寄存器 ...
        options(nostack, nomem)
    );
    ret
}

// 用户程序入口点
#[no_mangle]
pub extern "C" fn _start() -> ! {
    // 调用 sys_exit(0)
    unsafe { syscall1(93, 0) };

    // 如果 sys_exit 失败，进入死循环
    loop {
        unsafe { asm!("nop", options(nomem, nostack)) };
    }
}
```

**用户程序链接器脚本**：`userspace/user.ld`

```ld
OUTPUT_ARCH("riscv")
ENTRY(_start)

MEMORY {
    USER : ORIGIN = 0x10000, LENGTH = 64K
}

SECTIONS {
    .text : { *(.text .text.*) } > USER
    .rodata : { *(.rodata .rodata.*) } > USER
    .data : { *(.data .data.*) } > USER
    .bss : { *(.bss .bss.*) } > USER

    /* 用户栈在运行时分配 */
    .stack (NOLOAD) : {
        . = ALIGN(16);
        . = 0x10000; /* 栈大小 64KB */
    } > USER
}
```

### 系统调用测试

**测试命令**：
```bash
# 编译内核
cargo build --package rux --features riscv64

# 运行测试
qemu-system-riscv64 -M virt -cpu rv64 -m 2G -nographic \
    -serial mon:stdio -kernel target/riscv64gc-unknown-none-elf/debug/rux
```

**预期输出**：
```
test: USER PROGRAM STARTING
[TRAP:ECALL]           <- 陷阱处理入口
[ECALL:5D]             <- 系统调用 0x5D (93) = sys_exit
sys_exit: exiting with code 0  <- sys_exit 执行成功
]                      <- 汇编代码到达 sret
(然后进入 WFI 循环，因为没有创建 PCB)
```

### sscratch 寄存器管理

**问题**：在 trap 出口时，如果错误地将用户栈指针写入 `sscratch`，下一个系统调用将无法切换到内核栈。

**解决方案**：在 trap 出口时，确保 `sscratch` 始终包含内核栈指针。

```assembly
# 恢复原始 sp 并切换回
ld t0, 0(sp)           # Load original sp (user or kernel)
addi sp, sp, 272       # Deallocate trap frame
csrr t1, sscratch      # Read kernel stack pointer from sscratch
mv sp, t0              # Restore original sp (user or kernel)
csrw sscratch, t1      # Restore kernel stack pointer to sscratch  # 关键！
```

**关键点**：
- `sscratch` 在 trap 初始化时设置为内核栈指针
- Trap 入口时使用 `csrrw sp, sscratch, sp` 交换 sp 和 sscratch
- Trap 出口时必须恢复 `sscratch` 为内核栈指针
- 这样连续的系统调用可以正常工作

---

## CPU 操作

### 中断控制

**文件**：`kernel/src/arch/riscv64/cpu.rs`

```rust
/// 使能中断
pub fn enable_irq() {
    unsafe {
        let mut sstatus: u64;
        asm!("csrrs {}, sstatus, zero", out(reg) sstatus);
        sstatus |= 1 << 1; // SIE bit
        asm!("csrw sstatus, {}", in(reg) sstatus);
    }
}

/// 禁用中断
pub fn disable_irq() {
    unsafe {
        let mut sstatus: u64;
        asm!("csrrs {}, sstatus, zero", out(reg) sstatus);
        sstatus &= !(1 << 1); // Clear SIE
        asm!("csrw sstatus, {}", in(reg) sstatus);
    }
}
```

### CPU ID 读取

```rust
pub fn get_core_id() -> u64 {
    unsafe {
        let hart_id: u64;
        asm!("csrrw {}, mhartid, zero", out(reg) hart_id);
        hart_id
    }
}
```

### 计数器读取

```rust
pub fn read_counter() -> u64 {
    unsafe {
        let time: u64;
        asm!("csrrw {}, time, zero", out(reg) time);
        time
    }
}

pub fn get_counter_freq() -> u64 {
    // QEMU virt 默认频率
    10_000_000  // 10 MHz
}
```

---

## 设备驱动

### UART 驱动

**文件**：`kernel/src/console.rs`

**硬件配置**：
```rust
#[cfg(feature = "riscv64")]
const UART0_BASE: usize = 0x1000_0000;  // ns16550a
```

**字符输出**：
```rust
pub fn putc(&self, c: u8) {
    unsafe {
        let addr = self.base;
        asm!(
            "sb {0}, 0({1})",    // store byte
            in(reg) c,
            in(reg) addr,
            options(nostack, nomem)
        );
    }
}
```

**字符输入**：
```rust
pub fn getc(&self) -> Option<u8> {
    unsafe {
        let addr = self.base;
        let data: u8;
        asm!(
            "lb {0}, 5({1})",    // load byte from LSR register
            out(reg) data,
            in(reg) addr,
            options(nostack, nomem)
        );

        if data & 0x01 != 0 {  // LSR_DATA_READY
            let c: u8;
            asm!(
                "lb {0}, 0({1})",  // RBR register
                out(reg) c,
                in(reg) addr,
                options(nostack, nomem)
            );
            Some(c)
        } else {
            None
        }
    }
}
```

---

## 与 ARM 对比

### 特权级对比

| ARM | RISC-V | 说明 |
|-----|--------|------|
| EL0 | U-mode | 用户模式 |
| EL1 | S-mode | 内核模式 |
| EL2 | ⚠️ 无 | RISC-V 无虚拟化扩展 |
| EL3 | M-mode | 机器模式（固件） |

### CSR 对比

| ARM | RISC-V | 说明 |
|-----|--------|------|
| `VBAR_EL1` | `stvec` | 异常向量表 |
| `ESR_EL1` | `scause` | 异常原因 |
| `ELR_EL1` | `sepc` | 异常返回地址 |
| `FAR_EL1` | `stval` | 故障地址 |
| `DAIF` | `sstatus` | 中断屏蔽 |
| `CNTVCT_EL0` | `time` | 计数器 |

### 异常返回对比

**ARM**:
```asm
eret       // Exception Return
```

**RISC-V**:
```asm
sret       // Supervisor Return
```

### 系统调用对比

**ARM**:
```asm
svc #0     // Supervisor Call
// x8 = 系统调用号
```

**RISC-V**:
```asm
ecall      // Environment Call
// a7 = 系统调用号
```

---

## 已知限制

### 待实现功能

1. **PLIC (Platform-Level Interrupt Controller)**
   - 外部中断处理
   - 优先级管理
   - 中断路由

2. **CLINT (Core-Local Interrupt Controller)**
   - 定时器中断
   - 软件中断（IPI）
   - 时钟管理

3. **SMP 多核支持**
   - 次核启动
   - Per-CPU 数据
   - IPI 机制

4. **MMU 使能**
   - 虚拟内存映射
   - 页表管理
   - 地址空间隔离

### 当前限制

- ⏳ 仅支持单核
- ⏳ 无硬件中断支持
- ⏳ 无定时器支持
- ⏳ MMU 未启用

---

## 参考资料

### 官方规范
- [RISC-V 特权架构规范](https://riscv.org/technical/specifications/)
- [RISC-V 指令集手册](https://riscv.org/technical/specifications/)
- [RISC-V Unprivileged ISA](https://riscv.org/specifications/)

### 开源项目
- [OpenSBI](https://github.com/riscv/opensbi)
- [Linux RISC-V 移植](https://kernel.org/doc/html/latest/riscv/index.html)
- [rCore OS (RISC-V)](https://github.com/rcore-os/rCore)

### QEMU 文档
- [QEMU RISC-V virt 平台](https://www.qemu.org/docs/master/system/riscv/virt.html)
- [QEMU RISC-V 文档](https://www.qemu.org/docs/master/system/target-riscv.html)

### 学习资源
- [riscv-rust-kernel](https://github.com/d0iasm/riscv-rust-kernel)
- [RISC-V OS 开发教程](https://osblog.stephenmarz.com/)
- [RISC-V Internals](https://riscv.org/internals/)

---

**文档版本**：v1.0.0
**最后更新**：2025-02-06
**维护者**：Claude Sonnet 4.5 (AI 辅助)
