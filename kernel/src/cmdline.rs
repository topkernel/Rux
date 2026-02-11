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

/// 全局命令行参数存储
static CMDLINE_STORAGE: AtomicPtr<u8> = AtomicPtr::new(core::ptr::null_mut());
static mut CMDLINE_STRING: Option<String> = None;

/// 命令行参数最大长度
const MAX_CMDLINE_LEN: usize = 2048;

/// 默认命令行参数
const DEFAULT_CMDLINE: &str = "root=/dev/ram0 rw console=ttyS0 init=/hello_world";

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
        // TODO: 解析设备树获取 bootargs
        // 简化实现：暂时使用默认值
        println!("cmdline: Device tree at {:#x}, parsing not yet implemented", dtb_ptr);
        println!("cmdline: Using default cmdline: {}", DEFAULT_CMDLINE);
        String::from(DEFAULT_CMDLINE)
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
        String::from("/hello_world")
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
