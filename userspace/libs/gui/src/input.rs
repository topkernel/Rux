//! Input event reading interface
//!
//! Provides user-space input event reading functionality, supporting:
//! - Keyboard events
//! - Mouse/touch events
//!
//! Uses Linux standard interface: open("/dev/input/eventX") + read()

// ============================================================================
// System call numbers
// ============================================================================

mod syscall {
    pub const SYS_OPENAT: usize = 56;
    pub const SYS_READ: usize = 63;
    pub const SYS_CLOSE: usize = 57;

    /// openat flags
    pub const O_RDONLY: u32 = 0;
    pub const O_NONBLOCK: u32 = 0o00004000;

    /// AT_FDCWD
    pub const AT_FDCWD: isize = -100;
}

// ============================================================================
// Event type constants (consistent with kernel input/event.rs)
// ============================================================================

/// Key event
pub const EV_KEY: u16 = 0x01;
/// Relative coordinate event (mouse movement)
pub const EV_REL: u16 = 0x02;
/// Absolute coordinate event (touchscreen, tablet)
pub const EV_ABS: u16 = 0x03;

// ============================================================================
// Key codes
// ============================================================================

/// Mouse left button
pub const BTN_LEFT: u16 = 0x110;
/// Mouse right button
pub const BTN_RIGHT: u16 = 0x111;
/// Mouse middle button
pub const BTN_MIDDLE: u16 = 0x112;

/// Common keyboard keys
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

/// Function keys
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

/// Arrow keys
pub const KEY_UP: u16 = 0x67;
pub const KEY_DOWN: u16 = 0x6c;
pub const KEY_LEFT: u16 = 0x69;
pub const KEY_RIGHT: u16 = 0x6a;

/// Relative axes
pub const REL_X: u16 = 0x00;
pub const REL_Y: u16 = 0x01;
pub const REL_WHEEL: u16 = 0x08;

/// Key values
pub const KEY_RELEASE: i32 = 0;
pub const KEY_PRESS: i32 = 1;
pub const KEY_REPEAT: i32 = 2;

// ============================================================================
// Input event structures
// ============================================================================

/// Input event (24 bytes, consistent with kernel InputEvent)
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct InputEvent {
    /// Timestamp (seconds)
    pub tv_sec: u64,
    /// Timestamp (microseconds)
    pub tv_usec: u64,
    /// Event type
    pub type_: u16,
    /// Event code
    pub code: u16,
    /// Event value
    pub value: i32,
}

impl InputEvent {
    /// Check if this is a key event
    pub fn is_key(&self) -> bool {
        self.type_ == EV_KEY
    }

    /// Check if this is a relative (mouse movement) event
    pub fn is_relative(&self) -> bool {
        self.type_ == EV_REL
    }

    /// Check if this is an absolute coordinate event
    pub fn is_absolute(&self) -> bool {
        self.type_ == EV_ABS
    }

    /// Check if this is a key press
    pub fn is_press(&self) -> bool {
        self.type_ == EV_KEY && self.value == KEY_PRESS
    }

    /// Check if this is a key release
    pub fn is_release(&self) -> bool {
        self.type_ == EV_KEY && self.value == KEY_RELEASE
    }

    /// Check if this is a left mouse button event
    pub fn is_left_button(&self) -> bool {
        self.type_ == EV_KEY && self.code == BTN_LEFT
    }

    /// Check if this is a right mouse button event
    pub fn is_right_button(&self) -> bool {
        self.type_ == EV_KEY && self.code == BTN_RIGHT
    }

    /// Check if this is X-axis movement
    pub fn is_rel_x(&self) -> bool {
        self.type_ == EV_REL && self.code == REL_X
    }

    /// Check if this is Y-axis movement
    pub fn is_rel_y(&self) -> bool {
        self.type_ == EV_REL && self.code == REL_Y
    }
}

// ============================================================================
// System call wrappers
// ============================================================================

/// RISC-V syscall (3 arguments)
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

/// RISC-V syscall (4 arguments)
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

/// Non-RISC-V platforms (for development/testing)
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

/// openat syscall
fn sys_openat(path: &str, flags: u32) -> isize {
    // Use fixed-size buffer to avoid heap allocation
    let mut path_buf: [u8; 64] = [0; 64];
    let bytes = path.as_bytes();
    let len = bytes.len().min(63);
    path_buf[..len].copy_from_slice(&bytes[..len]);

    unsafe {
        syscall4(
            syscall::SYS_OPENAT,
            syscall::AT_FDCWD as usize,
            path_buf.as_ptr() as usize,
            flags as usize,
            0, // mode
        )
    }
}

/// read syscall
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

/// close syscall
fn sys_close(fd: i32) -> isize {
    unsafe {
        syscall3(syscall::SYS_CLOSE, fd as usize, 0, 0) }
}

// ============================================================================
// Input device
// ============================================================================

/// Input device type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputDeviceType {
    /// Keyboard
    Keyboard,
    /// Pointer device (mouse/touchscreen)
    Pointer,
}

/// Input device
pub struct InputDevice {
    /// File descriptor
    fd: i32,
    /// Device type
    device_type: InputDeviceType,
}

impl InputDevice {
    /// Open keyboard device
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

    /// Open pointer device
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

    /// Read input event (non-blocking)
    ///
    /// # Returns
    /// - Some(InputEvent): event available
    /// - None: no event
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

    /// Get device type
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
// Input state tracker
// ============================================================================

/// Input state tracker
///
/// Maintains current input state (mouse position, key states, etc.)
pub struct InputState {
    /// Mouse X position
    pub mouse_x: i32,
    /// Mouse Y position
    pub mouse_y: i32,
    /// Screen width
    screen_width: u32,
    /// Screen height
    screen_height: u32,
    /// Left button pressed
    pub left_button: bool,
    /// Right button pressed
    pub right_button: bool,
    /// Middle button pressed
    pub middle_button: bool,
    /// Modifier key states
    pub shift_pressed: bool,
    pub ctrl_pressed: bool,
    pub alt_pressed: bool,
}

impl InputState {
    /// Create new input state tracker
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

    /// Process input event and update state
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

    /// Get mouse position
    pub fn mouse_position(&self) -> (i32, i32) {
        (self.mouse_x, self.mouse_y)
    }

    /// Check if mouse is within specified rectangle
    pub fn mouse_in_rect(&self, x: i32, y: i32, width: u32, height: u32) -> bool {
        self.mouse_x >= x
            && self.mouse_x < x + width as i32
            && self.mouse_y >= y
            && self.mouse_y < y + height as i32
    }
}
