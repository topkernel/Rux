# Debugging Guide

This guide covers the debugging infrastructure in Rux and how to diagnose kernel issues.

## Kernel Logging

### Log Levels

Rux uses Linux-compatible log levels (lower = higher priority):

| Level | Name     | Description              |
|-------|----------|--------------------------|
| 0     | emerg    | System is unusable       |
| 1     | alert    | Immediate action needed  |
| 2     | crit     | Critical conditions      |
| 3     | err      | Error conditions         |
| 4     | warn     | Warning conditions       |
| 5     | notice   | Normal but significant  |
| 6     | info     | Informational            |
| 7     | debug    | Debug-level messages     |

### Logging Macros

```rust
use crate::printk;

pr_emerg!("system unusable: {}", reason);   // Level 0
pr_err!("failed to allocate: {}", err);      // Level 3
pr_warn!("deprecated API called");           // Level 4
pr_info!("device initialized");              // Level 6
pr_debug!("value: {:#x}", val);             // Level 7 (debug builds only)
println!("boot: phase {} complete", n);      // Level 6 (alias for pr_info!)
```

### Console Output

During boot, all log levels are printed to the UART serial console. After the scheduler starts, the console log level is set to 0 (emergencies only):

```rust
// kernel/src/main.rs
printk::set_console_loglevel(0);
```

To re-enable console output from userspace:

```bash
# Show all messages (equivalent to Linux's dmesg -n 8)
dmesg -n 7
```

## kmsg Ring Buffer

All kernel messages are stored in a 1MB in-memory ring buffer, regardless of the console log level. This buffer is readable via:

- **`/proc/kmsg`** — the standard procfs interface
- **`syslog(2)` syscall** — action type 2 (read sequential) or 3 (read all)
- **`dmesg`** command — reads and displays the buffer

Each record stores the log level, a CLINT timestamp, a sequence number, and up to 256 bytes of text. When the buffer fills, oldest records are overwritten.

## Persistent Log (kmsg to Disk)

**Currently disabled by default**: the initialization call is commented
out in `kernel/src/main.rs` (`// printk::persistent_log_init();`). When
enabled, every kernel message is also written to `/var/log/kmsg` on the
ext4 filesystem, providing crash survivability: if the kernel panics, the
log file retains the most recent messages across reboots.

### Configuration

- **File path**: `/var/log/kmsg`
- **Maximum size**: 1MB (ring buffer style, wraps around)
- **Format**: `[seq] [timestamp_us] <level> message\n`

### How It Works

1. `printk::persistent_log_init()` is called after ext4 mount during boot
   (currently commented out in `kernel/src/main.rs`)
2. Every call to `printk()` / `println!()` / `pr_debug!()` triggers a write
3. Writes are synchronous (ext4 block I/O) — no data loss on panic
4. When the file exceeds 1MB, writes wrap to the beginning

### Reading the Log After a Crash

```bash
# Reboot and check the persistent log
cat /var/log/kmsg

# Show the last 50 lines
cat /var/log/kmsg | tail -50
```

### Enabling / Disabling

To enable persistent logging, uncomment the initialization in
`kernel/src/main.rs` (and rebuild):

```rust
printk::persistent_log_init();
```

## Panic Handler

When the kernel panics, the panic handler outputs detailed diagnostic information directly to the UART serial port (bypassing the console log level). The system then halts with `wfi` instructions.

### Output Format

```
Kernel panic - not syncing:

PANIC: <panic message>
  Location: <file>:<line>

---[ end Kernel panic - not syncing ]---

Sstatus: 0000000000000000
Scause : 0000000000000000
Stval  : 0000000000000000
Sepc   : 0000000000000000

Registers:
  ra  : 0000000000000000  sp  : 0000000000000000  gp  : 0000000000000000  tp  : 0000000000000000
  t0  : 0000000000000000  t1  : 0000000000000000  t2  : 0000000000000000  s0  : 0000000000000000
  s1  : 0000000000000000  a0  : 0000000000000000  a1  : 0000000000000000  a2  : 0000000000000000
  a3  : 0000000000000000  a4  : 0000000000000000  a5  : 0000000000000000  a6  : 0000000000000000
  a7  : 0000000000000000  s2  : 0000000000000000  s3  : 0000000000000000  s4  : 0000000000000000
  s5  : 0000000000000000  s6  : 0000000000000000  s7  : 0000000000000000  s8  : 0000000000000000
  s9  : 0000000000000000  s10 : 0000000000000000  s11 : 0000000000000000  t3  : 0000000000000000
  t4  : 0000000000000000  t5  : 0000000000000000  t6  : 0000000000000000

Call trace:
  [<0xffffffff8005de36>] (current)
  [<0xffffffff801154b1>]
  [<0xffffffff80012345>]
```

### What Gets Printed

- **Panic message and source location** — the `panic!()` arguments and file:line
- **CSR registers** — `sstatus`, `scause`, `stval`, `sepc`
- **All 31 GPRs** — saved immediately via inline assembly before any stack unwinding
- **Stack backtrace** — walks the frame pointer chain (`s0/fp`), up to 32 frames

### Interpreting the Output

**Scause** (Supervisor Cause Register):
- Bit 63 set = interrupt, clear = exception
- Exception codes: 0 = instruction misaligned, 2 = illegal instruction, 8 = ecall from U-mode, 12 = instruction page fault, 13 = load page fault, 15 = store/AMO page fault
- Interrupt codes: 5 = supervisor timer, 9 = supervisor external

**Sepc** (Supervisor Exception Program Counter):
- The address of the instruction that caused the trap

**Stval** (Supervisor Trap Value):
- For page faults: the faulting virtual address
- For illegal instructions: the instruction bits

**Call trace**:
- The first address (`ra`) is the return address of the panicking function
- Subsequent addresses are caller return addresses, walking up the call stack
- Use `addr2line` or the kernel symbol table to resolve addresses to source locations

### Triggering a Panic for Testing

```rust
// In any kernel code path:
panic!("test panic for debugging");

// In a specific condition:
if some_condition {
    panic!("unexpected state: val={}", val);
}
```

## Debugging Workflow

### 1. Kernel Hangs (No Output)

If the kernel hangs with no visible output:

1. **Check persistent log** — reboot and `cat /var/log/kmsg` to see the last messages before the hang
2. **Enable console debug output** — change `printk::set_console_loglevel(0)` to `printk::set_console_loglevel(7)` in `main.rs` to see all messages on serial
3. **Add panic checkpoints** — insert `panic!("reached point X")` at various points to narrow down where the hang occurs

### 2. Page Fault in Kernel

If you see a kernel page fault:

1. Check `Stval` — the faulting address
2. Check `Sepc` — which instruction caused it
3. Check `Call trace` — the call chain leading to the fault
4. Common causes:
   - Null pointer dereference
   - Use-after-free
   - Accessing unmapped memory
   - Kernel stack overflow

### 3. Userspace Program Crashes

If a userspace program crashes:

1. Check `/proc/[pid]/maps` for the process's memory layout
2. Check `/var/log/kmsg` for kernel-side diagnostics (page faults, signals)
3. Look for `SIGSEGV` or `SIGKILL` signals in the log

### 4. Filesystem Issues

1. Check `/var/log/kmsg` for ext4 errors
2. Use `println!` (level 6) in the relevant code path — these are always logged to both ring buffer and persistent file
3. Verify the rootfs image integrity: `e2fsck -f test/rootfs.img`

## Build Modes

### Debug Build (default)

- `pr_debug!` macros are active
- Full panic output with register dump and stack trace
- Useful for active development

```bash
make build
```

### Release Build

- `pr_debug!` macros are compiled out (zero overhead)
- Panic handler still produces full diagnostic output
- Better optimization but harder to debug

```bash
make build RELEASE=1
```

## Source Files

| File | Description |
|------|-------------|
| `kernel/src/printk.rs` | printk ring buffer, log levels, persistent log |
| `kernel/src/main.rs` | panic handler, boot sequence |
| `kernel/src/console.rs` | UART serial driver |
| `kernel/src/arch/riscv64/trap.rs` | Trap/exception handling |
| `kernel/src/arch/riscv64/pt_regs.rs` | PtRegs structure (register layout) |

## DFX 可开关诊断特性

内核的可观测性手段沉淀在 `kernel/src/dfx/`，分两类开关：**编译期 feature**（热路径钩子，关闭时零开销）与**运行时 boot 参数开关**（`dfx=...`，逐项启用）。

### 编译期 feature（Cargo.toml）

| Feature | 开销 | 用途 |
|---|---|---|
| `dfx-lock-owner` | 每次锁/解锁各 1 次原子 store | 自旋锁持有者跟踪；死锁告警打印 `holder=<cpu>` |

```bash
# 平时构建（零 DFX 开销）
cargo build --target riscv64gc-unknown-none-elf --features riscv64
# 追锁死锁时
cargo build --target riscv64gc-unknown-none-elf --features riscv64,dfx-lock-owner
```

### 运行时开关（boot 参数 `dfx=`，逗号分隔）

| 开关 | 用途 |
|---|---|
| `watchdog` | 自旋锁死锁告警之后自动打印全任务状态快照（SBI 直写，不依赖 printk） |
| `taskdump` | 预留：按需任务快照触发钩 |

```bash
-append "root=/dev/vda rw console=ttyS0 dfx=watchdog"
```

### 死锁/挂起现场捕获工具

`test/hunt-wedge.sh` 自动构建带 `dfx-lock-owner` 的内核，循环跑
smoke+nettest+管道轰炸，检测两类挂起（A 型：锁死锁告警；B 型：管道静默无输出
>30s），任一触发即通过 QEMU monitor 抓取每 CPU 寄存器/栈/反汇编到
`/tmp/rux-hunt/`，并保留当轮 `kernel.elf` 供 `addr2line` 符号化。

```bash
./test/hunt-wedge.sh [轮数]   # 默认 6 轮；捕获即停，exit 0=捕获, 2=未复现
```

现场分析方法：`hunt.log` 判型（DEADLOCK 行/管道输出断流），`hunt.dump` 中
`pc=`/`ra=` 用 `riscv64-linux-gnu-addr2line -e /tmp/rux-hunt/kernel.elf -f -C <addr>`
符号化；栈字按内核 `ra` 惯例扫描（返回地址列表逐一符号化即得调用链）。
