//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! Kernel command line argument parsing module
//!
//!
//! OpenSBI passes boot arguments through the bootargs property of the /chosen node in device tree
//! QEMU can pass arguments using `-append "root=/dev/vda ..."`
//!
//! This module also provides device tree (FDT) parsing for:
//! - bootargs from /chosen node
//! - memory regions from /memory nodes (for memblock initialization)

use crate::println;
use core::sync::atomic::{AtomicPtr, Ordering};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use alloc::vec;

/// Global command line argument storage
/// Uses AtomicPtr and length storage to ensure memory visibility in multi-core environment
pub(crate) static CMDLINE_PTR: AtomicPtr<u8> = AtomicPtr::new(core::ptr::null_mut());
pub(crate) static CMDLINE_LEN: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// Maximum command line argument length - from config
const MAX_CMDLINE_LEN: usize = crate::config::MAX_CMDLINE_LEN;

/// Default command line arguments (U2: init defaults to /sbin/init for
/// systemd-style userspace; boot loaders that pass init= override it).
const DEFAULT_CMDLINE: &str = "root=/dev/vda rw console=ttyS0 init=/sbin/init";

/// Memory region descriptor (from device tree)
#[derive(Debug, Clone, Copy)]
pub struct MemoryRegion {
    /// Physical start address
    pub base: usize,
    /// Region size in bytes
    pub size: usize,
}

/// Parse bootargs from device tree
///
/// # Arguments
/// - `dtb_ptr`: Device tree flattened data pointer (virtual address)
///
/// # Returns
/// - `Some(bootargs)`: Found bootargs string
/// - `None`: Not found
/// The ONE audited read edge for boot-time FDT byte accesses
/// (INV-FDT-1): boot.S maps the bootloader-provided DTB blob at its
/// physical address for the machine's lifetime; every parse fn anchors
/// the blob with the 0xd00dfeed magic check before walking, and the
/// walkers keep their cursors inside [fdt, fdt+totalsize).
#[inline]
pub fn fdt_u8(p: *const u8) -> u8 {
    // SAFETY: INV-FDT-1 — a 1-byte read inside the mapped DTB blob.
    unsafe { p.read() }
}

/// Cursor advance inside the mapped DTB (INV-FDT-1): a safe wrapper so
/// the walkers never hand-roll pointer arithmetic.
#[inline]
pub fn fdt_add(p: *const u8, n: isize) -> *const u8 {
    // SAFETY: INV-FDT-1 — arithmetic within the mapped DTB blob; the
    // walkers bound their loops by the header-derived `end`.
    unsafe { p.offset(n) }
}

/// Big-endian u32 read at a cursor inside the mapped DTB (INV-FDT-1).
#[inline]
pub fn fdt_u32(p: *const u8) -> u32 {
    // SAFETY: INV-FDT-1 — a 4-byte read inside the mapped DTB blob.
    unsafe { u32::from_be_bytes(core::ptr::read_unaligned(p as *const [u8; 4])) }
}

// ============================================================================
// The FDT walkers — MOVED to the rux-drivers service crate
// (kernel/rux-drivers/src/of_fdt.rs; root-cold lane 2026-11-24)
// ============================================================================

/// Maximum number of memory regions we track (stack-allocated, no heap
/// needed). The row's return-array length — published here because the
/// BootScanHooks signature names it.
pub const MAX_MEMORY_REGIONS: usize = 8;

// The four dispatchers below keep the historical names and signatures,
// so every caller (cmdline::init, lib.rs's memblock/paging phases,
// vext's ISA prime) is unchanged; the bodies now dispatch through the
// BootScanHooks rows the rux-drivers registrar publishes before the
// framework impl boots. All call sites are boot-time cold.

/// Parse the kernel command line from the bootloader DTB (`/chosen`).
fn parse_bootargs(dtb_ptr: u64) -> Option<String> {
    (crate::boot_scan_hooks::boot_scan_hooks().parse_bootargs)(dtb_ptr)
}

/// Parse the /memory nodes' reg pairs into the stack region array
/// (called on the early identity mapping — the row body allocates
/// nothing).
pub fn parse_memory_regions(
    dtb_ptr: u64,
) -> [MemoryRegion; MAX_MEMORY_REGIONS] {
    (crate::boot_scan_hooks::boot_scan_hooks().parse_memory_regions)(dtb_ptr)
}

/// Parse the initrd location from /chosen (linux,initrd-start/end —
/// physical addresses, early mapping).
pub fn parse_initrd_region(dtb_ptr: u64) -> Option<(usize, usize)> {
    (crate::boot_scan_hooks::boot_scan_hooks().parse_initrd_region)(dtb_ptr)
}

/// True when the first /cpus/cpu@* node's "riscv,isa" property lists
/// the V extension (primes the vext capability cache at boot).
pub fn parse_cpu_isa_has_v(dtb_ptr: u64) -> bool {
    (crate::boot_scan_hooks::boot_scan_hooks().parse_cpu_isa_has_v)(dtb_ptr)
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
    let dtb_phys = if dtb_ptr != 0 {
        dtb_ptr
    } else {
        QEMU_DTB_ADDR
    };

    // Convert physical address to kernel virtual address using linear mapping
    let dtb_virt = crate::arch::mm::phys_to_virt(
        crate::arch::mm::PhysAddr::new(dtb_phys)
    ).bits();

    let cmdline: &'static str = {
        match parse_bootargs(dtb_virt) {
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

    install(cmdline);
}

/// Initialize from a bootloader-provided command line (x86_64 multiboot
/// path — no device tree). `cmdline` may be empty, in which case the
/// default command line applies.
#[cfg(feature = "x86_64")]
pub fn init_from(cmdline: &str) {
    let cmdline: &'static str = if cmdline.is_empty() {
        DEFAULT_CMDLINE
    } else {
        // Leak a heap copy so the global has 'static lifetime.
        alloc::boxed::Box::leak(alloc::boxed::Box::new(
            alloc::string::String::from(cmdline),
        ))
    };
    install(cmdline);
}

/// Publish the active command line and apply early printk loglevel
/// parameters (`quiet`, `loglevel=`).
fn install(cmdline: &'static str) {
    // Store command line arguments (use atomic operations to ensure multi-core visibility)
    let len = cmdline.len();
    let ptr = cmdline.as_ptr() as *mut u8;
    CMDLINE_LEN.store(len, Ordering::Release);
    CMDLINE_PTR.store(ptr, Ordering::Release);

    // U2 console loglevel boot parameters (quiet / loglevel=N), applied
    // to printk immediately so early-boot noise matches the operator's
    // wish. `quiet` lowers the console to KERN_WARNING (4) like Linux;
    // an explicit `loglevel=` (1-8, clamped) overrides it.
    if let Some(level_str) = get_param("loglevel") {
        if let Ok(level) = level_str.parse::<u8>() {
            let clamped = level.clamp(1, 8);
            crate::printk::set_console_loglevel(clamped);
        }
    } else if has_param("quiet") {
        crate::printk::set_console_loglevel(crate::printk::loglevel::KERN_WARNING);
    }

    // `quietcon` (sc-series launcher race): divert /dev/console output
    // from the UART into the printk ring. hilogd's console stream is the
    // dominant serial burn in the launcher-race decision window; see
    // fs/tty.rs `console_output` for the rationale.
    if has_param("quietcon") {
        crate::fs::leaf_glue::tty::set_console_sink_quiet(true);
    }

    // `printk.devkmsg=<on|ratelimit|off>` — the console-echo policy for
    // userspace writes to /dev/kmsg (the reference exposes the same
    // choice as its printk_devkmsg knob). The printk RING always records
    // every line, so hilog consumers see the whole log either way; this
    // only decides how much of it reaches the serial console. Default is
    // the reference behavior (`on`).
    if let Some(mode) = get_param("printk.devkmsg") {
        if let Some(m) = crate::printk::kmsg_console_mode_from_str(mode.as_str()) {
            crate::printk::set_kmsg_console_mode(m);
        }
    }
}

/// Get command line argument string (returns static reference to avoid allocation)
pub fn get_cmdline() -> Option<&'static str> {
    let ptr = CMDLINE_PTR.load(Ordering::Acquire);
    let len = CMDLINE_LEN.load(Ordering::Acquire);

    if ptr.is_null() || len == 0 {
        return None;
    }

    // SAFETY (one storage edge): ptr/len were set together by the boot
    // parse (Box::leak of the bootargs string) — machine-lifetime,
    // never freed, len exactly the string's byte length.
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

/// P2: parse a dotted-decimal IPv4 address ("a.b.c.d") into host byte
/// order. Returns None on malformed input (wrong octet count, non-digit,
/// or an octet above 255).
pub fn parse_ipv4_addr(s: &str) -> Option<u32> {
    let mut ip: u32 = 0;
    let mut octets = 0;
    for part in s.split('.') {
        octets += 1;
        if octets > 4 {
            return None;
        }
        if part.is_empty() || part.len() > 3 || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let v: u32 = part.parse().ok()?;
        if v > 255 {
            return None;
        }
        ip = (ip << 8) | v;
    }
    if octets != 4 {
        return None;
    }
    Some(ip)
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
/// - Init program path (e.g. "/sbin/init", "/bin/sh")
///
/// U2: the default is /sbin/init (systemd) per the Ubuntu boot
/// convention; a bare `init=` on the command line still wins.
pub fn get_init_program() -> String {
    get_param("init").unwrap_or_else(|| String::from("/sbin/init"))
}

/// Absolutize a (possibly relative) init/rdinit path.
///
/// OH's cmdline carries `init=init` (a bare name); Linux execs it via
/// kernel_execve against PID 1's cwd "/", i.e. "/init". Normalize the same
/// way so the loader can find it on the rootfs.
pub fn absolutize_init_path(p: &str) -> String {
    let trimmed = p.trim();
    if trimmed.starts_with('/') {
        String::from(trimmed)
    } else if trimmed.is_empty() {
        String::from("/init")
    } else {
        format!("/{}", trimmed)
    }
}

/// `rdinit=` (initramfs init override), absolutized. None when absent.
pub fn get_rdinit_program() -> Option<String> {
    get_param("rdinit").map(|p| absolutize_init_path(&p))
}

/// Get the root filesystem type (U2 boot-parameter parsing).
///
/// `rootfstype=ext4` → "ext4". Default "ext4" (the only root fs Rux
/// mounts today).
pub fn get_root_fs_type() -> String {
    get_param("rootfstype").unwrap_or_else(|| String::from("ext4"))
}

/// Get the root mount options (U2 boot-parameter parsing).
///
/// `rootflags=<opts>` wins; otherwise "rw" unless the bare `ro` token
/// is present ("rw" token → "rw").
pub fn get_root_flags() -> String {
    if let Some(flags) = get_param("rootflags") {
        return flags;
    }
    if is_root_readonly() {
        String::from("ro")
    } else {
        String::from("rw")
    }
}

/// Check if root filesystem is read-only
pub fn is_root_readonly() -> bool {
    // Default is read-write, unless ro is specified
    has_param("ro")
}

/// Check if in debug mode
pub fn is_debug_mode() -> bool {
    has_param("debug")
}

/// Get console device
///
/// Returns the PRIMARY console device name with any `,speed` suffix
/// stripped (`console=ttyS0,115200` → `ttyS0`). Per Linux semantics the
/// LAST `console=` token names the device behind /dev/console; earlier
/// tokens are additional output consoles.
pub fn get_console_device() -> String {
    get_console_devices()
        .last()
        .cloned()
        .unwrap_or_else(|| String::from("ttyS0"))
}

/// All console devices named on the command line, in order.
///
/// Each `console=` token may name one device plus options
/// (`console=ttyS0,115200n8`); multiple `console=` tokens accumulate.
/// Unknown devices are returned as named — the caller decides what it can
/// provide (riscv64 virt has exactly one UART: ttyS0).
pub fn get_console_devices() -> Vec<String> {
    let cmdline = match get_cmdline() {
        Some(c) => c,
        None => return alloc::vec![String::from("ttyS0")],
    };
    let mut devices = Vec::new();
    for token in cmdline.split_whitespace() {
        if let Some(value) = token.strip_prefix("console=") {
            // First comma-separated field is the device name; the rest is
            // options (speed, parity, ...).
            if let Some(dev) = value.split(',').next() {
                if !dev.is_empty() {
                    devices.push(String::from(dev));
                }
            }
        }
    }
    if devices.is_empty() {
        devices.push(String::from("ttyS0"));
    }
    devices
}

