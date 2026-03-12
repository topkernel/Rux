# Fork + Execve Debug Report

**Date**: 2026-03-01 ~ 2026-03-04
**Debuggers**: Fei Wang + Claude Code
**Status**: ✅ Resolved

---

## 1. Background

During the implementation of complete Unix-style process management, the `fork()` and `execve()` system calls encountered a series of complex issues:

1. **Fork child processes could not properly return to user space**
2. **COW (Copy-on-Write) page table handling errors**
3. **Register state lost during context switching**
4. **Incorrect task_struct offset in trap handling**

---

## 2. Issue 1: task_struct Offset Error

### 2.1 Symptoms

- Fork child processes accessed invalid memory addresses when handling traps
- System crashed or hung

### 2.2 Debugging Process

By analyzing `trap.S` assembly code and `Task` structure layout:

```asm
# trap.S original code
ld sp, TASK_TI_KERNEL_SP(tp)  # Load kernel stack pointer
```

Checking the Task structure:

```rust
// kernel/src/process/task.rs
pub struct Task {
    // thread_info embedded at the beginning
    pub ti_cpu: u32,           // offset 0x00
    pub ti_preempt_count: u32, // offset 0x04
    pub ti_kernel_sp: u64,     // offset 0x08 ← actual offset
    pub ti_user_sp: u64,       // offset 0x10
    // ...
}
```

### 2.3 Root Cause

The `TASK_TI_KERNEL_SP` constant was defined as `0x10`, but `ti_kernel_sp` actually has an offset of `0x08` in the structure.

`0x10` is the offset of `ti_user_sp`, causing the wrong stack pointer to be loaded.

### 2.4 Solution

**File**: `kernel/src/arch/riscv64/trap.S`

```asm
# Before fix
.equ TASK_TI_KERNEL_SP, 0x10

# After fix
.equ TASK_TI_KERNEL_SP, 0x08
```

**Commit**: `33415ca fix(arch): fix task_struct offset in trap handling and init process kernel stack`

---

## 3. Issue 2: sscratch Detection Mechanism

### 3.1 Symptoms

- Could not correctly distinguish whether a trap came from user mode or kernel mode
- User mode traps were misidentified as kernel mode traps, causing incorrect stack pointers

### 3.2 Linux Standard Approach

Linux uses the `sscratch` register to implement efficient trap source detection:

```asm
# Linux entry.S
handle_exception:
    csrrw tp, sscratch, tp   # Atomic swap tp and sscratch
    bnez tp, .Lsave_context  # tp != 0 means from user mode
                             # tp == 0 means from kernel mode
```

**Principle**:
- When running in user mode: `sscratch = current_task`, `tp = user TLS`
- When running in kernel mode: `sscratch = 0`, `tp = current_task`
- Swap on trap entry, determine source by tp value

### 3.3 Rux Implementation

**File**: `kernel/src/arch/riscv64/trap.S`

```asm
trap_entry:
    csrrw tp, sscratch, tp    # Swap tp and sscratch
    bnez tp, .Lfrom_user      # Non-zero = user mode
    j .Lfrom_kernel           # Zero = kernel mode
```

**File**: `kernel/src/sched/sched.rs`

```rust
pub fn init() {
    // Initialize sscratch = 0 (indicates kernel mode)
    unsafe {
        csrw_sscratch(0);
    }
    // tp points to idle task
    switch_to(&mut idle_task);
}
```

**Commit**: `d5c82c7 feat(arch): implement Linux-style sscratch detection mechanism`

---

## 4. Issue 3: COW Page Table Copy Error

### 4.1 Symptoms

- After fork, parent and child processes shared the same physical pages
- Page faults were not triggered on write
- Or page faults could not be handled correctly

### 4.2 Debugging Process

Analyzing the `copy_page_table` function:

```rust
// Original code issue
let pfn = (pte >> 10) << 12;  // Error: redundant shifting
```

### 4.3 Root Cause

1. **PFN calculation error**: The PPN (Physical Page Number) in PTE is already the physical page number, no need to left-shift 12 bits again
2. **COW flags not set correctly**: Need to modify PTEs of both parent and child processes to read-only
3. **TLB not flushed**: TLB was not flushed after modifying page tables

### 4.4 Solution

**File**: `kernel/src/arch/riscv64/mm/base.rs`

```rust
// Correct COW implementation
pub fn copy_page_table(src_root: PhysAddr, dst_root: PhysAddr) -> Result<(), i32> {
    for vpn in 0..512 {
        let src_pte = read_pte(src_root, vpn);

        if src_pte & PTE_V != 0 && src_pte & PTE_R != 0 {
            // Get physical page number
            let ppn = (src_pte >> 10) & 0x3FFFFFFF;  // PPN[2:0]
            let phys_addr = ppn << 12;

            // Mark as COW: clear write permission, set COW flag
            let cow_pte = (src_pte & !PTE_W) | PTE_COW;

            // Update both parent and child PTEs
            write_pte(src_root, vpn, cow_pte);
            write_pte(dst_root, vpn, cow_pte);

            // Increment page reference count
            inc_page_ref_count(phys_addr);
        }
    }

    // Flush TLB
    sfence_vma();
    Ok(())
}
```

**COW Page Fault Handling**:

```rust
pub fn handle_cow_fault(vaddr: VirtAddr) -> Result<PhysAddr, i32> {
    let pte = get_pte(vaddr)?;
    let old_phys = pte_to_phys(pte);

    // Allocate new physical page
    let new_phys = alloc_user_phys_page()?;

    // Copy data
    memcpy(new_phys, old_phys, PAGE_SIZE);

    // Decrement old page reference count
    if dec_page_ref_count(old_phys) == 0 {
        free_user_phys_page(old_phys);
    }

    // Update PTE: writable, clear COW flag
    let new_pte = (pte & !PTE_COW) | PTE_W | phys_to_pte(new_phys);
    update_pte(vaddr, new_pte);

    // Flush TLB
    sfence_vma_addr(vaddr);

    Ok(new_phys)
}
```

**Key Fixes**:

1. **TLB flush order**: Update page table entries first, then flush TLB

```rust
// Wrong order
sfence_vma();        // Flush TLB first
write_pte(...);      // Update page table later

// Correct order
write_pte(...);      // Update page table first
sfence_vma();        // Flush TLB later
```

2. **Use user physical allocator**: fork and COW should use `alloc_user_phys_page()` instead of kernel allocator

**Commit**: `2839915 fix(fork): fix COW implementation and context switching for fork child processes`

---

## 5. Issue 4: Fork Child Process Context Switch

### 5.1 Symptoms

- Fork child process crashed immediately after being scheduled
- Or child process returned to wrong address

### 5.2 Debugging Process

Analyzing `cpu_switch_to` and fork child process initialization:

```rust
// Original code: set pc register
child_ctx.pc = ret_from_fork as u64;
```

But `cpu_switch_to` restores the `ra` register, then executes the `ret` instruction to jump to the address pointed to by `ra`.

### 5.3 Root Cause

`cpu_switch_to` uses the `ret` instruction to return, which jumps to the address stored in the `ra` register, not `pc`.

Therefore, `ra` should be set instead of `pc`.

### 5.4 Solution

**File**: `kernel/src/process/fork.rs`

```rust
// Before fix
child_ctx.pc = ret_from_fork as u64;

// After fix
child_ctx.ra = ret_from_fork as u64;
```

**Also simplify context_switch logic**:

**File**: `kernel/src/sched/sched.rs`

```rust
// Remove complex fork child process special handling code
// Fork child processes use standard kernel context switch path

pub fn context_switch(next: &Arc<Task>) {
    let current = current_task();

    // Set next's thread_info
    next.ti_cpu = cpu_id() as u32;  // Fix cpu_id() returning invalid value

    // Standard context switch
    unsafe {
        cpu_switch_to(&mut next.cpu_context, &mut current.cpu_context);
    }
}
```

**Commit**: `6127d94 fix(fork): fix context switching and COW handling for fork child processes`

---

## 6. Issue 5: execve Implementation

### 6.1 Requirements

execve needs to replace the current process's address space and load a new program while keeping the PID unchanged.

### 6.2 Implementation Plan

**File**: `kernel/src/syscall/process.rs`

```rust
pub fn sys_execve(pathname: *const u8, argv: *const *const u8, envp: *const *const u8) -> i64 {
    // 1. Read ELF file from ext4
    let elf_data = read_file_from_mounted(path)?;

    // 2. Parse ELF
    let elf = parse_elf(&elf_data)?;

    // 3. Create new address space
    let new_page_table = create_address_space()?;

    // 4. Load ELF segments
    for segment in elf.segments {
        map_segment(&new_page_table, segment)?;
    }

    // 5. Set up user stack
    let stack_top = setup_user_stack(&new_page_table, argv, envp)?;

    // 6. Modify trap frame to return to new program
    let task = current_task();
    task.user_context.sepc = elf.entry;
    task.user_context.sp = stack_top;

    // 7. Switch to new page table
    switch_page_table(new_page_table);

    // 8. Return to user mode (actually via sret)
    0
}
```

### 6.3 Key Points

1. **Preserve PID**: execve does not create a new process, only replaces the address space
2. **Stack layout**: argc, argv, envp, auxv need to be placed in the format expected by musl libc
3. **Page table switch**: Need to switch page tables at the right time
4. **Register initialization**: sepc set to entry point, sp set to stack top

**Commit**: `bfd9404 feat(syscall): implement execve system call basic framework`

---

## 7. Debugging Tips Summary

### 7.1 Assembly-Level Debugging

```bash
# Debug with GDB
riscv64-unknown-elf-gdb target/riscv64gc-unknown-none-elf/debug/rux

# Set breakpoints at trap entry
(gdb) break trap_entry
(gdb) break ret_from_fork

# View registers
(gdb) info registers
(gdb) p/x $tp
(gdb) p/x $sscratch
```

### 7.2 Page Table Debugging

```rust
// Add debug output
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

### 7.3 Context Debugging

```rust
// Print information before and after context_switch
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

## 8. Verification Tests

### 8.1 fork Test

```bash
# In Rux shell
/bin/toybox ls
# Expected: toybox fork child process executes ls command, shell returns correctly
```

### 8.2 COW Test

```c
// test_cow.c
int main() {
    int x = 42;
    int pid = fork();

    if (pid == 0) {
        // Child process modifies x
        x = 100;
        printf("Child: x = %d\n", x);
    } else {
        // Parent waits
        wait(NULL);
        printf("Parent: x = %d\n", x);  // Should still be 42
    }
    return 0;
}
```

### 8.3 mini-ltp Test

```bash
cd /test/mini-ltp
./run_tests.sh
# Expected: test_fork, test_execve, etc. pass
```

---

## 9. Related Commits

| Commit | Description |
|--------|-------------|
| `d5c82c7` | Implement Linux-style sscratch detection mechanism |
| `33415ca` | Fix task_struct offset in trap handling |
| `bfd9404` | Implement execve system call basic framework |
| `2839915` | Fix COW implementation and context switching for fork child processes |
| `6127d94` | Fix context switching and COW handling for fork child processes |

---

## 10. Lessons Learned

1. **Reference Linux implementation**: OS kernel development must reference Linux source code, do not "innovate"
2. **Understand ABI conventions**: System calls and context switches have strict register usage conventions
3. **TLB consistency**: TLB must be flushed after modifying page tables, and order matters
4. **Use correct allocator**: User memory and kernel memory use different allocators
5. **Assembly and Rust coordination**: Naked functions and assembly require careful checking of register conventions

---

**Report Written**: 2026-03-04
**Last Updated**: 2026-03-04
