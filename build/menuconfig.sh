#!/bin/bash
# Rux 内核配置工具
# 类似 Linux kernel menuconfig 的交互式配置界面

# 获取项目根目录（脚本所在目录的父目录）
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
CONFIG_FILE="$PROJECT_ROOT/Kernel.toml"

# 读取配置值的辅助函数
get_config() {
    grep -E "^$1\s*=" "$CONFIG_FILE" | head -1 | sed 's/.*=\s*//' | tr -d '"'
}

# 设置配置值的辅助函数
set_config() {
    sed -i "s/^$1\s*=.*/$1 = $2/" "$CONFIG_FILE"
}

# 主菜单
show_main_menu() {
    while true; do
        choice=$(whiptail --title "Rux Kernel Configuration" \
                        --menu "选择配置类别:" 25 80 10 \
                        "1. General" "基本信息（名称、版本）" \
                        "2. Platform" "平台设置（架构选择）" \
                        "3. Memory" "内存配置（堆大小、页大小）" \
                        "4. Features" "功能特性（进程、文件系统、网络等）" \
                        "5. Drivers" "驱动配置（UART、定时器、GIC等）" \
                        "6. Debug" "调试选项（日志级别、跟踪）" \
                        "7. Performance" "性能调优（优化级别、LTO）" \
                        "8. Security" "安全选项（栈保护、边界检查）" \
                        "9. Save and Exit" "保存配置并退出" \
                        "10. Exit Without Saving" "退出不保存" \
                        3>&1 1>&2 2>&3)

        case $choice in
            1) show_general_menu ;;
            2) show_platform_menu ;;
            3) show_memory_menu ;;
            4) show_features_menu ;;
            5) show_drivers_menu ;;
            6) show_debug_menu ;;
            7) show_performance_menu ;;
            8) show_security_menu ;;
            9) save_and_exit ;;
            10) exit 0 ;;
            *) exit 0 ;;
        esac
    done
}

# 基本信息菜单
show_general_menu() {
    local name=$(get_config "name" | tr -d '"')
    local version=$(get_config "version" | tr -d '"')

    if result=$(whiptail --title "General Configuration" \
                        --inputmenu "编辑基本配置:" 15 60 3 \
                        "Kernel Name" "$name" \
                        "Version" "$version" \
                        3>&1 1>&2 2>&3); then

        local new_name=$(echo "$result" | grep "Kernel Name" | cut -d' ' -f3-)
        local new_version=$(echo "$result" | grep "Version" | cut -d' ' -f2-)

        [ -n "$new_name" ] && set_config "name" "\"$new_name\""
        [ -n "$new_version" ] && set_config "version" "\"$new_version\""

        whiptail --msgbox "配置已更新" 8 40
    fi
}

# 平台菜单
show_platform_menu() {
    local current=$(get_config "default_platform" | tr -d '"')

    if choice=$(whiptail --title "Platform Selection" \
                      --radiolist "选择目标平台:" 15 60 3 \
                      "aarch64" "ARM 64-bit" $( [ "$current" = "aarch64" ] && echo ON || echo OFF ) \
                      "x86_64" "x86 64-bit" $( [ "$current" = "x86_64" ] && echo ON || echo OFF ) \
                      "riscv64" "RISC-V 64-bit" $( [ "$current" = "riscv64" ] && echo ON || echo OFF ) \
                      3>&1 1>&2 2>&3); then
        set_config "default_platform" "\"$choice\""
        whiptail --msgbox "平台设置为: $choice" 8 40
    fi
}

# 内存菜单
show_memory_menu() {
    local heap=$(get_config "kernel_heap_size")
    local phys=$(get_config "physical_memory")
    local page=$(get_config "page_size")

    if result=$(whiptail --title "Memory Configuration" \
                        --inputmenu "内存配置 (MB):" 15 60 3 \
                        "Kernel Heap Size" "$heap" \
                        "Physical Memory" "$phys" \
                        "Page Size" "$page" \
                        3>&1 1>&2 2>&3); then

        local new_heap=$(echo "$result" | grep "Kernel Heap Size" | cut -d' ' -f3-)
        local new_phys=$(echo "$result" | grep "Physical Memory" | cut -d' ' -f2-)
        local new_page=$(echo "$result" | grep "Page Size" | cut -d' ' -f2-)

        [ -n "$new_heap" ] && set_config "kernel_heap_size" "$new_heap"
        [ -n "$new_phys" ] && set_config "physical_memory" "$new_phys"
        [ -n "$new_page" ] && set_config "page_size" "$new_page"

        whiptail --msgbox "内存配置已更新" 8 40
    fi
}

# 功能特性菜单
show_features_menu() {
    local items=()
    local features=(
        "enable_process:进程管理"
        "enable_scheduler:调度器"
        "enable_vfs:虚拟文件系统"
        "enable_network:网络协议栈"
        "enable_pipe:管道IPC"
        "enable_signal:信号处理"
    )

    local checklist_args=""
    for feat in "${features[@]}"; do
        local key="${feat%%:*}"
        local desc="${feat##*:}"
        local val=$(get_config "$key")
        local state="OFF"
        [ "$val" = "true" ] && state="ON"
        checklist_args="$checklist_args $key '$desc' $state "
    done

    if result=$(eval whiptail --title \"Feature Selection\" \
                        --checklist \"选择要启用的功能:\" 20 70 10 \
                        $checklist_args \
                        3>&1 1>&2 2>&3); then

        # 先禁用所有
        for feat in "${features[@]}"; do
            local key="${feat%%:*}"
            set_config "$key" "false"
        done

        # 启用选中的
        for key in $result; do
            set_config "$key" "true"
        done

        whiptail --msgbox "功能配置已更新" 8 40
    fi
}

# 驱动菜单
show_drivers_menu() {
    local uart=$(get_config "enable_uart")
    local timer=$(get_config "enable_timer")
    local gic=$(get_config "enable_gic")

    if result=$(whiptail --title "Driver Configuration" \
                        --checklist "选择要启用的驱动:" 15 60 4 \
                        "enable_uart" "UART驱动" $([ "$uart" = "true" ] && echo ON || echo OFF) \
                        "enable_timer" "定时器驱动" $([ "$timer" = "true" ] && echo ON || echo OFF) \
                        "enable_gic" "GIC中断控制器" $([ "$gic" = "true" ] && echo ON || echo OFF) \
                        3>&1 1>&2 2>&3); then

        set_config "enable_uart" "false"
        set_config "enable_timer" "false"
        set_config "enable_gic" "false"

        for key in $result; do
            set_config "$key" "true"
        done

        whiptail --msgbox "驱动配置已更新" 8 40
    fi
}

# 调试菜单
show_debug_menu() {
    local log_level=$(get_config "log_level" | tr -d '"')
    local debug=$(get_config "debug_output")

    if result=$(whiptail --title "Debug Configuration" \
                        --radiolist "选择日志级别:" 15 60 5 \
                        "error" "仅错误" OFF \
                        "warn" "警告" OFF \
                        "info" "信息" $([ "$log_level" = "info" ] && echo ON || echo OFF) \
                        "debug" "调试" OFF \
                        "trace" "跟踪" OFF \
                        3>&1 1>&2 2>&3); then
        set_config "log_level" "\"$result\""
    fi

    if whiptail --title "Debug Output" --yesno "启用调试输出?" 8 40; then
        set_config "debug_output" "true"
    else
        set_config "debug_output" "false"
    fi
}

# 性能菜单
show_performance_menu() {
    local opt=$(get_config "opt_level")
    local lto=$(get_config "lto")

    if result=$(whiptail --title "Performance Configuration" \
                        --menu "性能调优:" 15 60 5 \
                        "1" "优化级别 (当前: $opt)" \
                        "2" "链接时优化 (当前: $lto)" \
                        3>&1 1>&2 2>&3); then

        case $result in
            1)
                if opt_choice=$(whiptail --inputbox "优化级别 (0-3):" 8 40 "$opt" 3>&1 1>&2 2>&3); then
                    set_config "opt_level" "$opt_choice"
                fi
                ;;
            2)
                if whiptail --yesno "启用链接时优化 (LTO)?" 8 40; then
                    set_config "lto" "true"
                else
                    set_config "lto" "false"
                fi
                ;;
        esac
    fi
}

# 安全菜单
show_security_menu() {
    local stack=$(get_config "stack_protector")
    local bounds=$(get_config "bounds_check")
    local overflow=$(get_config "overflow_check")

    if result=$(whiptail --title "Security Configuration" \
                        --checklist "安全选项:" 15 60 4 \
                        "stack_protector" "栈保护" $([ "$stack" = "true" ] && echo ON || echo OFF) \
                        "bounds_check" "边界检查" $([ "$bounds" = "true" ] && echo ON || echo OFF) \
                        "overflow_check" "溢出检查" $([ "$overflow" = "true" ] && echo ON || echo OFF) \
                        3>&1 1>&2 2>&3); then

        set_config "stack_protector" "false"
        set_config "bounds_check" "false"
        set_config "overflow_check" "false"

        for key in $result; do
            set_config "$key" "true"
        done

        whiptail --msgbox "安全配置已更新" 8 40
    fi
}

# 保存并退出
save_and_exit() {
    whiptail --msgbox "配置已保存到 $CONFIG_FILE\n\n运行 'cargo build' 重新编译内核" 10 60
    exit 0
}

# 主程序
if [ ! -f "$CONFIG_FILE" ]; then
    echo "错误: 找不到配置文件 $CONFIG_FILE"
    exit 1
fi

show_main_menu
