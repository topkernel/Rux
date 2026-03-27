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

Every kernel message is also written to `/kmsg.log` on the ext4 filesystem. This provides crash survivability: if the kernel panics, the log file retains the most recent messages across reboots.

### Configuration

- **File path**: `/kmsg.log` (root directory)
- **Maximum size**: 1MB (ring buffer style, wraps around)
- **Format**: `[seq] [timestamp_us] <level> message\n`

### How It Works

1. `printk::persistent_log_init()` is called after ext4 mount during boot
2. Every call to `printk()` / `println!()` / `pr_debug!()` triggers a write
3. Writes are synchronous (ext4 block I/O) — no data loss on panic
4. When the file exceeds 1MB, writes wrap to the beginning

### Reading the Log After a Crash

```bash
# Reboot and check the persistent log
cat /kmsg.log

# Show the last 50 lines
cat /kmsg.log | tail -50
```

### Disabling

To disable persistent logging, comment out the initialization in `kernel/src/main.rs`:

```rust
// printk::persistent_log_init();
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

1. **Check persistent log** — reboot and `cat /kmsg.log` to see the last messages before the hang
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
2. Check `/kmsg.log` for kernel-side diagnostics (page faults, signals)
3. Look for `SIGSEGV` or `SIGKILL` signals in the log

### 4. Filesystem Issues

1. Check `/kmsg.log` for ext4 errors
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
