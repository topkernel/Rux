//! Rux 内核构建脚本
//!
//! 这个脚本在编译前运行，负责：
//! 1. 解析 Kernel.toml 配置文件
//! 2. 生成配置代码
//! 3. 设置条件编译选项

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=../Kernel.toml");

    // 读取配置（在工作区根目录）
    let config_content = fs::read_to_string("../Kernel.toml")
        .expect("无法读取 Kernel.toml");

    // 解析 TOML
    let config: toml::Value = toml::from_str(&config_content)
        .expect("配置文件解析失败");

    // 打印配置信息
    if let Some(general) = config.get("general") {
        if let Some(name) = general["name"].as_str() {
            println!("cargo:rustc-env=CARGO_KERNEL_NAME={}", name);
        }
        if let Some(version) = general["version"].as_str() {
            println!("cargo:rustc-env=CARGO_KERNEL_VERSION={}", version);
        }
    }

    // 获取目标平台
    let platform = config.get("platform")
        .and_then(|p| p["default_platform"].as_str())
        .unwrap_or("aarch64");

    println!("cargo:rustc-env=RUX_TARGET_PLATFORM={}", platform);

    // 设置 Rust 编译选项
    if let Some(perf) = config.get("performance") {
        // 设置优化级别
        if let Some(opt_level) = perf.get("opt_level").and_then(|v| v.as_integer()) {
            let opt_str = match opt_level {
                0 => "0",
                1 => "1",
                2 => "2",
                3 => "3",
                _ => "2",
            };
            println!("cargo:rustc-env=RUX_OPT_LEVEL={}", opt_str);
        }

        // 设置LTO
        if let Some(lto) = perf.get("lto").and_then(|v| v.as_bool()) {
            println!("cargo:rustc-env=RUX_ENABLE_LTO={}", lto);
        }

        // 设置codegen-units
        if let Some(units) = perf.get("codegen_units").and_then(|v| v.as_integer()) {
            println!("cargo:rustc-env=RUX_CODEGEN_UNITS={}", units);
        }
    }

    // 设置特性标志
    if let Some(platform) = config.get("platform") {
        let enable_aarch64 = platform.get("enable_aarch64")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let enable_x86_64 = platform.get("enable_x86_64")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let enable_riscv64 = platform.get("enable_riscv64")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // 输出启用的平台
        let enabled_platforms = [
            if enable_aarch64 { "aarch64" } else { "" },
            if enable_x86_64 { "x86_64" } else { "" },
            if enable_riscv64 { "riscv64" } else { "" },
        ];
        println!("cargo:rustc-env=RUX_ENABLED_PLATFORMS={}", enabled_platforms.join(","));
    }

    // 设置调试选项
    if let Some(debug) = config.get("debug") {
        if let Some(log_level) = debug.get("log_level").and_then(|v| v.as_str()) {
            println!("cargo:rustc-env=RUX_LOG_LEVEL={}", log_level);
        }

        if debug.get("debug_output").and_then(|v| v.as_bool()).unwrap_or(false) {
            println!("cargo:rustc-env=RUX_DEBUG={}", "true");
        }

        if debug.get("profiling").and_then(|v| v.as_bool()).unwrap_or(false) {
            println!("cargo:rustc-env=RUX_PROFILING={}", "true");
        }
    }

    // 生成配置代码
    generate_config_code(&config);

    println!("cargo:warning=Rux configuration loaded successfully");
}

fn generate_config_code(config: &toml::Value) {
    let _out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());

    // 获取基本信息
    let kernel_name = config.get("general")
        .and_then(|g| g["name"].as_str())
        .unwrap_or("Rux");

    let kernel_version = config.get("general")
        .and_then(|g| g["version"].as_str())
        .unwrap_or("0.1.0");

    let target_platform = config.get("platform")
        .and_then(|p| p["default_platform"].as_str())
        .unwrap_or("aarch64");

    // 生成配置代码
    let config_header = format!(
        r#"//! Rux 内核配置（自动生成）
//!
//! 此文件由 build.rs 根据 Kernel.toml 自动生成，请勿手动修改

// ============================================================
// 基本信息
// ============================================================

/// 内核名称
pub const KERNEL_NAME: &str = "{}";

/// 内核版本
pub const KERNEL_VERSION: &str = "{}";

/// 目标平台
pub const TARGET_PLATFORM: &str = "{}";

// ============================================================
// 内存配置
// ============================================================

/// 内核堆大小（字节）
pub const KERNEL_HEAP_SIZE: usize = {};

/// 物理内存大小（字节）
pub const PHYS_MEMORY_SIZE: usize = {};

/// 页大小
pub const PAGE_SIZE: usize = {};

/// 页大小位移
pub const PAGE_SHIFT: usize = {};

// ============================================================
// 驱动配置
// ============================================================

/// 是否启用UART驱动
pub const ENABLE_UART: bool = {};

/// 是否启用定时器驱动
pub const ENABLE_TIMER: bool = {};

/// 是否启用GIC中断控制器
pub const ENABLE_GIC: bool = {};

/// 是否启用VirtIO网络设备探测
pub const ENABLE_VIRTIO_NET_PROBE: bool = {};
"#,
        kernel_name,
        kernel_version,
        target_platform,
        config.get("memory")
            .and_then(|m| m["kernel_heap_size"].as_integer())
            .unwrap_or(16) * 1024 * 1024,
        config.get("memory")
            .and_then(|m| m["physical_memory"].as_integer())
            .unwrap_or(2048) * 1024 * 1024,
        config.get("memory")
            .and_then(|m| m["page_size"].as_integer())
            .unwrap_or(4096),
        config.get("memory")
            .and_then(|m| m["page_size"].as_integer())
            .unwrap_or(4096)
            .trailing_zeros() as usize,
        config.get("drivers")
            .and_then(|d| d["enable_uart"].as_bool())
            .unwrap_or(true),
        config.get("drivers")
            .and_then(|d| d["enable_timer"].as_bool())
            .unwrap_or(true),
        config.get("drivers")
            .and_then(|d| d["enable_gic"].as_bool())
            .unwrap_or(false),
        config.get("drivers")
            .and_then(|d| d["enable_virtio_net_probe"].as_bool())
            .unwrap_or(false)
    );

    let src_dir = manifest_dir.join("src");
    let config_file = src_dir.join("config.rs");

    fs::write(&config_file, config_header)
        .expect("写入配置文件失败");

    println!("cargo:rerun-if-changed={}", config_file.display());
}
