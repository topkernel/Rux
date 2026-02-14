//! 图形子系统
//!
//! 提供基础的图形绘制和字体渲染功能

pub mod font;

pub use font::{FontRenderer, Framebuffer};

/// 重新导出 framebuffer
pub use crate::drivers::gpu::framebuffer::FrameBuffer;
