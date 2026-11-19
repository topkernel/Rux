# User Program Development Guide

This document explains how to develop and run user programs in Rux OS.

**Last Updated**: 2026-03-04
**Status**: Shell, Toybox, GUI applications fully operational

---

## Table of Contents

- [Overview](#overview)
- [User Program Types](#user-program-types)
- [no_std User Programs](#no_std-user-programs)
- [musl libc Programs](#musl-libc-programs)
- [System Calls](#system-calls)
- [Debugging Tips](#debugging-tips)

---

## Overview

Rux OS supports RISC-V 64-bit user programs through the following mechanisms:

1. **ELF Loader** - Parses and loads ELF format user programs
2. **User Mode Switching** - Uses sret instruction to switch from S-mode to U-mode
3. **System Call Handling** - Uses ecall instruction to enter kernel from user mode
4. **Single Page Table Approach** - Linux style, permissions controlled via U-bit

### User Program Execution Flow

```
+-------------------------------------------------------------+
| 1. Kernel loads user program ELF into memory                |
|    - Parse ELF program headers                              |
|    - Allocate physical memory pages                         |
|    - Map to user virtual address space                      |
+-------------------------------------------------------------+
                          |
                          v
+-------------------------------------------------------------+
| 2. Kernel switches to user mode                             |
|    - Set sstatus.SPP=0 (return to U-mode)                   |
|    - Set sepc=user program entry point                      |
|    - Set sp=user stack pointer                              |
|    - Execute sret                                           |
+-------------------------------------------------------------+
                          |
                          v
+-------------------------------------------------------------+
| 3. User program execution                                   |
|    - Run in user mode (U-mode)                              |
|    - Can call system calls (ecall)                          |
+-------------------------------------------------------------+
```

---

## User Program Types

Rux OS supports multiple types of user programs:

| Type | Status | Description |
|------|--------|-------------|
| **no_std Rust** | Fully operational | Bare metal Rust programs, no standard library |
| **musl libc C** | Fully operational | C programs, verified with Toybox |
| **GUI Applications** | Fully operational | Desktop environment, calculator, clock |

### Currently Available User Programs

| Program | Type | Description |
|---------|------|-------------|
| `/bin/shell` | no_std Rust | Default Shell |
| `/bin/toybox` | musl libc | BusyBox replacement |
| `/app/desktop` | musl libc + GUI | Desktop environment |
| `/app/calculator` | musl libc + GUI | Calculator |
| `/app/clock` | musl libc + GUI | Clock |
| `/app/vshell` | musl libc + GUI | Visual Shell |

---

## no_std User Programs

### Minimal Example

```rust
#![no_std]
#![no_main]

use core::panic::PanicInfo;

// System call numbers (RISC-V Linux ABI)
const SYS_EXIT: u64 = 93;

// System call function
pub unsafe fn syscall1(n: u64, a0: u64) -> u64 {
    let mut ret: u64;
    core::arch::asm!(
        "ecall",
        inlateout("a7") n => _,
        inlateout("a0") a0 => ret,
        lateout("a1") _,
        options(nostack, nomem)
    );
    ret
}

// Program entry point
#[no_mangle]
pub extern "C" fn _start() -> ! {
    // Call sys_exit(0)
    syscall1(SYS_EXIT, 0);

    loop {
        unsafe { core::arch::asm!("nop", options(nomem, nostack)); }
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        unsafe { core::arch::asm!("nop", options(nomem, nostack)); }
    }
}
```

---

## musl libc Programs

### Build Toolchain

```bash
cd toolchain
bash build-musl.sh
```

### C Program Example

```c
#include <unistd.h>
#include <stdio.h>

int main(int argc, char *argv[]) {
    printf("Hello from Rux OS!\n");
    return 0;
}
```

### Compilation

```bash
riscv64-linux-gnu-gcc -static -o hello hello.c
```

### musl Linker Script

User space program memory layout:
- TEXT: 0x10000 (1MB)
- DATA: 0x110000 (512KB)
- HEAP: 0x190000 (2MB)
- STACK: 0x390000 (128KB)

---

## System Calls

### System Call Convention

**Register Convention** (RISC-V Linux ABI):
- `a7`: System call number
- `a0-a5`: Parameters (up to 6)
- `a0`: Return value

### Implemented System Calls (80+)

**File Operations**:

| Syscall Number | Name | Description |
|----------------|------|-------------|
| 56 | sys_openat | Open file |
| 57 | sys_close | Close file |
| 63 | sys_read | Read file |
| 64 | sys_write | Write file |
| 62 | sys_lseek | Seek file |
| 80 | sys_fstat | Get file status |

**Process Operations**:

| Syscall Number | Name | Description |
|----------------|------|-------------|
| 93 | sys_exit | Exit process |
| 172 | sys_getpid | Get process ID |
| 110 | sys_getppid | Get parent process ID |
| 220 | sys_clone | Create process/thread |
| 221 | sys_execve | Execute program |
| 260 | sys_wait4 | Wait for child process |

**Memory Operations**:

| Syscall Number | Name | Description |
|----------------|------|-------------|
| 214 | sys_brk | Adjust heap |
| 222 | sys_mmap | Memory mapping |
| 215 | sys_munmap | Unmap memory |
| 226 | sys_mprotect | Modify protection |

**Network Operations**:

| Syscall Number | Name | Description |
|----------------|------|-------------|
| 198 | sys_socket | Create socket |
| 200 | sys_bind | Bind address |
| 201 | sys_listen | Listen for connections |
| 202 | sys_accept | Accept connection |
| 203 | sys_connect | Initiate connection |

---

## Debugging Tips

### 1. Adding Debug Output

```rust
#[no_mangle]
pub extern "C" fn _start() -> ! {
    // Debug: Write characters to UART
    unsafe {
        const UART: u64 = 0x10000000;
        core::ptr::write_volatile(UART as *mut u8, b'H');
        core::ptr::write_volatile(UART as *mut u8, b'i');
    }

    syscall1(93, 0);
    loop { core::arch::asm!("nop", options(nomem, nostack)); }
}
```

### 2. Using GDB Debugging

```bash
# Start QEMU with GDB support
qemu-system-riscv64 -M virt -nographic -kernel rux.elf -s -S

# In another terminal, start GDB
riscv64-unknown-elf-gdb
(gdb) target remote :1234
(gdb) break _start
(gdb) continue
```

### 3. mini-ltp Testing

```bash
# Run in Rux
cd /test/mini-ltp
./run_tests.sh
```

---

## rootfs Directory Structure

```
/
+-- bin/                # Basic commands
|   +-- shell           # Shell
|   +-- sh -> shell     # Shell symlink
|   +-- toybox          # Toybox
|   +-- ls -> toybox    # Common command symlinks
|   +-- cat -> toybox
|
+-- app/                # GUI applications
|   +-- desktop         # Desktop environment
|   +-- calculator      # Calculator
|   +-- clock           # Clock
|   +-- vshell          # Visual Shell
|
+-- test/               # Test programs
|   +-- mini-ltp/       # Kernel compatibility tests
|
+-- dev/                # Device files
+-- proc/               # procfs mount point
+-- tmp/                # Temporary files
```

---

## References

- [RISC-V Linux ABI](https://github.com/riscv-non-isa/riscv-elf-psabi-doc)
- [RISC-V Privileged Architecture Specification](https://riscv.org/specifications/privileged-isa/)
- [ELF Format Specification](https://refspecs.linuxfoundation.org/elf/elf.pdf)
- [Linux System Call Table](https://github.com/torvalds/linux/blob/master/arch/riscv/include/asm/unistd.h)

---

**Document Version**: v2.0.0
**Last Updated**: 2026-03-04
