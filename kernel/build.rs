//! Rux 内核构建脚本
//!
//! 这个脚本在编译前运行，负责：
//! 1. 解析 Kernel.toml 配置文件
//! 2. 生成配置代码
//! 3. 设置条件编译选项

use std::env;
use std::fs;
use std::path::PathBuf;
use std::collections::HashMap;

/// 解析 build/.config 文件（简单 key=value 格式）
fn parse_dot_config(content: &str) -> toml::Value {
    // 存储各 section 的配置
    let mut sections: HashMap<String, HashMap<String, toml::Value>> = HashMap::new();

    for line in content.lines() {
        let line = line.trim();

        // 跳过注释和空行
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        // 解析 section_key=value 格式
        if let Some(eq_pos) = line.find('=') {
            let key = &line[..eq_pos];
            let value = &line[eq_pos + 1..];
            let value = value.trim();

            // 分割 section_key（使用第一个下划线分割）
            if let Some(underscore_pos) = key.find('_') {
                let section = &key[..underscore_pos];
                let config_key = &key[underscore_pos + 1..];

                // 转换值类型
                let parsed_value = if value == "true" {
                    toml::Value::Boolean(true)
                } else if value == "false" {
                    toml::Value::Boolean(false)
                } else if let Ok(int_val) = value.parse::<i64>() {
                    toml::Value::Integer(int_val)
                } else {
                    toml::Value::String(value.to_string())
                };

                sections.entry(section.to_string())
                    .or_insert_with(HashMap::new)
                    .insert(config_key.to_string(), parsed_value);
            }
        }
    }

    // 构建 TOML Value
    let mut root_map = toml::map::Map::new();

    // general section
    let mut general = toml::map::Map::new();
    general.insert("name".to_string(), toml::Value::String("Rux".to_string()));
    general.insert("version".to_string(), toml::Value::String("0.1.0".to_string()));
    root_map.insert("general".to_string(), toml::Value::Table(general));

    // platform section
    let mut platform = toml::map::Map::new();
    platform.insert("default_platform".to_string(), toml::Value::String("riscv64".to_string()));
    root_map.insert("platform".to_string(), toml::Value::Table(platform));

    // 其他 sections - 转换 HashMap 为 toml::map::Map
    for (section_name, section_data) in sections {
        let mut toml_map = toml::map::Map::new();
        for (k, v) in section_data {
            toml_map.insert(k, v);
        }
        root_map.insert(section_name, toml::Value::Table(toml_map));
    }

    toml::Value::Table(root_map)
}

fn main() {
    println!("cargo:rerun-if-changed=../Kernel.toml");
    println!("cargo:rerun-if-changed=../build/.config");

    // 尝试读取 build/.config（menuconfig 生成的配置）
    let config_content = if let Ok(content) = fs::read_to_string("../build/.config") {
        println!("cargo:warning=Using build/.config configuration");
        content
    } else {
        // 回退到 Kernel.toml
        fs::read_to_string("../Kernel.toml")
            .expect("无法读取 Kernel.toml")
    };

    // 判断配置文件类型：检查是否有 TOML 的 [section] 格式
    let is_toml = config_content.lines().any(|line| {
        let trimmed = line.trim();
        trimmed.starts_with('[') && trimmed.ends_with(']')
    });

    // 解析配置
    let config = if is_toml {
        // 解析 TOML
        toml::from_str(&config_content)
            .expect("配置文件解析失败")
    } else {
        // 解析 .config 格式（menuconfig 生成的）
        parse_dot_config(&config_content)
    };

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

// ============================================================
// SMP 配置
// ============================================================

/// 是否启用SMP多核支持
pub const ENABLE_SMP: bool = {};

/// 最大CPU数量
pub const MAX_CPUS: usize = {};

// ============================================================
// 调度器配置
// ============================================================

/// 是否启用调度器
pub const ENABLE_SCHEDULER: bool = {};

/// 默认时间片 (毫秒)
pub const DEFAULT_TIME_SLICE_MS: u32 = {};

/// 时间片滴答数
pub const TIME_SLICE_TICKS: u32 = {};

// ============================================================
// 内存管理配置
// ============================================================

/// 用户栈大小 (字节)
pub const USER_STACK_SIZE: usize = {};

/// 用户栈顶地址
pub const USER_STACK_TOP: u64 = {};

/// 最大页表数量
pub const MAX_PAGE_TABLES: usize = {};

// ============================================================
// 网络配置
// ============================================================

/// 是否启用网络协议栈
pub const ENABLE_NETWORK: bool = {};

/// 以太网 MTU
pub const ETH_MTU: usize = {};

/// TCP 套接字表大小
pub const TCP_SOCKET_TABLE_SIZE: usize = {};

/// UDP 套接字表大小
pub const UDP_SOCKET_TABLE_SIZE: usize = {};

/// ARP 缓存大小
pub const ARP_CACHE_SIZE: usize = {};

/// 路由表大小
pub const ROUTE_TABLE_SIZE: usize = {};

/// IPv4 默认 TTL
pub const IP_DEFAULT_TTL: u8 = {};

// ============================================================
// 子功能使能
// ============================================================

/// 是否启用 TCP 协议
pub const ENABLE_TCP: bool = {};

/// 是否启用 UDP 协议
pub const ENABLE_UDP: bool = {};

/// 是否启用 ARP 协议
pub const ENABLE_ARP: bool = {};

/// 是否启用 IPv4 协议
pub const ENABLE_IPV4: bool = {};

/// 是否启用以太网
pub const ENABLE_ETHERNET: bool = {};

/// 是否启用信号处理
pub const ENABLE_SIGNAL: bool = {};

/// 是否启用虚拟内存
pub const ENABLE_VM: bool = {};

/// 是否启用 VFS
pub const ENABLE_VFS: bool = {};

/// 是否启用管道
pub const ENABLE_PIPE: bool = {};

// ============================================================
// 挂载配置
// ============================================================

/// ext4 磁盘挂载点
pub const EXT4_MOUNT_POINT: &str = "{}";

/// 是否启用 ext4 自动挂载
pub const AUTO_MOUNT_EXT4: bool = {};
"#,
        kernel_name,
        kernel_version,
        target_platform,
        config.get("memory")
            .and_then(|m| m.get("kernel_heap_size"))
            .and_then(|v| v.as_integer())
            .unwrap_or(16) * 1024 * 1024,
        config.get("memory")
            .and_then(|m| m.get("physical_memory"))
            .and_then(|v| v.as_integer())
            .unwrap_or(2048) * 1024 * 1024,
        config.get("memory")
            .and_then(|m| m.get("page_size"))
            .and_then(|v| v.as_integer())
            .unwrap_or(4096),
        config.get("memory")
            .and_then(|m| m.get("page_size"))
            .and_then(|v| v.as_integer())
            .unwrap_or(4096)
            .trailing_zeros() as usize,
        config.get("drivers")
            .and_then(|d| d.get("enable_uart"))
            .and_then(|v| v.as_bool())
            .unwrap_or(true),
        config.get("drivers")
            .and_then(|d| d.get("enable_timer"))
            .and_then(|v| v.as_bool())
            .unwrap_or(true),
        config.get("drivers")
            .and_then(|d| d.get("enable_gic"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        config.get("drivers")
            .and_then(|d| d.get("enable_virtio_net_probe"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        // SMP 配置
        config.get("smp")
            .and_then(|s| s.get("enable_smp"))
            .and_then(|v| v.as_bool())
            .unwrap_or(true),
        config.get("smp")
            .and_then(|s| s.get("max_cpus"))
            .and_then(|v| v.as_integer())
            .unwrap_or(4) as usize,
        // 调度器配置
        config.get("scheduler")
            .and_then(|s| s.get("enable_scheduler"))
            .and_then(|v| v.as_bool())
            .unwrap_or(true),
        config.get("scheduler")
            .and_then(|s| s.get("default_time_slice_ms"))
            .and_then(|v| v.as_integer())
            .unwrap_or(100) as u32,
        config.get("scheduler")
            .and_then(|s| s.get("time_slice_ticks"))
            .and_then(|v| v.as_integer())
            .unwrap_or(10) as u32,
        // 内存管理配置
        config.get("memory")
            .and_then(|m| m.get("user_stack_size"))
            .and_then(|v| v.as_integer())
            .unwrap_or(8) * 1024 * 1024,
        0x0000_003f_ffff_f000u64,
        config.get("memory")
            .and_then(|m| m.get("max_page_tables"))
            .and_then(|v| v.as_integer())
            .unwrap_or(256) as usize,
        // 网络配置
        config.get("network")
            .and_then(|n| n.get("enable_network"))
            .and_then(|v| v.as_bool())
            .unwrap_or(true),
        config.get("network")
            .and_then(|n| n.get("eth_mtu"))
            .and_then(|v| v.as_integer())
            .unwrap_or(1500) as usize,
        config.get("network")
            .and_then(|n| n.get("tcp_socket_table_size"))
            .and_then(|v| v.as_integer())
            .unwrap_or(64) as usize,
        config.get("network")
            .and_then(|n| n.get("udp_socket_table_size"))
            .and_then(|v| v.as_integer())
            .unwrap_or(64) as usize,
        config.get("network")
            .and_then(|n| n.get("arp_cache_size"))
            .and_then(|v| v.as_integer())
            .unwrap_or(64) as usize,
        config.get("network")
            .and_then(|n| n.get("route_table_size"))
            .and_then(|v| v.as_integer())
            .unwrap_or(64) as usize,
        config.get("network")
            .and_then(|n| n.get("ip_default_ttl"))
            .and_then(|v| v.as_integer())
            .unwrap_or(64) as u8,
        // 子功能使能
        config.get("features")
            .and_then(|f| f.get("enable_tcp"))
            .and_then(|v| v.as_bool())
            .unwrap_or(true),
        config.get("features")
            .and_then(|f| f.get("enable_udp"))
            .and_then(|v| v.as_bool())
            .unwrap_or(true),
        config.get("features")
            .and_then(|f| f.get("enable_arp"))
            .and_then(|v| v.as_bool())
            .unwrap_or(true),
        config.get("features")
            .and_then(|f| f.get("enable_ipv4"))
            .and_then(|v| v.as_bool())
            .unwrap_or(true),
        config.get("features")
            .and_then(|f| f.get("enable_ethernet"))
            .and_then(|v| v.as_bool())
            .unwrap_or(true),
        config.get("features")
            .and_then(|f| f.get("enable_signal"))
            .and_then(|v| v.as_bool())
            .unwrap_or(true),
        config.get("features")
            .and_then(|f| f.get("enable_vm"))
            .and_then(|v| v.as_bool())
            .unwrap_or(true),
        config.get("features")
            .and_then(|f| f.get("enable_vfs"))
            .and_then(|v| v.as_bool())
            .unwrap_or(true),
        config.get("features")
            .and_then(|f| f.get("enable_pipe"))
            .and_then(|v| v.as_bool())
            .unwrap_or(true),
        // 挂载配置
        config.get("mount")
            .and_then(|m| m.get("ext4_mount_point"))
            .and_then(|v| v.as_str())
            .unwrap_or("/"),
        config.get("mount")
            .and_then(|m| m.get("auto_mount_ext4"))
            .and_then(|v| v.as_bool())
            .unwrap_or(true),
    );

    let src_dir = manifest_dir.join("src");
    let config_file = src_dir.join("config.rs");

    // 只有内容变化时才写入，避免每次编译都更新文件时间戳
    let existing_content = fs::read_to_string(&config_file).unwrap_or_default();
    if existing_content != config_header {
        fs::write(&config_file, &config_header)
            .expect("写入配置文件失败");
    }
}
