//! Rux GUI Library
//!
//! User-space graphical interface library providing:
//! - Basic drawing primitives
//! - Font rendering
//! - Double buffering
//! - Window management
//! - UI widgets
//! - Mouse cursor
//! - Input event handling

pub mod framebuffer;
pub mod font;
pub mod double_buffer;
pub mod cursor;
pub mod window;
pub mod widgets;
pub mod input;

/// Debug printing macro using write syscall
#[macro_export]
macro_rules! debug_println {
    ($($arg:tt)*) => {
        {
            use core::fmt::Write;
            struct DebugWriter;
            impl Write for DebugWriter {
                fn write_str(&mut self, s: &str) -> core::fmt::Result {
                    unsafe {
                        let _ = core::arch::asm!(
                            "ecall",
                            in("a7") 64usize,  // SYS_write
                            in("a0") 1usize,   // stdout
                            in("a1") s.as_ptr() as usize,
                            in("a2") s.len(),
                            lateout("a0") _,
                            options(nostack)
                        );
                    }
                    Ok(())
                }
            }
            let _ = writeln!(DebugWriter, $($arg)*);
        }
    };
}

pub use framebuffer::{Framebuffer, FramebufferDevice, color};
pub use font::FontRenderer;
pub use double_buffer::DoubleBuffer;
pub use cursor::MouseCursor;
pub use window::{Window, WindowManager, WindowId, WindowState};
pub use widgets::{Button, Label, TextBox, SimplePanel, WidgetState, WidgetEvent, WidgetId};
pub use input::{InputDevice, InputDeviceType, InputEvent, InputState};
