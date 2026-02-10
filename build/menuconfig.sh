#!/bin/bash
# Rux 内核交互式配置菜单
# 生成 build/.config 文件（类似 Linux 内核）

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
KERNEL_TOML="$PROJECT_ROOT/Kernel.toml"
CONFIG_FILE="$SCRIPT_DIR/.config"
CONFIG_BACKUP="$SCRIPT_DIR/.config.bak"

# 从 Kernel.toml 读取默认值
get_default_value() {
    local section=$1
    local key=$2
    local default=$3

    local result=$(sed -n "/^\[$section\]/,/\[/p" "$KERNEL_TOML" 2>/dev/null | \
        grep "^[[:space:]]*$key[[:space:]]*=" | \
        head -1 | \
        sed 's/.*=[[:space:]]*//' | \
        sed 's/[[:space:]]*#.*$//' | \
        sed 's/"//g' | sed "s/'//g")  # Remove quotes

    if [ -n "$result" ]; then
        echo "$result"
    else
        echo "$default"
    fi
}

# 从 .config 读取当前值（如果存在）
get_config_value() {
    local section=$1
    local key=$2
    local default=$3
    
    # 首先尝试从 .config 读取
    if [ -f "$CONFIG_FILE" ]; then
        local value=$(grep "^${section}_${key}=" "$CONFIG_FILE" 2>/dev/null | cut -d'=' -f2)
        if [ -n "$value" ]; then
            echo "$value"
            return
        fi
    fi
    
    # .config 不存在或没有该值，从 Kernel.toml 读取默认值
    get_default_value "$section" "$key" "$default"
}

# 写入配置到 .config
write_config() {
    local section=$1
    local key=$2
    local value=$3
    echo "${section}_${key}=${value}" >> "$CONFIG_FILE.tmp"
}

# 生成 .config 文件
generate_config() {
    cat > "$CONFIG_FILE.tmp" << 'CONFIGEOF'
# Rux 内核配置文件
# 由 make menuconfig 生成
# 
# 格式: section_key=value

CONFIGEOF

    # 内存配置
    write_config "memory" "kernel_heap_size" "$config_kernel_heap_size"
    write_config "memory" "physical_memory" "$config_physical_memory"
    write_config "memory" "user_stack_size" "$config_user_stack_size"
    write_config "memory" "max_page_tables" "$config_max_page_tables"

    # SMP 配置
    write_config "smp" "enable_smp" "$config_enable_smp"
    write_config "smp" "max_cpus" "$config_max_cpus"

    # 调度器配置
    write_config "scheduler" "enable_scheduler" "$config_enable_scheduler"
    write_config "scheduler" "default_time_slice_ms" "$config_default_time_slice_ms"
    write_config "scheduler" "time_slice_ticks" "$config_time_slice_ticks"

    # 网络配置
    write_config "network" "enable_network" "$config_enable_network"
    write_config "network" "eth_mtu" "$config_eth_mtu"
    write_config "network" "tcp_socket_table_size" "$config_tcp_socket_table_size"
    write_config "network" "udp_socket_table_size" "$config_udp_socket_table_size"
    write_config "network" "arp_cache_size" "$config_arp_cache_size"
    write_config "network" "route_table_size" "$config_route_table_size"
    write_config "network" "ip_default_ttl" "$config_ip_default_ttl"

    # 子功能配置
    write_config "features" "enable_tcp" "$config_enable_tcp"
    write_config "features" "enable_udp" "$config_enable_udp"
    write_config "features" "enable_arp" "$config_enable_arp"
    write_config "features" "enable_ipv4" "$config_enable_ipv4"
    write_config "features" "enable_ethernet" "$config_enable_ethernet"
    write_config "features" "enable_signal" "$config_enable_signal"
    write_config "features" "enable_vm" "$config_enable_vm"
    write_config "features" "enable_vfs" "$config_enable_vfs"
    write_config "features" "enable_pipe" "$config_enable_pipe"

    # 驱动配置
    write_config "drivers" "enable_uart" "$config_enable_uart"
    write_config "drivers" "enable_timer" "$config_enable_timer"
    write_config "drivers" "enable_virtio_net_probe" "$config_enable_virtio_net_probe"

    # 启动配置
    write_config "boot" "early_debug" "$config_early_debug"

    # 调试配置
    write_config "debug" "debug_output" "$config_debug_output"
    write_config "debug" "log_level" "$config_log_level"

    # 性能配置
    write_config "performance" "opt_level" "$config_opt_level"

    mv "$CONFIG_FILE.tmp" "$CONFIG_FILE"
}

# 菜单函数
memory_menu() {
    local heap=$(get_config_value "memory" "kernel_heap_size" "16")
    local phys=$(get_config_value "memory" "physical_memory" "2048")
    local ustack=$(get_config_value "memory" "user_stack_size" "8")
    local pagetables=$(get_config_value "memory" "max_page_tables" "256")

    choice=$(whiptail --title "内存配置" --menu "选择要修改的配置:" 16 60 6 \
        "1" "内核堆: ${heap} MB" \
        "2" "物理内存: ${phys} MB" \
        "3" "用户栈: ${ustack} MB" \
        "4" "最大页表: ${pagetables}" \
        3>&1 1>&2 2>&3)

    case $choice in
        1) config_kernel_heap_size=$(whiptail --inputbox "内核堆大小 (MB):" 8 40 "$heap" 3>&1 1>&2 2>&3) ;;
        2) config_physical_memory=$(whiptail --inputbox "物理内存 (MB):" 8 40 "$phys" 3>&1 1>&2 2>&3) ;;
        3) config_user_stack_size=$(whiptail --inputbox "用户栈大小 (MB):" 8 40 "$ustack" 3>&1 1>&2 2>&3) ;;
        4) config_max_page_tables=$(whiptail --inputbox "最大页表数:" 8 40 "$pagetables" 3>&1 1>&2 2>&3) ;;
    esac
}

smp_menu() {
    local enable=$(get_config_value "smp" "enable_smp" "true")
    local cpus=$(get_config_value "smp" "max_cpus" "4")

    if whiptail --yesno "启用 SMP 多核? ($enable)" 8 40; then
        config_enable_smp="true"
    else
        config_enable_smp="false"
    fi

    config_max_cpus=$(whiptail --inputbox "最大 CPU 数:" 8 40 "$cpus" 3>&1 1>&2 2>&3)
}

scheduler_menu() {
    local enable=$(get_config_value "scheduler" "enable_scheduler" "true")
    local slice=$(get_config_value "scheduler" "default_time_slice_ms" "100")
    local ticks=$(get_config_value "scheduler" "time_slice_ticks" "10")

    if whiptail --yesno "启用调度器? ($enable)" 8 40; then
        config_enable_scheduler="true"
    else
        config_enable_scheduler="false"
    fi

    config_default_time_slice_ms=$(whiptail --inputbox "时间片 (毫秒):" 8 40 "$slice" 3>&1 1>&2 2>&3)
    config_time_slice_ticks=$(whiptail --inputbox "时间片滴答数:" 8 40 "$ticks" 3>&1 1>&2 2>&3)
}

network_menu() {
    local enable=$(get_config_value "network" "enable_network" "true")
    local mtu=$(get_config_value "network" "eth_mtu" "1500")
    local tcp=$(get_config_value "network" "tcp_socket_table_size" "64")
    local udp=$(get_config_value "network" "udp_socket_table_size" "64")
    local arp=$(get_config_value "network" "arp_cache_size" "64")
    local route=$(get_config_value "network" "route_table_size" "64")
    local ttl=$(get_config_value "network" "ip_default_ttl" "64")

    if whiptail --yesno "启用网络协议栈? ($enable)" 8 40; then
        config_enable_network="true"
    else
        config_enable_network="false"
    fi

    choice=$(whiptail --title "网络配置" --menu "选择配置:" 18 60 8 \
        "1" "MTU: ${mtu}" \
        "2" "TCP 表: ${tcp}" \
        "3" "UDP 表: ${udp}" \
        "4" "ARP 缓存: ${arp}" \
        "5" "路由表: ${route}" \
        "6" "TTL: ${ttl}" \
        3>&1 1>&2 2>&3)

    case $choice in
        1) config_eth_mtu=$(whiptail --inputbox "以太网 MTU:" 8 40 "$mtu" 3>&1 1>&2 2>&3) ;;
        2) config_tcp_socket_table_size=$(whiptail --inputbox "TCP 套接字表:" 8 40 "$tcp" 3>&1 1>&2 2>&3) ;;
        3) config_udp_socket_table_size=$(whiptail --inputbox "UDP 套接字表:" 8 40 "$udp" 3>&1 1>&2 2>&3) ;;
        4) config_arp_cache_size=$(whiptail --inputbox "ARP 缓存:" 8 40 "$arp" 3>&1 1>&2 2>&3) ;;
        5) config_route_table_size=$(whiptail --inputbox "路由表:" 8 40 "$route" 3>&1 1>&2 2>&3) ;;
        6) config_ip_default_ttl=$(whiptail --inputbox "TTL:" 8 40 "$ttl" 3>&1 1>&2 2>&3) ;;
    esac
}

features_menu() {
    local tcp=$(get_config_value "features" "enable_tcp" "true")
    local udp=$(get_config_value "features" "enable_udp" "true")
    local arp=$(get_config_value "features" "enable_arp" "true")
    local ipv4=$(get_config_value "features" "enable_ipv4" "true")
    local eth=$(get_config_value "features" "enable_ethernet" "true")
    local signal=$(get_config_value "features" "enable_signal" "true")
    local vm=$(get_config_value "features" "enable_vm" "true")
    local vfs=$(get_config_value "features" "enable_vfs" "true")
    local pipe=$(get_config_value "features" "enable_pipe" "true")

    choice=$(whiptail --title "子功能" --checklist "选择要启用的功能:" 20 60 11 \
        "1" "TCP 协议" $([ "$tcp" = "true" ] && echo "ON" || echo "OFF") \
        "2" "UDP 协议" $([ "$udp" = "true" ] && echo "ON" || echo "OFF") \
        "3" "ARP 协议" $([ "$arp" = "true" ] && echo "ON" || echo "OFF") \
        "4" "IPv4 协议" $([ "$ipv4" = "true" ] && echo "ON" || echo "OFF") \
        "5" "以太网" $([ "$eth" = "true" ] && echo "ON" || echo "OFF") \
        "6" "信号处理" $([ "$signal" = "true" ] && echo "ON" || echo "OFF") \
        "7" "虚拟内存" $([ "$vm" = "true" ] && echo "ON" || echo "OFF") \
        "8" "VFS" $([ "$vfs" = "true" ] && echo "ON" || echo "OFF") \
        "9" "管道" $([ "$pipe" = "true" ] && echo "ON" || echo "OFF") \
        3>&1 1>&2 2>&3)

    # 默认全部禁用
    config_enable_tcp="false"
    config_enable_udp="false"
    config_enable_arp="false"
    config_enable_ipv4="false"
    config_enable_ethernet="false"
    config_enable_signal="false"
    config_enable_vm="false"
    config_enable_vfs="false"
    config_enable_pipe="false"

    for num in $choice; do
        case $num in
            1) config_enable_tcp="true" ;;
            2) config_enable_udp="true" ;;
            3) config_enable_arp="true" ;;
            4) config_enable_ipv4="true" ;;
            5) config_enable_ethernet="true" ;;
            6) config_enable_signal="true" ;;
            7) config_enable_vm="true" ;;
            8) config_enable_vfs="true" ;;
            9) config_enable_pipe="true" ;;
        esac
    done
}

drivers_menu() {
    local uart=$(get_config_value "drivers" "enable_uart" "true")
    local timer=$(get_config_value "drivers" "enable_timer" "true")
    local virtio_net=$(get_config_value "drivers" "enable_virtio_net_probe" "true")

    choice=$(whiptail --title "驱动配置" --checklist "选择要启用的驱动:" 12 60 5 \
        "1" "UART" $([ "$uart" = "true" ] && echo "ON" || echo "OFF") \
        "2" "Timer" $([ "$timer" = "true" ] && echo "ON" || echo "OFF") \
        "3" "VirtIO Net 探测" $([ "$virtio_net" = "true" ] && echo "ON" || echo "OFF") \
        3>&1 1>&2 2>&3)

    config_enable_uart="false"
    config_enable_timer="false"
    config_enable_virtio_net_probe="false"

    for num in $choice; do
        case $num in
            1) config_enable_uart="true" ;;
            2) config_enable_timer="true" ;;
            3) config_enable_virtio_net_probe="true" ;;
        esac
    done
}

boot_menu() {
    local debug=$(get_config_value "boot" "early_debug" "true")
    if whiptail --yesno "启用早期调试? ($debug)" 8 40; then
        config_early_debug="true"
    else
        config_early_debug="false"
    fi
}

debug_menu() {
    local output=$(get_config_value "debug" "debug_output" "true")
    local log=$(get_config_value "debug" "log_level" "info")

    if whiptail --yesno "启用调试输出? ($output)" 8 40; then
        config_debug_output="true"
    else
        config_debug_output="false"
    fi

    choice=$(whiptail --radiolist "日志级别:" 12 50 5 \
        "1" "error" off \
        "2" "warn" off \
        "3" "info" on \
        "4" "debug" off \
        "5" "trace" off \
        3>&1 1>&2 2>&3)

    case $choice in
        1) config_log_level="error" ;;
        2) config_log_level="warn" ;;
        3) config_log_level="info" ;;
        4) config_log_level="debug" ;;
        5) config_log_level="trace" ;;
    esac
}

performance_menu() {
    local opt=$(get_config_value "performance" "opt_level" "3")
    choice=$(whiptail --radiolist "优化级别:" 12 40 4 \
        "0" "0 - 无优化" off \
        "1" "1 - 基本优化" off \
        "2" "2 - 较大优化" off \
        "3" "3 - 最大优化" on \
        3>&1 1>&2 2>&3)
    config_opt_level="$choice"
}

security_menu() {
    whiptail --title "安全选项" --msgbox "安全配置：\n\n当前使用 Kernel.toml 中的默认配置" 10 50
}

show_config() {
    if [ -f "$CONFIG_FILE" ]; then
        whiptail --title "当前配置" --textbox "$CONFIG_FILE" 30 80
    else
        whiptail --title "当前配置" --msgbox "未配置，使用 Kernel.toml 默认值" 10 50
    fi
}

save_and_exit() {
    if whiptail --yesno "保存配置到 build/.config ?" 8 40; then
        generate_config
        rm -f "$CONFIG_BACKUP"
        echo "配置已保存到 build/.config"
        echo "运行 'make build' 编译内核"
        exit 0
    fi
}

# 主菜单
main_menu() {
    # 初始化配置变量（从现有 .config 或 Kernel.toml 默认值）
    config_kernel_heap_size=$(get_config_value "memory" "kernel_heap_size" "16")
    config_physical_memory=$(get_config_value "memory" "physical_memory" "2048")
    config_user_stack_size=$(get_config_value "memory" "user_stack_size" "8")
    config_max_page_tables=$(get_config_value "memory" "max_page_tables" "256")
    
    config_enable_smp=$(get_config_value "smp" "enable_smp" "true")
    config_max_cpus=$(get_config_value "smp" "max_cpus" "4")
    
    config_enable_scheduler=$(get_config_value "scheduler" "enable_scheduler" "true")
    config_default_time_slice_ms=$(get_config_value "scheduler" "default_time_slice_ms" "100")
    config_time_slice_ticks=$(get_config_value "scheduler" "time_slice_ticks" "10")
    
    config_enable_network=$(get_config_value "network" "enable_network" "true")
    config_eth_mtu=$(get_config_value "network" "eth_mtu" "1500")
    config_tcp_socket_table_size=$(get_config_value "network" "tcp_socket_table_size" "64")
    config_udp_socket_table_size=$(get_config_value "network" "udp_socket_table_size" "64")
    config_arp_cache_size=$(get_config_value "network" "arp_cache_size" "64")
    config_route_table_size=$(get_config_value "network" "route_table_size" "64")
    config_ip_default_ttl=$(get_config_value "network" "ip_default_ttl" "64")
    
    config_enable_tcp=$(get_config_value "features" "enable_tcp" "true")
    config_enable_udp=$(get_config_value "features" "enable_udp" "true")
    config_enable_arp=$(get_config_value "features" "enable_arp" "true")
    config_enable_ipv4=$(get_config_value "features" "enable_ipv4" "true")
    config_enable_ethernet=$(get_config_value "features" "enable_ethernet" "true")
    config_enable_signal=$(get_config_value "features" "enable_signal" "true")
    config_enable_vm=$(get_config_value "features" "enable_vm" "true")
    config_enable_vfs=$(get_config_value "features" "enable_vfs" "true")
    config_enable_pipe=$(get_config_value "features" "enable_pipe" "true")
    
    config_enable_uart=$(get_config_value "drivers" "enable_uart" "true")
    config_enable_timer=$(get_config_value "drivers" "enable_timer" "true")
    config_enable_virtio_net_probe=$(get_config_value "drivers" "enable_virtio_net_probe" "true")
    
    config_early_debug=$(get_config_value "boot" "early_debug" "true")
    config_debug_output=$(get_config_value "debug" "debug_output" "true")
    config_log_level=$(get_config_value "debug" "log_level" "info")
    config_opt_level=$(get_config_value "performance" "opt_level" "3")

    while true; do
        choice=$(whiptail --title "Rux 内核配置" --menu "选择配置类别:" 18 60 12 \
            "1" "内存管理" \
            "2" "SMP 多核" \
            "3" "调度器" \
            "4" "网络" \
            "5" "子功能" \
            "6" "驱动" \
            "7" "启动" \
            "8" "调试" \
            "9" "性能" \
            "10" "安全" \
            "11" "查看配置" \
            "12" "保存退出" \
            3>&1 1>&2 2>&3)

        [ $? != 0 ] && exit 0

        case $choice in
            1) memory_menu ;;
            2) smp_menu ;;
            3) scheduler_menu ;;
            4) network_menu ;;
            5) features_menu ;;
            6) drivers_menu ;;
            7) boot_menu ;;
            8) debug_menu ;;
            9) performance_menu ;;
            10) security_menu ;;
            11) show_config ;;
            12) save_and_exit ;;
        esac
    done
}

main() {
    if ! command -v whiptail &> /dev/null; then
        echo "错误: 未安装 whiptail"
        echo "运行: sudo apt-get install whiptail"
        exit 1
    fi

    [ ! -f "$KERNEL_TOML" ] && echo "错误: 配置文件不存在" && exit 1

    # 备份现有配置
    [ -f "$CONFIG_FILE" ] && cp "$CONFIG_FILE" "$CONFIG_BACKUP"

    whiptail --title "Rux 内核配置" --msgbox \
        "欢迎使用 Rux 内核配置系统\n\n\
        配置将保存到: build/.config\n\
        使用方向键选择选项\n\
        使用 Tab 键切换按钮\n\
        使用 Enter 确认选择\n\
        使用 Esc 取消操作" \
        12 60

    main_menu
}

main "$@"
