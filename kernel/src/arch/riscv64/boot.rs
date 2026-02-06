//! RISC-V 64位内核启动流程

use crate::println;

// 定义简单的 trap handler (使用 global_asm)
core::arch::global_asm!(
    r#"
.text
.align 2
.global simple_trap_entry
simple_trap_entry:
    // 简单的死循环 - 防止异常导致崩溃
1:  wfi
    j 1b
"#
);

/// 获取当前核心 ID (Hart ID)
pub fn get_core_id() -> u64 {
    unsafe {
        let hart_id: u64;
        core::arch::asm!("csrrw {}, mhartid, zero", out(reg) hart_id);
        hart_id
    }
}

/// 外部链接器符号（BSS 段边界）
extern "C" {
    #[link_name = "__bss_start"]
    static BSS_START: u64;
    #[link_name = "__bss_end"]
    static BSS_END: u64;

    fn simple_trap_entry();
}

/// 外部函数声明
extern "C" {
    fn main() -> !;
}

/// 内核入口点（从 boot.S 跳转）
#[no_mangle]
pub extern "C" fn _start() -> ! {
    // 初始化序列
    unsafe {
        core::arch::asm!(
            // 1. 设置栈指针
            "li sp, {stack_base}",
            // 2. 设置 stvec (trap handler) - 简单的死循环 handler
            "la t0, {simple_trap}",
            "csrw stvec, t0",
            simple_trap = sym simple_trap_entry,
            stack_base = const 0x801F_C000u64,
            options(nostack, nomem)
        );
    }

    // 清零 BSS 段
    unsafe {
        let bss_start = &BSS_START as *const u64 as usize;
        let bss_end = &BSS_END as *const u64 as usize;
        let mut bss_ptr = bss_start as *mut u64;
        let bss_end_ptr = bss_end as *mut u64;

        while bss_ptr < bss_end_ptr {
            *bss_ptr = 0;
            bss_ptr = bss_ptr.offset(1);
        }
    }

    // 调用内核主函数
    unsafe {
        main();
    }

    // 主函数不应该返回
    loop {
        unsafe {
            core::arch::asm!("wfi", options(nomem, nostack));
        }
    }
}
