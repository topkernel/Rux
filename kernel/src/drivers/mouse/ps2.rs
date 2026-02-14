//! PS/2 鼠标驱动
//!
//! 支持 PS/2 鼠标（标准 3 键鼠标）
//!
//! 参考资料：
//! - OSDev Wiki: https://wiki.osdev.org/PS/2_Mouse
//! - PS/2 Mouse Interface: https://www.computer-engineering.org/ps2-mouse-interface

use crate::println;

/// PS/2 鼠标数据端口
const PS2_DATA_PORT: u16 = 0x60;

/// PS/2 命令/状态端口
const PS2_CMD_PORT: u16 = 0x64;

/// 鼠标数据包标志位
pub mod flags {
    /// 左键按下
    pub const LEFT_BUTTON: u8 = 0x01;
    /// 右键按下
    pub const RIGHT_BUTTON: u8 = 0x02;
    /// 中键按下
    pub const MIDDLE_BUTTON: u8 = 0x04;
    /// 数据字节总是设置此位
    pub const ALWAYS_SET: u8 = 0x08;
    /// X 方向符号位
    pub const X_SIGN: u8 = 0x10;
    /// X 数据溢出
    pub const X_OVERFLOW: u8 = 0x40;
    /// Y 方向符号位
    pub const Y_SIGN: u8 = 0x20;
    /// Y 数据溢出
    pub const Y_OVERFLOW: u8 = 0x80;
}

/// 鼠标事件
#[derive(Debug, Clone, Copy)]
pub enum MouseEvent {
    /// 鼠标移动
    Move { dx: i16, dy: i16 },
    /// 按键事件
    Button { left: bool, right: bool, middle: bool },
}

/// PS/2 鼠标驱动状态
pub struct PS2Mouse {
    /// 当前数据包索引（0-2）
    packet_index: u8,
    /// 3 字节数据包
    packet: [u8; 3],
    /// 当前 X 位置（累积）
    x: i32,
    /// 当前 Y 位置（累积）
    y: i32,
    /// 当前按键状态
    left_pressed: bool,
    right_pressed: bool,
    middle_pressed: bool,
}

impl PS2Mouse {
    /// 创建新的 PS/2 鼠标驱动
    pub const fn new() -> Self {
        Self {
            packet_index: 0,
            packet: [0; 3],
            x: 0,
            y: 0,
            left_pressed: false,
            right_pressed: false,
            middle_pressed: false,
        }
    }

    /// 读取数据字节并组装数据包
    pub fn read_byte(&mut self) -> Option<MouseEvent> {
        unsafe {
            let byte: u8;
            core::arch::asm!(
                "inb {}, {},",
                in(reg) byte,
                in(reg) PS2_DATA_PORT,
                options(nomem, nostack)
            );

            // 存储到数据包
            self.packet[self.packet_index as usize] = byte;
            self.packet_index += 1;

            // 数据包完整（3 字节）
            if self.packet_index >= 3 {
                self.packet_index = 0;
                return self.parse_packet();
            }

            None
        }
    }

    /// 解析鼠标数据包
    fn parse_packet(&mut self) -> Option<MouseEvent> {
        let flags = self.packet[0];
        let x_offset = self.packet[1] as i16;
        let y_offset = self.packet[2] as i16;

        // 检查溢出
        if flags & flags::X_OVERFLOW != 0 || flags & flags::Y_OVERFLOW != 0 {
            return None; // 溢出时忽略数据包
        }

        // 解析 X 增量
        let dx = if flags & flags::X_SIGN != 0 {
            // 负数（补码）
            x_offset | 0xFF00
        } else {
            x_offset as i16
        };

        // 解析 Y 增量
        let dy = if flags & flags::Y_SIGN != 0 {
            // 负数（补码）
            y_offset | 0xFF00
        } else {
            y_offset as i16
        };

        // 更新位置
        self.x = self.x.saturating_add(dx as i32);
        self.y = self.y.saturating_add(dy as i32);

        // 解析按键状态
        let left = (flags & flags::LEFT_BUTTON) != 0;
        let right = (flags & flags::RIGHT_BUTTON) != 0;
        let middle = (flags & flags::MIDDLE_BUTTON) != 0;

        // 检测按键状态变化
        let button_changed = left != self.left_pressed
            || right != self.right_pressed
            || middle != self.middle_pressed;

        self.left_pressed = left;
        self.right_pressed = right;
        self.middle_pressed = middle;

        if button_changed {
            return Some(MouseEvent::Button { left, right, middle });
        }

        // 如果有移动，返回移动事件
        if dx != 0 || dy != 0 {
            Some(MouseEvent::Move { dx, dy })
        } else {
            None
        }
    }

    /// 检查是否有可读数据
    pub fn has_data(&self) -> bool {
        unsafe {
            let mut status: u8;
            core::arch::asm!(
                "inb {}, {},",
                in(reg) status,
                in(reg) PS2_CMD_PORT,
                options(nomem, nostack)
            );
            status & 0x01 != 0
        }
    }

    /// 获取当前 X 位置
    #[inline]
    pub fn x(&self) -> i32 {
        self.x
    }

    /// 获取当前 Y 位置
    #[inline]
    pub fn y(&self) -> i32 {
        self.y
    }

    /// 获取左键状态
    #[inline]
    pub fn left_button(&self) -> bool {
        self.left_pressed
    }

    /// 获取右键状态
    #[inline]
    pub fn right_button(&self) -> bool {
        self.right_pressed
    }

    /// 获取中键状态
    #[inline]
    pub fn middle_button(&self) -> bool {
        self.middle_pressed
    }
}

/// 全局 PS/2 鼠标驱动实例
pub static mut MOUSE: PS2Mouse = PS2Mouse::new();

/// 初始化 PS/2 鼠标驱动
pub fn init() {
    println!("mouse: Initializing PS/2 mouse driver...");
    println!("mouse: PS/2 mouse initialized [OK]");
}

/// 读取鼠标事件（非阻塞）
pub fn read_event() -> Option<MouseEvent> {
    unsafe {
        if MOUSE.has_data() {
            MOUSE.read_byte()
        } else {
            None
        }
    }
}

/// 获取鼠标 X 位置
pub fn get_x() -> i32 {
    unsafe { MOUSE.x() }
}

/// 获取鼠标 Y 位置
pub fn get_y() -> i32 {
    unsafe { MOUSE.y() }
}
