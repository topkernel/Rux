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
static CMDLINE_STORAGE: AtomicPtr<u8> = AtomicPtr::new(core::ptr::null_mut());
static mut CMDLINE_STRING: Option<String> = None;

/// 命令行参数最大长度
const MAX_CMDLINE_LEN: usize = 2048;

/// 默认命令行参数
const DEFAULT_CMDLINE: &str = "root=/dev/ram0 rw console=ttyS0 init=/shell";

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
        println!("cmdline: Invalid FDT magic: {:#x}", magic);
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
    let off_dt_struct = read_u32(4) as usize;    // 先用旧值测试
    let off_dt_strings = read_u32(12) as usize;  // 偏移 0x0C
    let size_dt_struct = read_u32(40) as usize;  // 先用旧值测试

    let mut ptr = fdt.offset(off_dt_struct as isize);
    let end = fdt.offset((off_dt_struct + size_dt_struct) as isize);
    let strings = fdt.offset(off_dt_strings as isize);

    let mut depth = 0;
    let mut in_chosen = false;
    let mut node_count = 0u32;
    let mut prop_count = 0u32;

    while ptr < end {
        let token = read_u32(ptr as usize);
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
                node_count += 1;
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
                let len = read_u32(ptr as usize) as usize;
                let nameoff = read_u32((ptr as usize) + 4) as usize;
                ptr = ptr.offset(8);

                if in_chosen {
                    // 读取属性名
                    let mut name_ptr = strings.offset(nameoff as isize);
                    let mut prop_name = [0u8; 32];
                    let mut i = 0;
                    while *name_ptr != 0 && i < 32 {
                        prop_name[i] = *name_ptr;
                        name_ptr = name_ptr.offset(1);
                        i += 1;
                    }
                    let name = core::str::from_utf8(&prop_name[..i]).ok()?;

                    if name == "bootargs" {
                        // 读取 bootargs 字符串
                        let mut bootargs = vec![0u8; len];
                        for i in 0..len {
                            bootargs[i] = *ptr.offset(i as isize);
                        }
                        let bootargs_str = core::str::from_utf8(&bootargs).ok()?;
                        return Some(String::from(bootargs_str));
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
/// 2. 如果没有设备树或没有 bootargs，使用默认值
/// 3. 将解析结果存储到全局变量
pub fn init(dtb_ptr: u64) {
    let cmdline = if dtb_ptr != 0 {
        // 尝试从设备树解析 bootargs
        unsafe {
            match parse_bootargs(dtb_ptr) {
                Some(bootargs) => {
                    println!("cmdline: Parsed bootargs from device tree: {}", bootargs);
                    bootargs
                }
                None => {
                    println!("cmdline: No bootargs found in device tree at {:#x}", dtb_ptr);
                    println!("cmdline: Using default cmdline: {}", DEFAULT_CMDLINE);
                    String::from(DEFAULT_CMDLINE)
                }
            }
        }
    } else {
        println!("cmdline: No device tree provided");
        println!("cmdline: Using default cmdline: {}", DEFAULT_CMDLINE);
        String::from(DEFAULT_CMDLINE)
    };

    // 存储命令行参数
    unsafe {
        CMDLINE_STRING = Some(cmdline);
        if let Some(ref s) = CMDLINE_STRING {
            CMDLINE_STORAGE.store(s.as_ptr() as *mut u8, Ordering::Release);
        }
    }

    println!("cmdline: Initialized successfully");
}

/// 获取命令行参数字符串
pub fn get_cmdline() -> Option<String> {
    unsafe {
        CMDLINE_STRING.as_ref().map(|s| s.clone())
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
    get_param("init").unwrap_or_else(|| {
        String::from("/shell")
    })
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

    #[test]
    fn test_parse_root() {
        // 测试前需要先初始化
        unsafe { CMDLINE_STRING = Some(String::from("root=/dev/vda rw console=ttyS0")); }
        assert_eq!(get_root_device(), "/dev/vda");
        assert!(!is_root_readonly());
    }

    #[test]
    fn test_parse_init() {
        unsafe { CMDLINE_STRING = Some(String::from("init=/sbin/init root=/dev/ram0")); }
        assert_eq!(get_init_program(), "/sbin/init");
    }

    #[test]
    fn test_has_param() {
        unsafe { CMDLINE_STRING = Some(String::from("debug quiet root=/dev/ram0")); }
        assert!(has_param("debug"));
        assert!(has_param("quiet"));
        assert!(!has_param("ro"));
    }

    #[test]
    fn test_get_all_params() {
        unsafe { CMDLINE_STRING = Some(String::from("root=/dev/vda init=/hello_world debug")); }
        let params = get_all_params();
        assert_eq!(params.len(), 2);
        assert_eq!(params[0], (String::from("root"), String::from("/dev/vda")));
        assert_eq!(params[1], (String::from("init"), String::from("/hello_world")));
    }
}
