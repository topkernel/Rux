//! Rux GUI 库
//!
//! 用户态图形界面库，提供：
//! - 基础绘图原语
//! - 字体渲染
//! - 双缓冲
//! - 窗口管理
//! - UI 控件
//! - 鼠标光标

pub mod framebuffer;
pub mod font;
pub mod double_buffer;
pub mod cursor;
pub mod window;
pub mod widgets;

pub use framebuffer::{Framebuffer, FramebufferDevice, color};
pub use font::FontRenderer;
pub use double_buffer::DoubleBuffer;
pub use cursor::MouseCursor;
pub use window::{Window, WindowManager, WindowId, WindowState};
pub use widgets::{Button, Label, TextBox, SimplePanel, WidgetState, WidgetEvent, WidgetId};
