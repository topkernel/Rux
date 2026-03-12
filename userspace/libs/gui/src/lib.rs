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

pub use framebuffer::{Framebuffer, FramebufferDevice, color};
pub use font::FontRenderer;
pub use double_buffer::DoubleBuffer;
pub use cursor::MouseCursor;
pub use window::{Window, WindowManager, WindowId, WindowState};
pub use widgets::{Button, Label, TextBox, SimplePanel, WidgetState, WidgetEvent, WidgetId};
pub use input::{InputDevice, InputDeviceType, InputEvent, InputState};
