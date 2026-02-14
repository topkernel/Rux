//! 图形子系统
//!
//! 提供基础的图形绘制和字体渲染功能

pub mod font;
pub mod double_buffer;

pub use font::{FontRenderer, Framebuffer};
pub use double_buffer::DoubleBuffer;

/// 重新导出 framebuffer
pub use crate::drivers::gpu::framebuffer::FrameBuffer;
