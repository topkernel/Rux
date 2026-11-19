# PSCI Debugging Record Document

**Date**: 2025-02-04
**Problem**: SMP (Symmetric Multi-Processing) Unable to Boot Secondary Cores
**Status**: Resolved
**Solution**: Use Correct PSCI Calling Method (HVC instead of SMC)

---

## 1. Problem Description

### 1.1 Initial Symptoms

While attempting to implement SMP (Symmetric Multi-Processing) support in the Rux kernel using PSCI (Power State Coordination Interface) to boot secondary cores, the following issues were encountered:

1. **SMC Call Causes Complete QEMU Hang**
   - Used `smc #0` instruction to call PSCI_CPU_ON
   - QEMU produced no output, completely deadlocked

2. **HVC Call Returns PSCI_RET_NOT_SUPPORTED**
   - Used `hvc #0` instruction to call PSCI_CPU_ON
   - Return value `0xEFFFFFFFFFFFFFFF` (-1), indicating not supported

3. **Secondary Cores Never Booted**
   - Added debug output to `secondary_entry` (write character '1' to UART)
   - No output, indicating secondary cores never reached the entry point

### 1.2 Environment Information

- **Platform**: QEMU virt machine (ARM virtualization platform)
- **CPU**: cortex-a57 (ARMv8-A)
- **Kernel**: Rux v0.1.0 (Rust, no_std)
- **Boot EL**: EL1 (confirmed via CurrentEL register)
- **PSCI Version**: 1.1 (0x10001000)

---

## 2. Problem Analysis

### 2.1 PSCI Basics

**PSCI (Power State Coordination Interface)** is ARM's standard power management interface used for:
- CPU power control (CPU_ON, CPU_OFF, CPU_SUSPEND)
- CPU hot-plug
- System-level power management

**PSCI Calling Methods**:
- **SMC (Secure Monitor Call)**: Used in secure monitor environments (EL3)
- **HVC (Hypervisor Call)**: Used in virtualization environments (EL2)

**PSCI Function IDs**:

| Function | HVC Call ID | SMC Call ID |
|----------|-------------|-------------|
| PSCI_VERSION | 0x84000000 | 0xC4000000 |
| PSCI_CPU_ON | 0x84000003 | 0xC4000003 |
| PSCI_CPU_OFF | 0x84000001 | 0xC4000001 |
| PSCI_CPU_SUSPEND | 0x84000002 | 0xC4000002 |

**Return Values**:
- `0`: Success
- Non-0: Error code (see PSCI specification)

### 2.2 QEMU virt's PSCI Implementation

QEMU virt machine provides PSCI services through **firmware implementation**:

1. **Default Behavior**: QEMU internally implements PSCI 1.0/1.1
2. **Calling Method**: Specified by device tree (`method` property)
3. **Boot EL**: Usually at EL2, but configurable

### 2.3 Debugging Steps

#### Step 1: Check Device Tree

```bash
# Start QEMU and export device tree
qemu-system-aarch64 -M virt -cpu cortex-a57 -m 2G -smp 2 \
  -dtb virt.dtb -kernel test.elf

# Export device tree
dtc -I dtb -O dts virt.dtb > virt.dts
```

**Key Finding** (`psci` node):
```dts
psci {
    compatible = "arm,psci-1.0", "arm,psci-0.2";
    method = "hvc";    <- Key: Uses HVC call
    cpu_on = <0xc4000003>;
    cpu_suspend = <0xc4000001>;
};
```

#### Step 2: Verify QEMU Boot EL

Create test program to check exception level:

```assembly
/* test_el.s */
.section .text
.global _start
_start:
    mrs     x0, CurrentEL
    and     x0, x0, #0xC
    cmp     x0, #0x8    /* EL2? */
    b.eq    in_el2
    cmp     x0, #0xC    /* EL3? */
    b.eq    in_el3
    /* EL1 */
    mov     x0, #1
    b       hang
in_el2:
    mov     x0, #2
    b       hang
in_el3:
    mov     x0, #3
    b       hang
hang:
    wfe
    b       hang
```

**Result**: QEMU virt boots at **EL1**, not EL2!

#### Step 3: Test PSCI Version Query

```rust
// Test PSCI_VERSION (0x84000000 for HVC)
let psci_version: u64;
unsafe {
    core::arch::asm!(
        "hvc #0",
        inlateout("x0") 0x84000000u64 => psci_version,
        options(nomem, nostack)
    );
}
println!("PSCI version = 0x{:x}", psci_version);
```

**Result**: `PSCI version = 0x10001000` (PSCI 1.1)

**Conclusion**: PSCI is available and supports HVC calls!

---

## 3. Attempted Solutions

### 3.1 Solution 1: SMC Call (Failed)

**Code**:
```rust
// kernel/src/arch/aarch64/smp.rs
unsafe {
    let mut result: u64;
    core::arch::asm!(
        "smc #0",
        inlateout("x0") 0xC4000003u64 => result,  // PSCI_CPU_ON (SMC)
        in("x1") mpidr,
        in("x2") secondary_entry as u64,
        in("x3") 0u64,
        options(nomem, nostack)
    );
}
```

**Result**: QEMU completely hung, no output

**Reason**: SMC is Secure Monitor Call, requires EL3 or ATF support. QEMU virt default configuration has no EL3 firmware.

### 3.2 Solution 2: HVC Call (Failed)

**Code**:
```rust
// Use HVC call (0x84000003)
unsafe {
    let mut result: u64;
    core::arch::asm!(
        "hvc #0",
        inlateout("x0") 0x84000003u64 => result,  // PSCI_CPU_ON (HVC)
        in("x1") mpidr,
        in("x2") secondary_entry as u64,
        in("x3") 0u64,
        options(nomem, nostack)
    );
}
```

**Result**: Returned `PSCI_RET_NOT_SUPPORTED` (-1)

**Reason**: The code had issues at this point, actually it was incorrect function ID usage or other problems.

### 3.3 Solution 3: EL2 PSCI Call (Ineffective)

**Code** (in boot.S):
```assembly
el2_entry:
    /* Set temporary stack */
    adr     x0, boot_stack_top
    mov     sp, x0

    /* PSCI_CPU_ON call */
    movz    x0, #0x0003, lsl #0
    movk    x0, #0xC400, lsl #16    /* 0xC4000003 - SMC ID! */
    mov     x1, #1                   /* CPU ID */
    adr     x2, secondary_entry
    mov     x3, #0
    hvc     #0

    /* Drop to EL1 */
    mov     x0, #(1 << 31)
    msr     spsr_el2, x0
    adr     x0, el1_entry
    msr     elr_el2, x0
    eret
```

**Result**: Secondary cores still didn't boot

**Reason**:
1. QEMU boots at EL1, `el2_entry` never executed
2. Even at EL2, used SMC function ID (0xC4000003) instead of HVC ID (0x84000003)

---

## 4. Final Solution

### 4.1 Key Findings

1. **Device tree specifies method as "hvc"**
2. **QEMU virt boots at EL1** (not EL2)
3. **PSCI version query successful** (HVC call returns 0x10001000)
4. **Must use HVC function ID** (0x84000003 not 0xC4000003)

### 4.2 Correct Implementation

#### Step 1: Add PSCI Version Check

```rust
// kernel/src/arch/aarch64/smp.rs

pub fn boot_secondary_cpus() {
    use crate::console::putchar;

    // First check PSCI version
    const MSG_CHECK: &[u8] = b"smp: Checking PSCI version...\n";
    for &b in MSG_CHECK {
        unsafe { putchar(b); }
    }

    let psci_version: u64;
    unsafe {
        // PSCI_VERSION uses HVC call (0x84000000)
        core::arch::asm!(
            "hvc #0",
            inlateout("x0") 0x84000000u64 => psci_version,
            options(nomem, nostack)
        );
    }

    // Print version
    unsafe {
        const MSG_VER: &[u8] = b"smp: PSCI version = 0x";
        for &b in MSG_VER {
            putchar(b);
        }
        let hex = b"0123456789ABCDEF";
        let mut v = psci_version;
        for _ in 0..8 {
            let digit = (v & 0xF) as usize;
            putchar(hex[digit]);
            v >>= 4;
        }
        putchar(b'\n');
    }

    // ... Continue booting secondary cores
}
```

#### Step 2: Use Correct HVC PSCI_CPU_ON

```rust
// Boot CPU 1
for cpu_id in 1..2 {
    let mpidr = cpu_id as u64;  // QEMU virt's CPU MPIDR is the CPU ID

    unsafe {
        // PSCI_CPU_ON HVC call
        // x0 = function ID (0x84000003 = PSCI_CPU_ON for HVC)
        // x1 = target CPU (MPIDR)
        // x2 = entry point (physical address of secondary_entry)
        // x3 = context ID (0)
        let mut result: u64;
        core::arch::asm!(
            "hvc #0",
            inlateout("x0") 0x84000003u64 => result,  // <- HVC function ID!
            in("x1") mpidr,
            in("x2") secondary_entry as u64,
            in("x3") 0u64,
            options(nomem, nostack)
        );

        // Check return value (0 = success)
        if result == 0 {
            const MSG_OK: &[u8] = b"smp: CPU boot PSCI success\n";
            for &b in MSG_OK {
                putchar(b);
            }
        } else {
            // Print error code
            let hex = b"0123456789ABCDEF";
            let mut r = result;
            for _ in 0..16 {
                let digit = (r & 0xF) as usize;
                putchar(hex[digit]);
                r >>= 4;
            }
        }
    }
}
```

#### Step 3: Clean up boot.S

Remove unused EL2 PSCI code (QEMU boots at EL1, this code never executes):

```assembly
/* kernel/src/arch/aarch64/boot/boot.S */

el2_entry:
    /* ========== Drop from EL2 to EL1 ==========*/
    /* Set temporary stack */
    adr     x0, boot_stack_top
    mov     sp, x0

    mov     x0, #(1 << 31)      /* EL1h, AArch64 */
    msr     spsr_el2, x0
    adr     x0, el1_entry
    msr     elr_el2, x0

    /* Don't enable MMU, directly return to EL1 */
    eret
```

---

## 5. Verification Results

### 5.1 Build and Run

```bash
make build
timeout 3 qemu-system-aarch64 -M virt -cpu cortex-a57 -m 2G -smp 2 \
  -nographic -serial mon:stdio \
  -kernel target/aarch64-unknown-none/debug/rux
```

### 5.2 Output Example

```
Rux Kernel v0.1.0 starting...
Target platform: aarch64
...
Initializing VFS...
vfs: VFS layer initialized [OK]
Initializing SMP...
Attempting PSCI CPU_ON...
smp: Booting secondary CPUs...
smp: Checking PSCI version...          <- PSCI version query
smp: PSCI version = 0x10001000        <- PSCI 1.1
smp: Calling PSCI for CPU 1
smp: PSCI result = 0000000000000000   <- Returns 0 (success)
smp: CPU boot PSCI success            <- CPU boot success
[CPU1 up]                              <- CPU 1 online!
Waiting for secondary CPUs...
SMP: 2 CPUs online                     <- Dual-core confirmed!
```

### 5.3 CPU 1 Initialization Output

```
[CPU1] init: runqueue                  <- CPU 1 initializing runqueue
sched: CPU 1 runqueue [OK]
[CPU1] init: IRQ enabled               <- CPU 1 enabling IRQ
[CPU1] idle: waiting for work          <- CPU 1 entering idle loop
```

---

## 6. Technical Summary

### 6.1 Key Points

1. **PSCI Function IDs Vary by Calling Method**
   - HVC call: `0x8400000N` (N = function number)
   - SMC call: `0xC400000N` (N = function number)

2. **Must Follow Device Tree's `method` Property**
   - QEMU virt device tree specifies `method = "hvc"`
   - Using incorrect calling method causes failure or hang

3. **QEMU virt Boots at EL1**
   - Not EL2 or EL3
   - HVC calls are handled internally by QEMU

4. **PSCI Version Query is Important**
   - Verifies PSCI availability
   - Confirms supported features

### 6.2 Common Pitfalls

| Pitfall | Symptom | Solution |
|---------|---------|----------|
| Using SMC call | QEMU hangs | Use HVC instead |
| Using SMC function ID | Returns NOT_SUPPORTED | Use 0x84... prefix |
| Calling at wrong EL | Call fails | Check CurrentEL |
| Forgetting version check | Cannot debug | Call PSCI_VERSION first |

### 6.3 Debugging Tips

1. **Add PSCI Version Check**
   ```rust
   let psci_version: u64;
   unsafe {
       core::arch::asm!(
           "hvc #0",
           inlateout("x0") 0x84000000u64 => psci_version,
           options(nomem, nostack)
       );
   }
   println!("PSCI version = 0x{:x}", psci_version);
   ```

2. **Add Debug Output to `secondary_entry`**
   ```assembly
   secondary_entry:
       mrs     x1, mpidr_el1
       and     x1, x1, #0xFF
       /* Output CPU ID */
       mov     x0, #0x09000000    /* UART base address */
       mov     w2, #0x31          /* '1' */
       str     w2, [x0]
       /* ... continue boot */
   ```

3. **Check Device Tree**
   ```bash
   dtc -I dtb -O dts virt.dts | grep -A 5 psci
   ```

---

## 7. References

### 7.1 ARM Official Documentation

- [PSCI Specification](https://developer.arm.com/documentation/den0022/latest/)
- [ARMv8-A Architecture Reference Manual](https://developer.arm.com/documentation/ddi0487/latest)
- [SMC Calling Convention](https://developer.arm.com/documentation/den0028/latest)

### 7.2 Linux Kernel References

- `arch/arm64/kernel/psci.c` - PSCI driver implementation
- `arch/arm64/kernel/smp.c` - SMP boot code
- `drivers/firmware/psci/psci.c` - PSCI client

### 7.3 QEMU Documentation

- [QEMU ARM virt Platform](https://qemu.readthedocs.io/en/latest/system/arm/virt.html)
- [QEMU and PSCI](https://qemu.readthedocs.io/en/latest/system/arm/virt.html)

### 7.4 Related Code

- [kernel/src/arch/aarch64/smp.rs](../kernel/src/arch/aarch64/smp.rs) - PSCI call implementation
- [kernel/src/arch/aarch64/boot/boot.S](../kernel/src/arch/aarch64/boot/boot.S) - Boot code
- [docs/TODO.md](TODO.md) - Project TODO list

---

## 8. Appendix: Complete Code Examples

### 8.1 PSCI Version Query

```rust
/// Query PSCI version
pub fn psci_version() -> u64 {
    let version: u64;
    unsafe {
        core::arch::asm!(
            "hvc #0",
            inlateout("x0") 0x84000000u64 => version,
            options(nomem, nostack)
        );
    }
    version
}

// Version number decoding
fn decode_psci_version(version: u64) -> (u16, u16) {
    let major = (version >> 16) as u16;
    let minor = (version & 0xFFFF) as u16;
    (major, minor)
}

// Example: PSCI 1.1 returns 0x10001000
// major = 1, minor = 1
```

### 8.2 CPU ON Call

```rust
/// Boot specified CPU
///
/// # Parameters
/// - `cpu_id`: CPU ID (0-3)
/// - `entry_point`: Boot entry point physical address
///
/// # Returns
/// - `Ok(())`: Success
/// - `Err(code)`: PSCI error code
pub fn psci_cpu_on(cpu_id: u64, entry_point: u64) -> Result<(), u64> {
    let result: u64;
    unsafe {
        core::arch::asm!(
            "hvc #0",
            inlateout("x0") 0x84000003u64 => result,
            in("x1") cpu_id,
            in("x2") entry_point,
            in("x3") 0u64,
            options(nomem, nostack)
        );
    }

    if result == 0 {
        Ok(())
    } else {
        Err(result)
    }
}
```

### 8.3 Error Code Definitions

```rust
/// PSCI error codes
#[repr(u64)]
pub enum PsciError {
    Success = 0,
    NotSupported = -1i64 as u64,
    InvalidParameters = -2i64 as u64,
    Denied = -3i64 as u64,
    AlreadyOn = -4i64 as u64,
    OnPending = -5i64 as u64,
    InternalFailure = -6i64 as u64,
    NotPresent = -7i64 as u64,
    Disabled = -8i64 as u64,
}
```

---

**Document Version**: v1.0
**Last Updated**: 2025-02-04
**Author**: Rux Kernel Development Team
**Status**: Production Ready
