//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! 用户态程序加载器
//!
//! 简化的用户程序加载和执行

// Conditional import for UserContext based on architecture
#[cfg(feature = "aarch64")]
use crate::arch::aarch64::context::UserContext;
#[cfg(feature = "riscv64")]
use crate::arch::riscv64::context::UserContext;

#[cfg(feature = "aarch64")]
pub const USER_CODE_BASE: u64 = 0x4008_0000;
#[cfg(feature = "riscv64")]
pub const USER_CODE_BASE: u64 = 0x8020_0000;

#[cfg(feature = "aarch64")]
pub const USER_STACK_TOP: u64 = 0x4020_0000;
#[cfg(feature = "riscv64")]
pub const USER_STACK_TOP: u64 = 0x8040_0000;

#[cfg(feature = "aarch64")]
pub static USER_PROGRAM_CODE: &[u8] = &[
    // svc #0 (系统调用，什么都不做，只是为了测试)
    0x00, 0x00, 0x00, 0xD4,  // 0xD4000000 - SVC #0

    // hang: infinite loop (branch to self)
    0x00, 0x00, 0x00, 0x14,  // 0x14000000 - B .
];

#[cfg(feature = "riscv64")]
pub static USER_PROGRAM_CODE: &[u8] = &[
    // ecall (系统调用)
    0x73, 0x00, 0x00, 0x00,  // 0x00000073 - ecall

    // hang: infinite loop (jump to self)
    0x6f, 0x00, 0x00, 0x00,  // 0x0000006f - j .
];

static mut USER_CONTEXT: Option<UserContext> = None;

pub fn exec_user_program() -> ! {
    use crate::console::putchar;

    unsafe {
        const MSG: &[u8] = b"Starting user program...\n";
        for &b in MSG {
            putchar(b);
        }

        // Copy user program code to memory
        let dst = USER_CODE_BASE as *mut u8;
        for i in 0..USER_PROGRAM_CODE.len() {
            *dst.add(i) = USER_PROGRAM_CODE[i];
        }

        // Clean data cache to point of coherency
        core::arch::asm!("dc cvau, {}", in(reg) USER_CODE_BASE, options(nomem, nostack));
        // Data synchronization barrier
        core::arch::asm!("dsb ish", options(nomem, nostack));
        // Invalidate instruction cache
        core::arch::asm!("ic ivau, {}", in(reg) USER_CODE_BASE, options(nomem, nostack));
        // Data synchronization barrier
        core::arch::asm!("dsb ish", options(nomem, nostack));
        // Instruction synchronization barrier
        core::arch::asm!("isb", options(nomem, nostack));

        // Debug: verify first instruction
        let first_instr = *((USER_CODE_BASE) as *const u32);
        let second_instr = *((USER_CODE_BASE + 4) as *const u32);
        const MSG_VERIFY: &[u8] = b"Instructions: ";
        for &b in MSG_VERIFY {
            putchar(b);
        }
        let hex_chars = b"0123456789ABCDEF";
        // Print first instruction
        putchar(hex_chars[((first_instr >> 28) & 0xF) as usize]);
        putchar(hex_chars[((first_instr >> 24) & 0xF) as usize]);
        putchar(hex_chars[((first_instr >> 20) & 0xF) as usize]);
        putchar(hex_chars[((first_instr >> 16) & 0xF) as usize]);
        putchar(hex_chars[((first_instr >> 12) & 0xF) as usize]);
        putchar(hex_chars[((first_instr >> 8) & 0xF) as usize]);
        putchar(hex_chars[((first_instr >> 4) & 0xF) as usize]);
        putchar(hex_chars[(first_instr & 0xF) as usize]);
        putchar(b' ');
        // Print second instruction
        putchar(hex_chars[((second_instr >> 28) & 0xF) as usize]);
        putchar(hex_chars[((second_instr >> 24) & 0xF) as usize]);
        putchar(hex_chars[((second_instr >> 20) & 0xF) as usize]);
        putchar(hex_chars[((second_instr >> 16) & 0xF) as usize]);
        putchar(hex_chars[((second_instr >> 12) & 0xF) as usize]);
        putchar(hex_chars[((second_instr >> 8) & 0xF) as usize]);
        putchar(hex_chars[((second_instr >> 4) & 0xF) as usize]);
        putchar(hex_chars[(second_instr & 0xF) as usize]);
        const MSG_NEWLINE2: &[u8] = b"\n";
        for &b in MSG_NEWLINE2 {
            putchar(b);
        }

        // Debug: print the user program address
        const MSG_USER_BASE: &[u8] = b"User code base = 0x";
        for &b in MSG_USER_BASE {
            putchar(b);
        }
        let hex_chars = b"0123456789ABCDEF";
        let addr = USER_CODE_BASE;
        putchar(hex_chars[((addr >> 28) & 0xF) as usize]);
        putchar(hex_chars[((addr >> 24) & 0xF) as usize]);
        putchar(hex_chars[((addr >> 20) & 0xF) as usize]);
        putchar(hex_chars[((addr >> 16) & 0xF) as usize]);
        putchar(hex_chars[((addr >> 12) & 0xF) as usize]);
        putchar(hex_chars[((addr >> 8) & 0xF) as usize]);
        putchar(hex_chars[((addr >> 4) & 0xF) as usize]);
        putchar(hex_chars[(addr & 0xF) as usize]);
        const MSG_NEWLINE: &[u8] = b"\n";
        for &b in MSG_NEWLINE {
            putchar(b);
        }

        // 初始化静态 UserContext
        // SPSR = 0x0 表示 EL0t（用户模式）
        #[cfg(feature = "aarch64")]
        {
            USER_CONTEXT = Some(UserContext {
                x0: 0,
                x1: 0,
                x2: 0,
                x3: 0,
                x4: 0,
                x5: 0,
                x6: 0,
                x7: 0,
                x8: 0,  // Set initial x8 to 0
                x19: 0,
                x20: 0,
                x21: 0,
                x22: 0,
                x23: 0,
                x24: 0,
                x25: 0,
                x26: 0,
                x27: 0,
                x28: 0,
                x29: 0,
                sp: USER_STACK_TOP,
                elr: USER_CODE_BASE,  // Use the code address as entry point
                spsr: 0x0,  // EL0t with all interrupts enabled
            });
        }

        #[cfg(feature = "riscv64")]
        {
            USER_CONTEXT = Some(UserContext {
                x0: 0,
                x1: 0,
                x2: 0,
                x3: 0,
                x4: 0,
                x5: 0,
                x6: 0,
                x7: 0,
                x8: 0,
                x9: 0,
                x18: 0,
                x19: 0,
                x20: 0,
                x21: 0,
                x22: 0,
                x23: 0,
                x24: 0,
                x25: 0,
                x26: 0,
                x27: 0,
                sp: USER_STACK_TOP,
                pc: USER_CODE_BASE,  // Use the code address as entry point
                status: 0x0,  // User mode with interrupts enabled
            });
        }

        // 调用汇编切换函数
        crate::arch::context::switch_to_user(USER_CONTEXT.as_ref().unwrap());
    }

    // 永远不会到达这里
    #[allow(unreachable_code)]
    loop {
        unsafe {
            core::arch::asm!("wfi", options(nomem, nostack));
        }
    }
}

pub fn test_user_program() {
    use crate::console::putchar;
    const MSG: &[u8] = b"Testing user program execution...\n";
    for &b in MSG {
        putchar(b);
    }

    // 首先测试简化的 EL0 切换
    unsafe {
        test_el0_switch();
    }

    exec_user_program();
}

unsafe fn test_el0_switch() {
    use crate::console::putchar;
    const MSG: &[u8] = b"Testing simplified EL0 switch...\n";
    for &b in MSG {
        putchar(b);
    }

    // 测试：直接从内核调用系统调用处理函数
    const MSG_SYSCALL_TEST: &[u8] = b"Testing direct syscall call from kernel...\n";
    for &b in MSG_SYSCALL_TEST {
        putchar(b);
    }

    // 创建一个简单的 SyscallFrame
    #[cfg(feature = "aarch64")]
    let mut frame = crate::arch::syscall::SyscallFrame {
        x0: 1,     // fd = 1 (stdout)
        x1: 0,     // buf = null (will cause error)
        x2: 10,    // count = 10
        x3: 0,
        x4: 0,
        x5: 0,
        x6: 0,
        x7: 0,
        x8: 0,     // SYS_READ = 0 (not Linux's 63!)
        x9: 0,
        x10: 0,
        x11: 0,
        x12: 0,
        x13: 0,
        x14: 0,
        x15: 0,
        x16: 0,
        x17: 0,
        x18: 0,
        x19: 0,
        x20: 0,
        x21: 0,
        x22: 0,
        x23: 0,
        x24: 0,
        x25: 0,
        x26: 0,
        x27: 0,
        x28: 0,
        x29: 0,
        x30: 0,
        elr: 0,
        esr: 0,
        spsr: 0,
    };

    #[cfg(feature = "riscv64")]
    let mut frame = crate::arch::syscall::SyscallFrame {
        a0: 1,     // fd = 1 (stdout)
        a1: 0,     // buf = null (will cause error)
        a2: 10,    // count = 10
        a3: 0,
        a4: 0,
        a5: 0,
        a6: 0,
        a7: 63,    // SYS_READ = 63 (RISC-V Linux ABI)
        t0: 0,
        t1: 0,
        t2: 0,
        t3: 0,
        t4: 0,
        t5: 0,
        t6: 0,
        s0: 0,
        s1: 0,
        s2: 0,
        s3: 0,
        s4: 0,
        s5: 0,
        s6: 0,
        s7: 0,
        s8: 0,
        s9: 0,
        s10: 0,
        s11: 0,
        ra: 0,
        sp: 0,
        gp: 0,
        tp: 0,
        pc: 0,
        status: 0,
    };

    crate::arch::syscall::syscall_handler(&mut frame);

    const MSG_SYSCALL_RET: &[u8] = b"Syscall returned, checking result...\n";
    for &b in MSG_SYSCALL_RET {
        putchar(b);
    }

    // 检查返回值
    #[cfg(feature = "aarch64")]
    let ret = frame.x0 as i64;
    #[cfg(feature = "riscv64")]
    let ret = frame.a0 as i64;
    if ret < 0 {
        const MSG_ERR_RET: &[u8] = b"Syscall returned error (expected)\n";
        for &b in MSG_ERR_RET {
            putchar(b);
        }
    } else {
        const MSG_OK_RET: &[u8] = b"Syscall succeeded unexpectedly\n";
        for &b in MSG_OK_RET {
            putchar(b);
        }
    }

    // 现在测试 EL0 切换
    const MSG_EL0_SWITCH: &[u8] = b"\nNow testing EL0 switch...\n";
    for &b in MSG_EL0_SWITCH {
        putchar(b);
    }

    // 设置 ELR_EL1 指向一个简单的用户代码
    // 用户代码：SVC #0 然后 B .
    let _user_code: u64 = 0xD400000014000000;  // svc #0; b .

    // 将用户代码写入已知地址
    // 用户代码：无限循环 (B .)
    let code_addr = 0x40081000u64;
    let code_ptr = code_addr as *mut u32;
    code_ptr.write(0x14000000);  // B . (无限循环)

    // 清理指令缓存
    core::arch::asm!(
        "dc cvau, {}",     // clean data cache
        in(reg) code_addr,
        options(nomem, nostack)
    );
    core::arch::asm!("dsb ish", options(nomem, nostack));
    core::arch::asm!(
        "ic ivau, {}",     // invalidate instruction cache
        in(reg) code_addr,
        options(nomem, nostack)
    );
    core::arch::asm!("dsb ish", options(nomem, nostack));
    core::arch::asm!("isb", options(nomem, nostack));

    // 设置用户栈
    let user_stack = 0x40200000u64;

    // 明确设置 SPSR = 0x00000500
    let spsr_value: u64 = 0x500;  // EL0t with D=0, A=0, I=1, F=1

    // 设置系统寄存器并执行 eret
    core::arch::asm!(
        "msr sp_el0, {}",      // 设置用户栈指针
        "msr elr_el1, {}",      // 设置入口点
        "msr spsr_el1, {}",     // 设置 SPSR
        "isb",                  // 指令同步屏障
        "eret",                 // 切换到 EL0
        in(reg) user_stack,
        in(reg) code_addr,
        in(reg) spsr_value,
        options(nomem, nostack)
    );

    // 不应该到达这里
    const MSG_ERET_ERROR: &[u8] = b"ERROR: Returned from eret!\n";
    for &b in MSG_ERET_ERROR {
        putchar(b);
    }

    loop {
        core::arch::asm!("wfi", options(nomem, nostack));
    }
}
