//! 输入事件读取接口
//!
//! 提供用户态输入事件读取功能，支持：
//! - 键盘事件
//! - 鼠标/触摸事件
//!
//! 使用 Linux 标准接口：open("/dev/input/eventX") + read()

// ============================================================================
// 系统调用号
// ============================================================================

mod syscall {
    pub const SYS_OPENAT: usize = 56;
    pub const SYS_READ: usize = 63;
    pub const SYS_CLOSE: usize = 57;

    /// openat 标志
    pub const O_RDONLY: u32 = 0;
    pub const O_NONBLOCK: u32 = 0o00004000;

    /// AT_FDCWD
    pub const AT_FDCWD: isize = -100;
}

// ============================================================================
// 事件类型常量 (与内核 input/event.rs 一致)
// ============================================================================

/// 按键事件
pub const EV_KEY: u16 = 0x01;
/// 相对坐标事件 (鼠标移动)
pub const EV_REL: u16 = 0x02;
/// 绝对坐标事件 (触摸屏、平板)
pub const EV_ABS: u16 = 0x03;

// ============================================================================
// 按键代码
// ============================================================================

/// 鼠标左键
pub const BTN_LEFT: u16 = 0x110;
/// 鼠标右键
pub const BTN_RIGHT: u16 = 0x111;
/// 鼠标中键
pub const BTN_MIDDLE: u16 = 0x112;

/// 常用键盘按键
pub const KEY_ESC: u16 = 0x01;
pub const KEY_1: u16 = 0x02;
pub const KEY_2: u16 = 0x03;
pub const KEY_3: u16 = 0x04;
pub const KEY_4: u16 = 0x05;
pub const KEY_5: u16 = 0x06;
pub const KEY_6: u16 = 0x07;
pub const KEY_7: u16 = 0x08;
pub const KEY_8: u16 = 0x09;
pub const KEY_9: u16 = 0x0a;
pub const KEY_0: u16 = 0x0b;
pub const KEY_BACKSPACE: u16 = 0x0e;
pub const KEY_TAB: u16 = 0x0f;
pub const KEY_Q: u16 = 0x10;
pub const KEY_W: u16 = 0x11;
pub const KEY_E: u16 = 0x12;
pub const KEY_R: u16 = 0x13;
pub const KEY_T: u16 = 0x14;
pub const KEY_Y: u16 = 0x15;
pub const KEY_U: u16 = 0x16;
pub const KEY_I: u16 = 0x17;
pub const KEY_O: u16 = 0x18;
pub const KEY_P: u16 = 0x19;
pub const KEY_ENTER: u16 = 0x1c;
pub const KEY_LEFTCTRL: u16 = 0x1d;
pub const KEY_A: u16 = 0x1e;
pub const KEY_S: u16 = 0x1f;
pub const KEY_D: u16 = 0x20;
pub const KEY_F: u16 = 0x21;
pub const KEY_G: u16 = 0x22;
pub const KEY_H: u16 = 0x23;
pub const KEY_J: u16 = 0x24;
pub const KEY_K: u16 = 0x25;
pub const KEY_L: u16 = 0x26;
pub const KEY_LEFTSHIFT: u16 = 0x2a;
pub const KEY_Z: u16 = 0x2c;
pub const KEY_X: u16 = 0x2d;
pub const KEY_C: u16 = 0x2e;
pub const KEY_V: u16 = 0x2f;
pub const KEY_B: u16 = 0x30;
pub const KEY_N: u16 = 0x31;
pub const KEY_M: u16 = 0x32;
pub const KEY_SPACE: u16 = 0x39;
pub const KEY_RIGHTSHIFT: u16 = 0x36;
pub const KEY_LEFTALT: u16 = 0x38;
pub const KEY_CAPSLOCK: u16 = 0x3a;

/// 功能键
pub const KEY_F1: u16 = 0x3b;
pub const KEY_F2: u16 = 0x3c;
pub const KEY_F3: u16 = 0x3d;
pub const KEY_F4: u16 = 0x3e;
pub const KEY_F5: u16 = 0x3f;
pub const KEY_F6: u16 = 0x40;
pub const KEY_F7: u16 = 0x41;
pub const KEY_F8: u16 = 0x42;
pub const KEY_F9: u16 = 0x43;
pub const KEY_F10: u16 = 0x44;
pub const KEY_F11: u16 = 0x57;
pub const KEY_F12: u16 = 0x58;

/// 方向键
pub const KEY_UP: u16 = 0x67;
pub const KEY_DOWN: u16 = 0x6c;
pub const KEY_LEFT: u16 = 0x69;
pub const KEY_RIGHT: u16 = 0x6a;

/// 相对坐标轴
pub const REL_X: u16 = 0x00;
pub const REL_Y: u16 = 0x01;
pub const REL_WHEEL: u16 = 0x08;

/// 按键值
pub const KEY_RELEASE: i32 = 0;
pub const KEY_PRESS: i32 = 1;
pub const KEY_REPEAT: i32 = 2;

// ============================================================================
// 输入事件结构体
// ============================================================================

/// 输入事件 (24 字节，与内核 InputEvent 一致)
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct InputEvent {
    /// 时间戳 (秒)
    pub tv_sec: u64,
    /// 时间戳 (微秒)
    pub tv_usec: u64,
    /// 事件类型
    pub type_: u16,
    /// 事件代码
    pub code: u16,
    /// 事件值
    pub value: i32,
}

impl InputEvent {
    /// 是否为按键事件
    pub fn is_key(&self) -> bool {
        self.type_ == EV_KEY
    }

    /// 是否为鼠标移动事件
    pub fn is_relative(&self) -> bool {
        self.type_ == EV_REL
    }

    /// 是否为绝对坐标事件
    pub fn is_absolute(&self) -> bool {
        self.type_ == EV_ABS
    }

    /// 是否为按键按下
    pub fn is_press(&self) -> bool {
        self.type_ == EV_KEY && self.value == KEY_PRESS
    }

    /// 是否为按键释放
    pub fn is_release(&self) -> bool {
        self.type_ == EV_KEY && self.value == KEY_RELEASE
    }

    /// 是否为鼠标左键
    pub fn is_left_button(&self) -> bool {
        self.type_ == EV_KEY && self.code == BTN_LEFT
    }

    /// 是否为鼠标右键
    pub fn is_right_button(&self) -> bool {
        self.type_ == EV_KEY && self.code == BTN_RIGHT
    }

    /// 是否为 X 轴移动
    pub fn is_rel_x(&self) -> bool {
        self.type_ == EV_REL && self.code == REL_X
    }

    /// 是否为 Y 轴移动
    pub fn is_rel_y(&self) -> bool {
        self.type_ == EV_REL && self.code == REL_Y
    }
}

// ============================================================================
// 系统调用包装
// ============================================================================

/// RISC-V 系统调用 (3 参数)
#[cfg(target_arch = "riscv64")]
#[inline(always)]
unsafe fn syscall3(num: usize, arg0: usize, arg1: usize, arg2: usize) -> isize {
    let ret: isize;
    core::arch::asm!(
        "ecall",
        inlateout("a0") arg0 => ret,
        in("a1") arg1,
        in("a2") arg2,
        in("a7") num,
        options(nostack)
    );
    ret
}

/// RISC-V 系统调用 (4 参数)
#[cfg(target_arch = "riscv64")]
#[inline(always)]
unsafe fn syscall4(num: usize, arg0: usize, arg1: usize, arg2: usize, arg3: usize) -> isize {
    let ret: isize;
    core::arch::asm!(
        "ecall",
        inlateout("a0") arg0 => ret,
        in("a1") arg1,
        in("a2") arg2,
        in("a3") arg3,
        in("a7") num,
        options(nostack)
    );
    ret
}

/// 非 RISC-V 平台（开发/测试用）
#[cfg(not(target_arch = "riscv64"))]
#[inline(always)]
unsafe fn syscall3(_num: usize, _arg0: usize, _arg1: usize, _arg2: usize) -> isize {
    -1
}

#[cfg(not(target_arch = "riscv64"))]
#[inline(always)]
unsafe fn syscall4(_num: usize, _arg0: usize, _arg1: usize, _arg2: usize, _arg3: usize) -> isize {
    -1
}

/// openat 系统调用
fn sys_openat(path: &str, flags: u32) -> isize {
    let path_bytes = [path.as_bytes(), &[0]].concat();
    unsafe {
        syscall4(
            syscall::SYS_OPENAT,
            syscall::AT_FDCWD as usize,
            path_bytes.as_ptr() as usize,
            flags as usize,
            0, // mode
        )
    }
}

/// read 系统调用
fn sys_read(fd: i32, buf: &mut [u8]) -> isize {
    unsafe {
        syscall3(
            syscall::SYS_READ,
            fd as usize,
            buf.as_mut_ptr() as usize,
            buf.len(),
        )
    }
}

/// close 系统调用
fn sys_close(fd: i32) -> isize {
    unsafe {
        syscall3(syscall::SYS_CLOSE, fd as usize, 0, 0) }
}

// ============================================================================
// 输入设备
// ============================================================================

/// 输入设备类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputDeviceType {
    /// 键盘
    Keyboard,
    /// 指针设备（鼠标/触摸屏）
    Pointer,
}

/// 输入设备
pub struct InputDevice {
    /// 文件描述符
    fd: i32,
    /// 设备类型
    device_type: InputDeviceType,
}

impl InputDevice {
    /// 打开键盘设备
    pub fn keyboard() -> Self {
        let fd = sys_openat(
            "/dev/input/event0",
            syscall::O_RDONLY | syscall::O_NONBLOCK,
        );

        Self {
            fd: fd as i32,
            device_type: InputDeviceType::Keyboard,
        }
    }

    /// 打开指针设备
    pub fn pointer() -> Self {
        let fd = sys_openat(
            "/dev/input/event1",
            syscall::O_RDONLY | syscall::O_NONBLOCK,
        );

        Self {
            fd: fd as i32,
            device_type: InputDeviceType::Pointer,
        }
    }

    /// 读取输入事件（非阻塞）
    ///
    /// # 返回
    /// - Some(InputEvent): 有事件
    /// - None: 无事件
    pub fn read_event(&mut self) -> Option<InputEvent> {
        if self.fd < 0 {
            return None;
        }

        let mut event: InputEvent = InputEvent::default();
        let event_size = core::mem::size_of::<InputEvent>();

        let ret = sys_read(
            self.fd,
            unsafe {
                core::slice::from_raw_parts_mut(
                    &mut event as *mut _ as *mut u8,
                    event_size,
                )
            },
        );

        if ret == event_size as isize {
            Some(event)
        } else {
            None
        }
    }

    /// 获取设备类型
    pub fn device_type(&self) -> InputDeviceType {
        self.device_type
    }
}

impl Drop for InputDevice {
    fn drop(&mut self) {
        if self.fd >= 0 {
            sys_close(self.fd);
        }
    }
}

// ============================================================================
// 输入状态追踪器
// ============================================================================

/// 输入状态追踪器
///
/// 维护当前输入状态（鼠标位置、按键状态等）
pub struct InputState {
    /// 鼠标 X 位置
    pub mouse_x: i32,
    /// 鼠标 Y 位置
    pub mouse_y: i32,
    /// 屏幕宽度
    screen_width: u32,
    /// 屏幕高度
    screen_height: u32,
    /// 左键按下
    pub left_button: bool,
    /// 右键按下
    pub right_button: bool,
    /// 中键按下
    pub middle_button: bool,
    /// 修饰键状态
    pub shift_pressed: bool,
    pub ctrl_pressed: bool,
    pub alt_pressed: bool,
}

impl InputState {
    /// 创建新的输入状态追踪器
    pub fn new(screen_width: u32, screen_height: u32) -> Self {
        Self {
            mouse_x: (screen_width / 2) as i32,
            mouse_y: (screen_height / 2) as i32,
            screen_width,
            screen_height,
            left_button: false,
            right_button: false,
            middle_button: false,
            shift_pressed: false,
            ctrl_pressed: false,
            alt_pressed: false,
        }
    }

    /// 处理输入事件，更新状态
    pub fn process_event(&mut self, event: &InputEvent) {
        match event.type_ {
            EV_KEY => {
                let pressed = event.value == KEY_PRESS;
                match event.code {
                    BTN_LEFT => self.left_button = pressed,
                    BTN_RIGHT => self.right_button = pressed,
                    BTN_MIDDLE => self.middle_button = pressed,
                    KEY_LEFTSHIFT | KEY_RIGHTSHIFT => self.shift_pressed = pressed,
                    KEY_LEFTCTRL | 0x61 => self.ctrl_pressed = pressed, // KEY_RIGHTCTRL
                    KEY_LEFTALT | 0x64 => self.alt_pressed = pressed,   // KEY_RIGHTALT
                    _ => {}
                }
            }
            EV_REL => {
                match event.code {
                    REL_X => {
                        self.mouse_x = (self.mouse_x + event.value)
                            .max(0)
                            .min(self.screen_width as i32 - 1);
                    }
                    REL_Y => {
                        self.mouse_y = (self.mouse_y + event.value)
                            .max(0)
                            .min(self.screen_height as i32 - 1);
                    }
                    _ => {}
                }
            }
            EV_ABS => {
                match event.code {
                    0x00 => { // ABS_X
                        if event.value >= 0 {
                            self.mouse_x = event.value.min(self.screen_width as i32 - 1);
                        }
                    }
                    0x01 => { // ABS_Y
                        if event.value >= 0 {
                            self.mouse_y = event.value.min(self.screen_height as i32 - 1);
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    /// 获取鼠标位置
    pub fn mouse_position(&self) -> (i32, i32) {
        (self.mouse_x, self.mouse_y)
    }

    /// 检查鼠标是否在指定区域内
    pub fn mouse_in_rect(&self, x: i32, y: i32, width: u32, height: u32) -> bool {
        self.mouse_x >= x
            && self.mouse_x < x + width as i32
            && self.mouse_y >= y
            && self.mouse_y < y + height as i32
    }
}
