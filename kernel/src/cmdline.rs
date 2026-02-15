//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! 内核命令行参数解析模块
//!
//! 对应 Linux 的 cmdline parsing (kernel/params.c)
//!
//! OpenSBI 通过设备树的 /chosen 节点的 bootargs 属性传递启动参数
//! QEMU 可以使用 `-append "root=/dev/vda ..."` 传递参数

use crate::println;
use core::sync::atomic::{AtomicPtr, Ordering};
use alloc::string::String;
use alloc::vec::Vec;
use alloc::vec;

/// 全局命令行参数存储
/// 使用 AtomicPtr 和长度存储，确保多核环境下的内存可见性
static CMDLINE_PTR: AtomicPtr<u8> = AtomicPtr::new(core::ptr::null_mut());
static CMDLINE_LEN: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// 命令行参数最大长度
const MAX_CMDLINE_LEN: usize = 2048;

/// 默认命令行参数
const DEFAULT_CMDLINE: &str = "root=/dev/vda rw console=ttyS0 init=/bin/shell";

/// 设备树头结构
#[repr(C)]
struct FdtHeader {
    magic: u32,           // 0xd00dfeed
    totalsize: u32,
    off_dt_struct: u32,
    off_dt_strings: u32,
    off_mem_rsvmap: u32,
    version: u32,
    last_comp_version: u32,
    boot_cpuid_phys: u32,
    size_dt_strings: u32,
    size_dt_struct: u32,
}

/// 设备树属性结构
#[repr(C)]
struct FdtProp {
    len: u32,
    nameoff: u32,
}

const FDT_BEGIN_NODE: u32 = 0x1;
const FDT_END_NODE: u32 = 0x2;
const FDT_PROP: u32 = 0x3;
const FDT_END: u32 = 0x9;

/// 从设备树解析 bootargs
///
/// # 参数
/// - `dtb_ptr`: 设备树扁平数据指针
///
/// # 返回
/// - `Some(bootargs)`: 找到 bootargs 字符串
/// - `None`: 未找到
unsafe fn parse_bootargs(dtb_ptr: u64) -> Option<String> {
    let fdt = dtb_ptr as *const u8;

    // 辅助函数：读取 u32 (big endian)
    let read_u32 = |offset: usize| -> u32 {
        let b0 = *fdt.offset(offset as isize) as u32;
        let b1 = *fdt.offset(offset as isize + 1) as u32;
        let b2 = *fdt.offset(offset as isize + 2) as u32;
        let b3 = *fdt.offset(offset as isize + 3) as u32;
        (b0 << 24) | (b1 << 16) | (b2 << 8) | b3
    };

    // 读取魔数
    let magic = read_u32(0);
    if magic != 0xd00dfeed {
        println!("cmdline: Invalid FDT magic: {:#x} (expected 0xd00dfeed)", magic);
        return None;
    }

    // 读取头信息
    // FDT 头布局（偏移→含义）：
    // 0x00: magic
    // 0x04: totalsize
    // 0x08: off_dt_struct
    // 0x0C: off_dt_strings
    // 0x10: off_mem_rsvmap
    // 0x14: version
    // 0x18: last_comp_version
    // 0x1C: boot_cpuid_phys
    // 0x20: size_dt_strings
    // 0x24: size_dt_struct
    let _totalsize = read_u32(0x04) as usize;
    let off_dt_struct = read_u32(0x08) as usize;
    let off_dt_strings = read_u32(0x0C) as usize;
    let _off_mem_rsvmap = read_u32(0x10) as usize;
    let version = read_u32(0x14) as usize;
    let _last_comp_version = read_u32(0x18) as usize;
    let _boot_cpuid_phys = read_u32(0x1C) as usize;
    let size_dt_strings = read_u32(0x20) as usize;
    let size_dt_struct = read_u32(0x24) as usize;

    let mut ptr = fdt.offset(off_dt_struct as isize);
    let end = fdt.offset((off_dt_struct + size_dt_struct) as isize);
    let strings = fdt.offset(off_dt_strings as isize);

    // 辅助函数：从指针位置读取 u32（大端）
    let read_u32_at = |p: *const u8| -> u32 {
        let b0 = unsafe { *p as u32 };
        let b1 = unsafe { *p.offset(1) as u32 };
        let b2 = unsafe { *p.offset(2) as u32 };
        let b3 = unsafe { *p.offset(3) as u32 };
        (b0 << 24) | (b1 << 16) | (b2 << 8) | b3
    };

    let mut depth = 0;
    let mut in_chosen = false;

    while ptr < end {
        let token = read_u32_at(ptr);
        ptr = ptr.offset(4);

        match token {
            FDT_BEGIN_NODE => {
                // 读取节点名
                let mut nodename = [0u8; 64];
                let mut i = 0;
                while *ptr != 0 && i < 64 {
                    nodename[i] = *ptr;
                    ptr = ptr.offset(1);
                    i += 1;
                }
                ptr = ptr.offset(1);
                // 对齐到 4 字节
                ptr = ptr.offset(((4 - ((ptr as usize) & 3)) & 3) as isize);

                let name = core::str::from_utf8(&nodename[..i]).ok()?;
                if name == "chosen" || name.starts_with("chosen@") {
                    in_chosen = true;
                }
                depth += 1;
            }
            FDT_END_NODE => {
                if in_chosen && depth == 1 {
                    in_chosen = false;
                }
                depth -= 1;
            }
            FDT_PROP => {
                let len = read_u32_at(ptr) as usize;
                let nameoff = read_u32_at(ptr.offset(4)) as usize;
                ptr = ptr.offset(8);

                // 读取属性名
                let mut name_ptr = strings.offset(nameoff as isize);
                let mut prop_name = [0u8; 32];
                let mut i = 0;
                while *name_ptr != 0 && i < 32 {
                    prop_name[i] = *name_ptr;
                    name_ptr = name_ptr.offset(1);
                    i += 1;
                }
                let name = core::str::from_utf8(&prop_name[..i]).ok().unwrap_or("???");

                if in_chosen && name == "bootargs" {
                    // 读取 bootargs 字符串
                    let mut bootargs = vec![0u8; len];
                    for j in 0..len {
                        bootargs[j] = *ptr.offset(j as isize);
                    }
                    if let Ok(bootargs_str) = core::str::from_utf8(&bootargs) {
                        // 去掉末尾的 null 字符
                        let trimmed = bootargs_str.trim_end_matches('\0');
                        return Some(String::from(trimmed));
                    }
                }

                ptr = ptr.offset(len as isize);
                // 对齐到 4 字节
                ptr = ptr.offset(((4 - ((ptr as usize) & 3)) & 3) as isize);
            }
            FDT_END => {
                break;
            }
            _ => {
                // 未知 token，忽略
                break;
            }
        }
    }

    None
}

/// 初始化命令行参数
///
/// # 参数
/// - `dtb_ptr`: 设备树指针（OpenSBI 通过 a1 传递）
///
/// # 功能
/// 1. 如果 dtb_ptr 不为 0，解析设备树的 /chosen/bootargs
/// 2. 如果 dtb_ptr 为 0，尝试从 QEMU virt 的默认 DTB 地址读取
/// 3. 如果没有设备树或没有 bootargs，使用默认值
/// 4. 将解析结果存储到全局变量
pub fn init(dtb_ptr: u64) {
    // QEMU virt 机器的 DTB 通常在这个地址（OpenSBI 使用）
    const QEMU_DTB_ADDR: u64 = 0xbfe00000;

    // 如果 dtb_ptr 为 0，尝试从已知的 QEMU DTB 地址读取
    let dtb_addr = if dtb_ptr != 0 {
        dtb_ptr
    } else {
        println!("cmdline: DTB pointer is 0, trying QEMU default address {:#x}", QEMU_DTB_ADDR);
        QEMU_DTB_ADDR
    };

    let cmdline: &'static str = unsafe {
        match parse_bootargs(dtb_addr) {
            Some(bootargs) => {
                println!("cmdline: Parsed bootargs from device tree: {}", bootargs);
                // 将 String 转换为 &'static str（通过 Box::leak）
                let boxed = alloc::boxed::Box::new(bootargs);
                alloc::boxed::Box::leak(boxed)
            }
            None => {
                println!("cmdline: No bootargs found in device tree at {:#x}", dtb_addr);
                println!("cmdline: Using default cmdline: {}", DEFAULT_CMDLINE);
                DEFAULT_CMDLINE
            }
        }
    };

    // 存储命令行参数（使用原子操作确保多核可见性）
    let len = cmdline.len();
    let ptr = cmdline.as_ptr() as *mut u8;
    CMDLINE_LEN.store(len, Ordering::Release);
    CMDLINE_PTR.store(ptr, Ordering::Release);

    println!("cmdline: Initialized successfully");
}

/// 获取命令行参数字符串（返回静态引用，避免分配）
pub fn get_cmdline() -> Option<&'static str> {
    let ptr = CMDLINE_PTR.load(Ordering::Acquire);
    let len = CMDLINE_LEN.load(Ordering::Acquire);

    if ptr.is_null() || len == 0 {
        return None;
    }

    unsafe {
        let slice = core::slice::from_raw_parts(ptr, len);
        core::str::from_utf8(slice).ok()
    }
}

/// 解析命令行参数，获取指定键的值
///
/// # 参数
/// - `key`: 要查找的参数名（如 "root", "init"）
///
/// # 返回
/// - `Some(value)`: 找到参数值
/// - `None`: 未找到参数
///
/// # 示例
/// ```
/// let root = cmdline::get_param("root");  // "root=/dev/ram0" -> Some("/dev/ram0")
/// let init = cmdline::get_param("init");  // "init=/hello_world" -> Some("/hello_world")
/// ```
pub fn get_param(key: &str) -> Option<String> {
    let cmdline = get_cmdline()?;

    // 查找 key= 格式的参数
    for token in cmdline.split_whitespace() {
        if let Some(idx) = token.find('=') {
            let token_key = &token[..idx];
            if token_key == key {
                let value = &token[idx + 1..];
                return Some(String::from(value));
            }
        }
    }

    None
}

/// 检查参数是否存在（布尔标志）
///
/// # 参数
/// - `key`: 要检查的参数名（如 "debug", "quiet"）
///
/// # 返回
/// - `true`: 参数存在
/// - `false`: 参数不存在
pub fn has_param(key: &str) -> bool {
    let cmdline = match get_cmdline() {
        Some(c) => c,
        None => return false,
    };

    for token in cmdline.split_whitespace() {
        if token == key {
            return true;
        }
    }

    false
}

/// 获取所有参数的列表
///
/// # 返回
/// - 包含所有 key=value 对的向量
pub fn get_all_params() -> Vec<(String, String)> {
    let mut result = Vec::new();
    let cmdline = match get_cmdline() {
        Some(c) => c,
        None => return result,
    };

    for token in cmdline.split_whitespace() {
        if let Some(idx) = token.find('=') {
            let key = String::from(&token[..idx]);
            let value = String::from(&token[idx + 1..]);
            result.push((key, value));
        }
    }

    result
}

/// 获取根文件系统设备
///
/// 对应 Linux 的 ROOT_DEV (include/linux/root_dev.h)
///
/// # 返回
/// - 根设备名称（如 "/dev/ram0", "/dev/vda"）
pub fn get_root_device() -> String {
    get_param("root").unwrap_or_else(|| {
        String::from("/dev/ram0")
    })
}

/// 获取 init 程序路径
///
/// # 返回
/// - init 程序路径（如 "/hello_world", "/sbin/init"）
pub fn get_init_program() -> String {
    get_param("init").unwrap_or_else(|| String::from("/bin/shell"))
}

/// 检查是否为只读根文件系统
pub fn is_root_readonly() -> bool {
    // 默认为读写，除非指定 ro
    !has_param("ro")
}

/// 检查是否为调试模式
pub fn is_debug_mode() -> bool {
    has_param("debug")
}

/// 获取控制台设备
pub fn get_console_device() -> String {
    get_param("console").unwrap_or_else(|| {
        String::from("ttyS0")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set_test_cmdline(cmdline: &'static str) {
        let ptr = cmdline.as_ptr() as *mut u8;
        let len = cmdline.len();
        CMDLINE_PTR.store(ptr, Ordering::SeqCst);
        CMDLINE_LEN.store(len, Ordering::SeqCst);
    }

    #[test]
    fn test_parse_root() {
        // 测试前需要先初始化
        set_test_cmdline("root=/dev/vda rw console=ttyS0");
        assert_eq!(get_root_device(), "/dev/vda");
        assert!(!is_root_readonly());
    }

    #[test]
    fn test_parse_init() {
        set_test_cmdline("init=/sbin/init root=/dev/ram0");
        assert_eq!(get_init_program(), "/sbin/init");
    }

    #[test]
    fn test_has_param() {
        set_test_cmdline("debug quiet root=/dev/ram0");
        assert!(has_param("debug"));
        assert!(has_param("quiet"));
        assert!(!has_param("ro"));
    }

    #[test]
    fn test_get_all_params() {
        set_test_cmdline("root=/dev/vda init=/hello_world debug");
        let params = get_all_params();
        assert_eq!(params.len(), 2);
        assert_eq!(params[0], (String::from("root"), String::from("/dev/vda")));
        assert_eq!(params[1], (String::from("init"), String::from("/hello_world")));
    }
}
