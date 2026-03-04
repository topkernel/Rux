# Fork + Execve 调试报告

**日期**: 2026-03-01 ~ 2026-03-04
**调试者**: Fei Wang + Claude Code
**状态**: ✅ 已解决

---

## 1. 问题背景

在实现完整的 Unix 风格进程管理时，`fork()` 和 `execve()` 系统调用遇到了一系列复杂问题：

1. **fork 子进程无法正常返回用户空间**
2. **COW (Copy-on-Write) 页表处理错误**
3. **上下文切换导致寄存器状态丢失**
4. **trap 处理的 task_struct 偏移量错误**

---

## 2. 问题一：task_struct 偏移量错误

### 2.1 症状

- fork 子进程在处理 trap 时访问无效内存地址
- 系统崩溃或挂起

### 2.2 调试过程

通过分析 `trap.S` 汇编代码和 `Task` 结构体布局：

```asm
# trap.S 原代码
ld sp, TASK_TI_KERNEL_SP(tp)  # 加载内核栈指针
```

检查 Task 结构体：

```rust
// kernel/src/process/task.rs
pub struct Task {
    // thread_info 嵌入在开头
    pub ti_cpu: u32,           // offset 0x00
    pub ti_preempt_count: u32, // offset 0x04
    pub ti_kernel_sp: u64,     // offset 0x08 ← 实际偏移
    pub ti_user_sp: u64,       // offset 0x10
    // ...
}
```

### 2.3 根因

`TASK_TI_KERNEL_SP` 常量定义为 `0x10`，但实际上 `ti_kernel_sp` 在结构体中的偏移是 `0x08`。

`0x10` 是 `ti_user_sp` 的偏移，导致加载了错误的栈指针。

### 2.4 解决方案

**文件**: `kernel/src/arch/riscv64/trap.S`

```asm
# 修复前
.equ TASK_TI_KERNEL_SP, 0x10

# 修复后
.equ TASK_TI_KERNEL_SP, 0x08
```

**Commit**: `33415ca fix(arch): 修复 trap 处理的 task_struct 偏移量和 init 进程内核栈`

---

## 3. 问题二：sscratch 检测机制

### 3.1 症状

- 无法正确区分 trap 来自用户态还是内核态
- 用户态 trap 被误判为内核态 trap，导致栈指针错误

### 3.2 Linux 标准做法

Linux 使用 `sscratch` 寄存器实现高效的 trap 来源检测：

```asm
# Linux entry.S
handle_exception:
    csrrw tp, sscratch, tp   # 原子交换 tp 和 sscratch
    bnez tp, .Lsave_context  # tp != 0 表示来自用户态
                             # tp == 0 表示来自内核态
```

**原理**:
- 用户态运行时：`sscratch = current_task`，`tp = user TLS`
- 内核态运行时：`sscratch = 0`，`tp = current_task`
- 进入 trap 时交换，通过 tp 值判断来源

### 3.3 Rux 实现

**文件**: `kernel/src/arch/riscv64/trap.S`

```asm
trap_entry:
    csrrw tp, sscratch, tp    # 交换 tp 和 sscratch
    bnez tp, .Lfrom_user      # 非 0 = 用户态
    j .Lfrom_kernel           # 0 = 内核态
```

**文件**: `kernel/src/sched/sched.rs`

```rust
pub fn init() {
    // 初始化 sscratch = 0（表示内核态）
    unsafe {
        csrw_sscratch(0);
    }
    // tp 指向 idle task
    switch_to(&mut idle_task);
}
```

**Commit**: `d5c82c7 feat(arch): 实现 Linux 风格的 sscratch 检测机制`

---

## 4. 问题三：COW 页表复制错误

### 4.1 症状

- fork 后父子进程共享相同的物理页
- 写入时没有触发 page fault
- 或者 page fault 后无法正确处理

### 4.2 调试过程

分析 `copy_page_table` 函数：

```rust
// 原代码问题
let pfn = (pte >> 10) << 12;  // 错误：重复移位
```

### 4.3 根因

1. **PFN 计算错误**: PTE 中的 PPN (Physical Page Number) 已经是物理页号，不需要再次左移 12 位
2. **COW 标志未正确设置**: 需要同时修改父子进程的 PTE 为只读
3. **TLB 未刷新**: 修改页表后没有刷新 TLB

### 4.4 解决方案

**文件**: `kernel/src/arch/riscv64/mm/base.rs`

```rust
// 正确的 COW 实现
pub fn copy_page_table(src_root: PhysAddr, dst_root: PhysAddr) -> Result<(), i32> {
    for vpn in 0..512 {
        let src_pte = read_pte(src_root, vpn);

        if src_pte & PTE_V != 0 && src_pte & PTE_R != 0 {
            // 获取物理页号
            let ppn = (src_pte >> 10) & 0x3FFFFFFF;  // PPN[2:0]
            let phys_addr = ppn << 12;

            // 标记为 COW：清除写权限，设置 COW 标志
            let cow_pte = (src_pte & !PTE_W) | PTE_COW;

            // 同时更新父进程和子进程的 PTE
            write_pte(src_root, vpn, cow_pte);
            write_pte(dst_root, vpn, cow_pte);

            // 增加页引用计数
            inc_page_ref_count(phys_addr);
        }
    }

    // 刷新 TLB
    sfence_vma();
    Ok(())
}
```

**COW Page Fault 处理**:

```rust
pub fn handle_cow_fault(vaddr: VirtAddr) -> Result<PhysAddr, i32> {
    let pte = get_pte(vaddr)?;
    let old_phys = pte_to_phys(pte);

    // 分配新物理页
    let new_phys = alloc_user_phys_page()?;

    // 复制数据
    memcpy(new_phys, old_phys, PAGE_SIZE);

    // 减少旧页引用计数
    if dec_page_ref_count(old_phys) == 0 {
        free_user_phys_page(old_phys);
    }

    // 更新 PTE：可写，清除 COW 标志
    let new_pte = (pte & !PTE_COW) | PTE_W | phys_to_pte(new_phys);
    update_pte(vaddr, new_pte);

    // 刷新 TLB
    sfence_vma_addr(vaddr);

    Ok(new_phys)
}
```

**关键修复**:

1. **TLB 刷新顺序**: 先更新页表项，再刷新 TLB

```rust
// 错误顺序
sfence_vma();        // 先刷新 TLB
write_pte(...);      // 后更新页表

// 正确顺序
write_pte(...);      // 先更新页表
sfence_vma();        // 后刷新 TLB
```

2. **使用用户物理分配器**: fork 和 COW 应使用 `alloc_user_phys_page()` 而非内核分配器

**Commit**: `2839915 fix(fork): 修复 fork 子进程的 COW 实现和上下文切换`

---

## 5. 问题四：fork 子进程上下文切换

### 5.1 症状

- fork 子进程被调度后立即崩溃
- 或者子进程返回到错误的地址

### 5.2 调试过程

分析 `cpu_switch_to` 和 fork 子进程初始化：

```rust
// 原代码：设置 pc 寄存器
child_ctx.pc = ret_from_fork as u64;
```

但 `cpu_switch_to` 恢复的是 `ra` 寄存器，然后执行 `ret` 指令跳转到 `ra` 指向的地址。

### 5.3 根因

`cpu_switch_to` 使用 `ret` 指令返回，它跳转到 `ra` 寄存器存储的地址，而不是 `pc`。

因此应该设置 `ra` 而不是 `pc`。

### 5.4 解决方案

**文件**: `kernel/src/process/fork.rs`

```rust
// 修复前
child_ctx.pc = ret_from_fork as u64;

// 修复后
child_ctx.ra = ret_from_fork as u64;
```

**同时简化 context_switch 逻辑**:

**文件**: `kernel/src/sched/sched.rs`

```rust
// 删除复杂的 fork 子进程特殊处理代码
// fork 子进程走标准的内核上下文切换路径

pub fn context_switch(next: &Arc<Task>) {
    let current = current_task();

    // 设置 next 的 thread_info
    next.ti_cpu = cpu_id() as u32;  // 修复 cpu_id() 返回无效值

    // 标准 context switch
    unsafe {
        cpu_switch_to(&mut next.cpu_context, &mut current.cpu_context);
    }
}
```

**Commit**: `6127d94 fix(fork): 修复 fork 子进程的上下文切换和 COW 处理`

---

## 6. 问题五：execve 实现

### 6.1 需求

execve 需要替换当前进程的地址空间，加载新程序，但保持 PID 不变。

### 6.2 实现方案

**文件**: `kernel/src/syscall/process.rs`

```rust
pub fn sys_execve(pathname: *const u8, argv: *const *const u8, envp: *const *const u8) -> i64 {
    // 1. 从 ext4 读取 ELF 文件
    let elf_data = read_file_from_mounted(path)?;

    // 2. 解析 ELF
    let elf = parse_elf(&elf_data)?;

    // 3. 创建新的地址空间
    let new_page_table = create_address_space()?;

    // 4. 加载 ELF 段
    for segment in elf.segments {
        map_segment(&new_page_table, segment)?;
    }

    // 5. 设置用户栈
    let stack_top = setup_user_stack(&new_page_table, argv, envp)?;

    // 6. 修改 trap frame 以返回到新程序
    let task = current_task();
    task.user_context.sepc = elf.entry;
    task.user_context.sp = stack_top;

    // 7. 切换到新页表
    switch_page_table(new_page_table);

    // 8. 返回到用户态（实际通过 sret）
    0
}
```

### 6.3 关键点

1. **保持 PID**: execve 不创建新进程，只替换地址空间
2. **栈布局**: argc, argv, envp, auxv 需要按 musl libc 期望的格式放置
3. **页表切换**: 需要在正确的时机切换页表
4. **寄存器初始化**: sepc 设置为入口点，sp 设置为栈顶

**Commit**: `bfd9404 feat(syscall): 实现 execve 系统调用基础框架`

---

## 7. 调试技巧总结

### 7.1 汇编级调试

```bash
# 使用 GDB 调试
riscv64-unknown-elf-gdb target/riscv64gc-unknown-none-elf/debug/rux

# 在 trap 入口设置断点
(gdb) break trap_entry
(gdb) break ret_from_fork

# 查看寄存器
(gdb) info registers
(gdb) p/x $tp
(gdb) p/x $sscratch
```

### 7.2 页表调试

```rust
// 添加调试输出
fn dump_page_table(root: PhysAddr) {
    for vpn in 0..512 {
        let pte = read_pte(root, vpn);
        if pte & PTE_V != 0 {
            println!("VPN {}: PTE = {:#x}, PPN = {:#x}",
                vpn, pte, (pte >> 10) & 0x3FFFFFFF);
        }
    }
}
```

### 7.3 上下文调试

```rust
// 在 context_switch 前后打印信息
fn context_switch(next: &Arc<Task>) {
    println!("Switching from PID {} to PID {}",
        current_task().pid, next.pid);
    println!("  current ra = {:#x}", current_task().cpu_context.ra);
    println!("  next ra = {:#x}", next.cpu_context.ra);

    unsafe { cpu_switch_to(...) };

    println!("Returned to PID {}", current_task().pid);
}
```

---

## 8. 验证测试

### 8.1 fork 测试

```bash
# 在 Rux shell 中
/bin/toybox ls
# 预期：toybox fork 子进程执行 ls 命令，shell 正确返回
```

### 8.2 COW 测试

```c
// test_cow.c
int main() {
    int x = 42;
    int pid = fork();

    if (pid == 0) {
        // 子进程修改 x
        x = 100;
        printf("Child: x = %d\n", x);
    } else {
        // 父进程等待
        wait(NULL);
        printf("Parent: x = %d\n", x);  // 应该还是 42
    }
    return 0;
}
```

### 8.3 mini-ltp 测试

```bash
cd /test/mini-ltp
./run_tests.sh
# 预期：test_fork, test_execve 等测试通过
```

---

## 9. 相关提交

| Commit | 描述 |
|--------|------|
| `d5c82c7` | 实现 Linux 风格的 sscratch 检测机制 |
| `33415ca` | 修复 trap 处理的 task_struct 偏移量 |
| `bfd9404` | 实现 execve 系统调用基础框架 |
| `2839915` | 修复 fork 子进程的 COW 实现和上下文切换 |
| `6127d94` | 修复 fork 子进程的上下文切换和 COW 处理 |

---

## 10. 经验教训

1. **参考 Linux 实现**: 操作系统内核开发必须参考 Linux 源码，不要"创新"
2. **理解 ABI 约定**: 系统调用和上下文切换有严格的寄存器使用约定
3. **TLB 一致性**: 修改页表后必须刷新 TLB，且顺序很重要
4. **使用正确的分配器**: 用户内存和内核内存使用不同的分配器
5. **汇编与 Rust 配合**: naked 函数和汇编需要仔细检查寄存器约定

---

**报告编写时间**: 2026-03-04
**最后更新**: 2026-03-04
