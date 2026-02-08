# 用户程序执行调试记录

**创建时间**：2025-02-07
**状态**：阻塞，待调试
**Phase**：Phase 11 - 用户程序执行

---

## 问题背景

### 目标
实现用户程序在 RISC-V Sv39 MMU 环境下的执行，包括：
1. 用户地址空间创建（独立页表）
2. ELF 程序加载
3. 用户栈分配
4. 内核模式到用户模式的切换（sret）
5. 用户模式异常处理（trap handler）

### 当前状态
- ✅ 用户程序执行框架已实现
- ⚠️ **测试时遇到页错误，暂时禁用**
- ⏳ 待调试：用户模式 trap 处理

---

## 实现细节

### 1. 用户程序构建系统

**位置**：`userspace/` 目录

**构建脚本**：`userspace/build.sh`
```bash
#!/bin/bash
cd "$(dirname "$0")"
cargo build --release --target riscv64gc-unknown-none-elf
```

**用户程序**：
- `hello_world/` - 简单的 Hello World 程序
- `shell/` - 简单的 shell（尝试执行 /hello_world）

**编译配置**：
```toml
[profile.release]
panic = "abort"
opt-level = "z"  # 优化代码大小
lto = true
```

**嵌入机制**：`kernel/src/embedded_user_programs.rs`
```rust
#[cfg(feature = "riscv64")]
pub static SHELL_ELF: &[u8] = include_bytes!("../../userspace/target/riscv64gc-unknown-none-elf/release/shell");
```

### 2. 用户地址空间创建

**函数**：`mm::create_user_address_space()`

**实现**：
```rust
pub fn create_user_address_space() -> Option<u64> {
    unsafe {
        // 1. 分配根页表（一页）
        let root_page = USER_PHYS_ALLOCATOR.alloc_page()?;

        // 2. 初始化页表（清零）
        let root_table = (root_page as *mut PageTable);
        (*root_table).zero();

        // 3. 复制内核映射到用户页表
        let kernel_ppn = (&raw mut ROOT_PAGE_TABLE as *mut PageTable as u64) / PAGE_SIZE;
        let root_ppn = root_page / PAGE_SIZE;
        copy_kernel_mappings(root_ppn, kernel_ppn);

        Some(root_ppn)
    }
}
```

**关键点**：
- 使用 `USER_PHYS_ALLOCATOR` 分配物理页（从高地址 0x88000000 向下分配）
- 调用 `copy_kernel_mappings()` 复制内核映射

### 3. 复制内核映射到用户页表

**函数**：`mm::copy_kernel_mappings(user_root_ppn, kernel_root_ppn)`

**实现逻辑**：
```rust
unsafe fn copy_kernel_mappings(user_root_ppn: u64, kernel_root_ppn: u64) {
    // 步骤 1：复制除 VPN2[0] 和 VPN2[2] 外的所有内核映射
    for i in 0..512 {
        let pte = (*kernel_table).get(i);
        if pte.is_valid() {
            // 跳过 VPN2[0]（用户代码和栈）
            if i == 0 { continue; }
            // 跳过 VPN2[2]（稍后单独处理）
            if i == 2 { continue; }
            (*user_table).set(i, pte);
        }
    }

    // 步骤 2：映射整个内核代码/数据区域（VPN2=2）
    // 0x80200000 - 0x80a00000 (8MB)
    // 权限：U=1, R=1, W=1, X=1
    let kernel_region_flags = PageTableEntry::V | PageTableEntry::U |
                              PageTableEntry::R | PageTableEntry::W | PageTableEntry::X |
                              PageTableEntry::A | PageTableEntry::D;
    map_region(user_root_ppn, 0x80200000, 0x800000, kernel_region_flags);

    // 步骤 3：映射用户物理内存区域
    // 0x84000000 - 0x88000000 (64MB)
    // 包含页表分配器分配的页表
    let user_phys_flags = PageTableEntry::V | PageTableEntry::U |
                          PageTableEntry::R | PageTableEntry::W |
                          PageTableEntry::A | PageTableEntry::D;
    map_region(user_root_ppn, 0x84000000, 0x4000000, user_phys_flags);
}
```

**映射总结**：
| 区域 | 虚拟地址 | 物理地址 | 大小 | 权限 | 用途 |
|-----|---------|---------|-----|------|-----|
| 用户空间 | 0x0 - 0x3FFFFFFF | 动态分配 | 1GB | U+R+W+X | 用户代码/栈/数据 |
| 内核代码 | 0x80200000+ | 恒等映射 | 8MB | U+R+W+X | 内核代码访问 |
| 用户物理页 | 0x84000000+ | 恒等映射 | 64MB | U+R+W | 页表访问 |

### 4. ELF 程序加载

**函数**：`test_shell_execution()` (在 `main.rs` 中)

**流程**：
```rust
// 1. 获取 shell ELF 数据
let shell_data = crate::embedded_user_programs::SHELL_ELF;

// 2. 验证 ELF 格式
ElfLoader::validate(shell_data)?;

// 3. 创建用户地址空间
let user_root_ppn = mm::create_user_address_space()?;

// 4. 解析 ELF 入口点和程序头
let entry = ElfLoader::get_entry(shell_data)?;
let phdr_count = ElfLoader::get_program_headers(shell_data)?;

// 5. 第一遍：计算虚拟地址范围
for i in 0..phdr_count {
    let phdr = ehdr.get_program_header(shell_data, i)?;
    if phdr.is_load() {
        // 更新 min_vaddr 和 max_vaddr
    }
}

// 6. 页对齐
let virt_start = min_vaddr & !(PAGE_SIZE - 1);
let virt_end = (max_vaddr + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
let total_size = virt_end - virt_start;

// 7. 一次性分配并映射整个用户内存范围
let flags = PageTableEntry::V | PageTableEntry::U |
           PageTableEntry::R | PageTableEntry::W |
           PageTableEntry::X | PageTableEntry::A |
           PageTableEntry::D;
let phys_base = mm::alloc_and_map_user_memory(
    user_root_ppn, virt_start, total_size, flags
)?;

// 8. 第二遍：加载每个 PT_LOAD 段
for i in 0..phdr_count {
    let phdr = ehdr.get_program_header(shell_data, i)?;
    if phdr.is_load() {
        // 复制 ELF 数据到物理内存
        // 清零 BSS
    }
}

// 9. 分配用户栈 (64KB)
const USER_STACK_TOP: u64 = 0x000000003FFF8000;
const USER_STACK_SIZE: u64 = 0x10000;
let user_stack_phys = mm::alloc_and_map_user_memory(
    user_root_ppn,
    USER_STACK_TOP - USER_STACK_SIZE,
    USER_STACK_SIZE,
    stack_flags,
)?;

// 10. 切换到用户模式执行
mm::switch_to_user(user_root_ppn, entry, USER_STACK_TOP);
```

**测试输出**（单核模式）：
```
test: Starting shell user program execution...
ElfLoader::validate: OK
mm: copy_kernel_mappings: kernel_ppn=0x80300, user_ppn=0x87fff
mm:   skipping VPN2[0] (user space)
mm:   skipping VPN2[2] (will handle separately)
mm: Mapping kernel region (0x80200000 - 0x80a00000) to user page table
mm: Mapping user physical memory region (0x84000000 - 0x88000000)
mm: copy_kernel_mappings: copied 2 mappings from 0x80300 to 0x87fff
test: User address space created, root PPN = 0x87fff
test: Virtual range: 0x10000 - 0x15000 (20480 bytes)
mm: alloc_and_map_user_memory: virt=0x10000, size=20480, pages=5
mm:   allocated phys=0x87ffa000
mm: map_user_region: user_root_ppn=0x87fff, virt=0x10000- 0x15000, size=0x5000
mm:   iteration 0: virt=0x10000
mm:     offset=0x0, phys=0x87ffa000
mm:   iteration 1: virt=0x11000
mm:     offset=0x1000, phys=0x87ffb000
mm:   iteration 2: virt=0x12000
mm:     offset=0x2000, phys=0x87ffc000
mm:   iteration 3: virt=0x13000
mm:     offset=0x3000, phys=0x87ffd000
mm:   iteration 4: virt=0x14000
mm:     offset=0x4000, phys=0x87ffe000
mm:   mapping complete
test: User memory allocated at phys=0x87ffa000
test: Loaded 2 segments
mm: alloc_and_map_user_memory: virt=0x3fff8000-0x40008000, size=0x10000, pages=16
mm:   allocated phys=0x87ff4000
mm: map_user_region: user_root_ppn=0x87fff, virt=0x3fff8000-0x40008000, size=0x10000
mm:   mapping complete
test: User stack ready, entry=0x10000
```

**地址布局**：
- **用户代码段**：虚拟地址 0x10000 - 0x15000，物理地址 0x87ffa000 - 0x87ffe000
- **用户栈**：虚拟地址 0x3FFF8000 - 0x40008000，物理地址 0x87ff4000 - 0x88004000
- **入口点**：0x10000

### 5. 切换到用户模式

**函数**：`mm::switch_to_user(user_root_ppn, entry, user_stack)`

**实现**：
```rust
pub unsafe fn switch_to_user(user_root_ppn: u64, entry: u64, user_stack: u64) -> ! {
    // 创建 satp 值（Sv39 模式）
    let satp = Satp::sv39(user_root_ppn, 0);

    // 获取 trap 栈（用于处理来自用户模式的异常）
    let trap_stack = get_trap_stack();

    core::arch::asm!(
        // 设置用户程序入口点
        "csrw sepc, {entry}",

        // 设置 sstatus (SPP=0 for user mode, SPIE=1)
        "li t0, 0x10",
        "csrw sstatus, t0",

        // 设置 sscratch 为内核 trap 栈
        // 当从用户模式进入 trap 时，trap_entry 会交换 sp 和 sscratch
        "csrw sscratch, {trap_stack}",

        // 刷新指令缓存
        "fence.i",

        // 设置 satp (使能 MMU)
        "csrw satp, {satp}",

        // 刷新 TLB
        "sfence.vma",

        // 设置用户栈指针
        "mv sp, {stack}",

        // 跳转到用户模式
        "sret",

        entry = in(reg) entry,
        satp = in(reg) satp.bits(),
        stack = in(reg) user_stack,
        trap_stack = in(reg) trap_stack,
        options(nostack, noreturn, nomem)
    );
}
```

**寄存器设置**：
- **sepc** = 0x10000（用户入口点）
- **sstatus** = 0x10（SPP=0 用户模式, SPIE=1 中断使能）
- **sscratch** = trap_stack（内核栈地址）
- **satp** = 0x8000000000087fff（MODE=8 Sv39, PPN=0x87fff）
- **sp** = 0x3FFF8000（用户栈顶）

### 6. Trap 处理机制

**文件**：`kernel/src/arch/riscv64/trap.S`

**Trampoline 页**：
- 位置：`.section .text.trampoline`
- 对齐：4KB (`.align 12`)
- 符号：`trampoline_start` - `trampoline_end`

**Trap 入口**：
```assembly
trap_entry:
    // 交换 sp 和 sscratch
    csrrw sp, sscratch, sp

    // 检查 sscratch 是否为 0（判断来自内核还是用户）
    csrr t0, sscratch
    bnez t0, from_user

from_kernel:
    // 来自内核模式，sp 不变
    j save_regs

from_user:
    // 来自用户模式，sp 现在指向内核栈
    // 多预留 8 字节用于保存用户栈指针
    addi sp, sp, -280

save_regs:
    // 保存调用者寄存器 (24 * 8 = 192 字节)
    sd x1, 0(sp)
    sd x5, 8(sp)
    // ... 其他寄存器 ...

    // 保存 sstatus, sepc, stval (24 字节)
    csrr t0, sstatus
    csrr t1, sepc
    csrr t2, stval
    sd t0, 208(sp)
    sd t1, 216(sp)
    sd t2, 224(sp)

    // 调用 Rust trap 处理函数
    mv a0, sp
    call trap_handler

    // 恢复寄存器...
    // 检查是否需要恢复用户栈
    csrr t0, sscratch
    bnez t0, restore_user_sp

    // 来自内核模式
    addi sp, sp, 272
    j do_sret

restore_user_sp:
    // 来自用户模式
    addi sp, sp, 280
    csrrw sp, sscratch, sp

do_sret:
    sret
```

**TrapFrame 结构**：
```rust
#[repr(C)]
pub struct TrapFrame {
    // 通用寄存器 (24 个)
    x1: u64,   // ra
    x5: u64,   // t0
    x6: u64,   // t1
    x7: u64,   // t2
    x10: u64,  // a0
    x11: u64,  // a1
    x12: u64,  // a2
    x13: u64,  // a3
    x14: u64,  // a4
    x15: u64,  // a5
    x16: u64,  // a6
    x17: u64,  // a7
    x18: u64,  // s2
    x19: u64,  // s3
    x20: u64,  // s4
    x21: u64,  // s5
    x22: u64,  // s6
    x23: u64,  // s7
    x24: u64,  // s8
    x25: u64,  // s9
    x26: u64,  // s10
    x27: u64,  // s11
    x28: u64,  // t3
    x29: u64,  // t4
    x30: u64,  // t5
    x31: u64,  // t6
    // CSR 寄存器 (3 个)
    sstatus: u64,
    sepc: u64,
    stval: u64,
}
// 总共 27 * 8 = 216 字节
// 来自用户模式时额外 + 8 字节保存用户栈指针 = 224 字节
```

---

## 问题分析

### 预期行为
1. `switch_to_user()` 设置好所有寄存器
2. 执行 `sret` 跳转到用户模式
3. 用户程序从 0x10000 开始执行
4. 用户程序调用 `ecall` 进行系统调用
5. `trap_handler` 处理系统调用
6. `sret` 返回用户模式

### 实际问题
**测试时遇到页错误，内核挂起。**

根据之前的调试日志，可能的问题：

1. **Trampoline 页未映射到用户页表**
   - `trap_entry` 地址在内核代码段 (0x80200000+)
   - 用户页表需要能访问 trap_entry
   - 当前 `copy_kernel_mappings()` 映射了 0x80200000 - 0x80a00000
   - **需要确认**：trap_entry 是否在这个范围内？

2. **内核栈地址未映射到用户页表**
   - `TRAP_STACKS` 数组位置未知
   - 用户模式访问内核栈会导致页错误
   - **需要确认**：TRAP_STACKS 的物理地址

3. **UART 设备未映射到用户页表**
   - UART 地址 0x10000000
   - `println!` 宏调用 UART 输出
   - 用户模式访问 UART 会导致页错误
   - **需要确认**：是否需要在 trap_handler 中访问 UART？

4. **用户代码本身有问题**
   - 入口点 0x10000 可能不是正确的代码地址
   - 用户程序可能执行了非法指令
   - **需要确认**：shell ELF 的实际入口点

### 调试方向

#### 方向 1：确认 Trampoline 映射

**检查**：trap_entry 的实际地址
```bash
# 查看内核符号表
riscv64-linux-gnu-objdump -d target/riscv64gc-unknown-none-elf/debug/rux | grep trap_entry
```

**验证**：trap_entry 是否在 0x80200000 - 0x80a00000 范围内

**如果不在**：需要调整 `copy_kernel_mappings()` 的映射范围

#### 方向 2：确认 TRAP_STACKS 位置

**检查**：TRAP_STACKS 的链接地址
```bash
# 查看内核段布局
riscv64-linux-gnu-objdump -h target/riscv64gc-unknown-none-elf/debug/rux
```

**验证**：TRAP_STACKS 在哪个段？

**如果在 BSS 段**：需要在用户页表中映射 BSS 段

**如果不在用户页表映射范围**：需要调整映射范围

#### 方向 3：禁用 trap_handler 中的 UART 输出

**当前状态**：trap_handler 中的调试输出已被注释

**验证**：确认没有其他 UART 访问

**如果还有**：需要完全禁用 trap_handler 中的所有 println! 和 putchar

#### 方向 4：检查用户程序入口点

**检查**：shell ELF 的 e_entry
```bash
# 读取 ELF header
readelf -h userspace/target/riscv64gc-unknown-none-elf/release/shell
```

**验证**：e_entry 是否为 0x10000

**如果不匹配**：可能是链接器脚本配置问题

#### 方向 5：简化测试用例

**创建最简单的用户程序**：
```assembly
# 用户程序：死循环
user_loop:
    wfi
    j user_loop
```

**目标**：验证用户模式切换本身是否工作

**如果失败**：问题在切换机制
**如果成功**：问题在复杂的用户程序

---

## 当前阻塞点

### 主要问题
**用户模式执行后触发页错误，内核挂起。**

### 已知信息
1. ✅ 用户地址空间创建成功（root PPN = 0x87fff）
2. ✅ ELF 加载成功（2 segments）
3. ✅ 用户栈分配成功
4. ✅ `switch_to_user()` 执行到 `sret`
5. ❌ **`sret` 之后的状态未知**
6. ❌ **是否进入用户代码未知**
7. ❌ **触发页错误的具体位置未知**

### 调试限制
- 无法在用户模式下使用 `println!`（UART 未映射）
- 无法在 `trap_handler` 中使用 `println!`（可能导致递归页错误）
- 多核环境下输出混乱（控制台同步问题）

### 下一步调试计划

#### 优先级 1：确认 Trampoline 映射
```bash
# 检查 trap_entry 地址
riscv64-linux-gnu-nm target/riscv64gc-unknown-none-elf/debug/rux | grep trap_entry
```

**如果地址 < 0x80200000 或 > 0x80a00000**：
- 调整 `copy_kernel_mappings()` 的映射范围
- 确保整个 `.text.trampoline` 段被映射

#### 优先级 2：确认 TRAP_STACKS 映射
```bash
# 检查 TRAP_STACKS 地址
riscv64-linux-gnu-nm target/riscv64gc-unknown-none-elf/debug/rux | grep TRAP_STACKS
```

**如果不在用户页表映射范围**：
- 将 TRAP_STACKS 移到映射范围内
- 或在用户页表中额外映射 TRAP_STACKS

#### 优先级 3：使用 GDB 调试
```bash
# 启动 QEMU with GDB server
qemu-system-riscv64 -machine virt -cpu rv64 -smp 1 -m 2G -nographic \
    -bios /usr/share/qemu/opensbi-riscv64-generic-fw_dynamic.bin \
    -kernel target/riscv64gc-unknown-none-elf/debug/rux \
    -s -S

# 在另一个终端启动 GDB
riscv64-linux-gnu-gdb target/riscv64gc-unknown-none-elf/debug/rux
(gdb) target remote localhost:1234
(gdb) break *0x80204000  # switch_to_user
(gdb) continue
(gdb) stepi  # 单步执行 sret
(gdb) info registers  # 查看寄存器状态
```

**关键检查点**：
1. `sret` 执行后的 PC 值（应该是 0x10000）
2. `sret` 执行后的 satp 值（应该是 0x8000000000087fff）
3. 是否触发异常（查看 scause）

#### 优先级 4：简化测试用例
**创建最小用户程序**：
```rust
// userspace/minimal/src/main.rs
#![no_std]
#![no_main]

#[no_mangle]
pub extern "C" fn _start() -> ! {
    loop {
        unsafe { core::arch::asm!("wfi", options(nomem, nostack)) };
    }
}
```

**目标**：验证用户模式切换是否工作

---

## 附录：关键代码位置

### 用户程序执行相关文件
| 文件 | 描述 |
|-----|------|
| `kernel/src/main.rs:158-380` | `test_shell_execution()` 测试函数 |
| `kernel/src/arch/riscv64/mm.rs:770-796` | `create_user_address_space()` |
| `kernel/src/arch/riscv64/mm.rs:804-865` | `copy_kernel_mappings()` |
| `kernel/src/arch/riscv64/mm.rs:928-960` | `alloc_and_map_user_memory()` |
| `kernel/src/arch/riscv64/mm.rs:970-1010` | `switch_to_user()` |
| `kernel/src/arch/riscv64/trap.S:1-152` | Trap 入口和 Trampoline |
| `kernel/src/arch/riscv64/trap.rs:182-329` | `trap_handler()` |
| `kernel/src/embedded_user_programs.rs` | 用户程序嵌入 |
| `kernel/src/fs/elf.rs` | ELF 加载器 |
| `userspace/` | 用户程序源码 |

### 链接器脚本
| 文件 | 描述 |
|-----|------|
| `kernel/src/arch/riscv64/linker.ld` | 内核链接脚本 |
| `userspace/target/riscv64gc-unknown-none-elf/release/build.rs` | 用户程序链接脚本 |

### 测试脚本
| 脚本 | 描述 |
|-----|------|
| `test/run_riscv.sh` | RISC-V 运行脚本 |
| `test/debug_riscv.sh` | RISC-V GDB 调试脚本 |

---

## 更新日志

### 2025-02-07
- ✅ 用户程序执行框架实现完成
- ✅ 代码清理完成（删除 GDB 文件和调试输出）
- ✅ 多核启动测试成功
- ❌ 用户程序执行遇到页错误，暂时禁用
- 📝 创建调试文档

### 待完成
- ⏳ 调试用户模式 trap 处理
- ⏳ 解决页错误问题
- ⏳ 验证用户程序执行
- ⏳ 实现系统调用（从用户模式）
