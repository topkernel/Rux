//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! Kernel command line argument parsing module
//!
//!
//! OpenSBI passes boot arguments through the bootargs property of the /chosen node in device tree
//! QEMU can pass arguments using `-append "root=/dev/vda ..."`

use crate::println;
use core::sync::atomic::{AtomicPtr, Ordering};
use alloc::string::String;
use alloc::vec::Vec;
use alloc::vec;

/// Global command line argument storage
/// Uses AtomicPtr and length storage to ensure memory visibility in multi-core environment
static CMDLINE_PTR: AtomicPtr<u8> = AtomicPtr::new(core::ptr::null_mut());
static CMDLINE_LEN: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// Maximum command line argument length
const MAX_CMDLINE_LEN: usize = 2048;

/// Default command line arguments
const DEFAULT_CMDLINE: &str = "root=/dev/vda rw console=ttyS0 init=/bin/shell";

/// Device tree header structure
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

/// Device tree property structure
#[repr(C)]
struct FdtProp {
    len: u32,
    nameoff: u32,
}

const FDT_BEGIN_NODE: u32 = 0x1;
const FDT_END_NODE: u32 = 0x2;
const FDT_PROP: u32 = 0x3;
const FDT_END: u32 = 0x9;

/// Parse bootargs from device tree
///
/// # Arguments
/// - `dtb_ptr`: Device tree flattened data pointer
///
/// # Returns
/// - `Some(bootargs)`: Found bootargs string
/// - `None`: Not found
unsafe fn parse_bootargs(dtb_ptr: u64) -> Option<String> {
    let fdt = dtb_ptr as *const u8;

    // Helper function: read u32 (big endian)
    let read_u32 = |offset: usize| -> u32 {
        let b0 = *fdt.offset(offset as isize) as u32;
        let b1 = *fdt.offset(offset as isize + 1) as u32;
        let b2 = *fdt.offset(offset as isize + 2) as u32;
        let b3 = *fdt.offset(offset as isize + 3) as u32;
        (b0 << 24) | (b1 << 16) | (b2 << 8) | b3
    };

    // Read magic number
    let magic = read_u32(0);
    if magic != 0xd00dfeed {
        return None;
    }

    // Read header info
    // FDT header layout (offset -> meaning):
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

    // Helper function: read u32 from pointer position (big endian)
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
                // Read node name
                let mut nodename = [0u8; 64];
                let mut i = 0;
                while *ptr != 0 && i < 64 {
                    nodename[i] = *ptr;
                    ptr = ptr.offset(1);
                    i += 1;
                }
                ptr = ptr.offset(1);
                // Align to 4 bytes
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

                // Read property name
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
                    // Read bootargs string
                    let mut bootargs = vec![0u8; len];
                    for j in 0..len {
                        bootargs[j] = *ptr.offset(j as isize);
                    }
                    if let Ok(bootargs_str) = core::str::from_utf8(&bootargs) {
                        // Remove trailing null character
                        let trimmed = bootargs_str.trim_end_matches('\0');
                        return Some(String::from(trimmed));
                    }
                }

                ptr = ptr.offset(len as isize);
                // Align to 4 bytes
                ptr = ptr.offset(((4 - ((ptr as usize) & 3)) & 3) as isize);
            }
            FDT_END => {
                break;
            }
            _ => {
                // Unknown token, ignore
                break;
            }
        }
    }

    None
}

/// Initialize command line arguments
///
/// # Arguments
/// - `dtb_ptr`: Device tree pointer (passed by OpenSBI through a1)
///
/// # Features
/// 1. If dtb_ptr is not 0, parse /chosen/bootargs from device tree
/// 2. If dtb_ptr is 0, try reading from QEMU virt's default DTB address
/// 3. If no device tree or no bootargs, use default value
/// 4. Store parsed result to global variable
pub fn init(dtb_ptr: u64) {
    // QEMU virt machine's DTB is usually at this address (used by OpenSBI)
    const QEMU_DTB_ADDR: u64 = 0xbfe00000;

    // If dtb_ptr is 0, try reading from known QEMU DTB address
    let dtb_addr = if dtb_ptr != 0 {
        dtb_ptr
    } else {
        QEMU_DTB_ADDR
    };

    let cmdline: &'static str = unsafe {
        match parse_bootargs(dtb_addr) {
            Some(bootargs) => {
                // Convert String to &'static str (via Box::leak)
                let boxed = alloc::boxed::Box::new(bootargs);
                alloc::boxed::Box::leak(boxed)
            }
            None => {
                DEFAULT_CMDLINE
            }
        }
    };

    // Store command line arguments (use atomic operations to ensure multi-core visibility)
    let len = cmdline.len();
    let ptr = cmdline.as_ptr() as *mut u8;
    CMDLINE_LEN.store(len, Ordering::Release);
    CMDLINE_PTR.store(ptr, Ordering::Release);
}

/// Get command line argument string (returns static reference to avoid allocation)
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

/// Parse command line arguments, get value for specified key
///
/// # Arguments
/// - `key`: Parameter name to find (e.g. "root", "init")
///
/// # Returns
/// - `Some(value)`: Found parameter value
/// - `None`: Parameter not found
///
/// # Examples
/// ```
/// let root = cmdline::get_param("root");  // "root=/dev/ram0" -> Some("/dev/ram0")
/// let init = cmdline::get_param("init");  // "init=/hello_world" -> Some("/hello_world")
/// ```
pub fn get_param(key: &str) -> Option<String> {
    let cmdline = get_cmdline()?;

    // Find parameter in key= format
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

/// Check if parameter exists (boolean flag)
///
/// # Arguments
/// - `key`: Parameter name to check (e.g. "debug", "quiet")
///
/// # Returns
/// - `true`: Parameter exists
/// - `false`: Parameter does not exist
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

/// Get list of all parameters
///
/// # Returns
/// - Vector containing all key=value pairs
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

/// Get root filesystem device
///
///
/// # Returns
/// - Root device name (e.g. "/dev/ram0", "/dev/vda")
pub fn get_root_device() -> String {
    get_param("root").unwrap_or_else(|| {
        String::from("/dev/ram0")
    })
}

/// Get init program path
///
/// # Returns
/// - Init program path (e.g. "/hello_world", "/sbin/init")
pub fn get_init_program() -> String {
    get_param("init").unwrap_or_else(|| String::from("/bin/shell"))
}

/// Check if root filesystem is read-only
pub fn is_root_readonly() -> bool {
    // Default is read-write, unless ro is specified
    !has_param("ro")
}

/// Check if in debug mode
pub fn is_debug_mode() -> bool {
    has_param("debug")
}

/// Get console device
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
        // Need to initialize before test
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
