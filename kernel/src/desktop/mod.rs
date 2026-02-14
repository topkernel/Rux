//! 桌面环境
//!
//! 提供简单的图形桌面环境

extern crate alloc;
use crate::println;
use crate::drivers::gpu::framebuffer::{FrameBuffer, color};
use crate::graphics::font::FontRenderer;
use crate::gui;
use crate::input::{InputEvent, poll_event};
use alloc::vec::Vec;

/// 桌面环境
pub struct Desktop {
    /// 是否正在运行
    running: bool,
    /// 应用程序窗口列表
    app_windows: Vec<gui::WindowId>,
}

impl Desktop {
    /// 创建新的桌面环境
    pub fn new() -> Self {
        Self {
            running: true,
            app_windows: Vec::new(),
        }
    }

    /// 初始化桌面
    pub fn init(&mut self) {
        println!("desktop: Initializing desktop environment...");

        // 创建启动器窗口
        let launcher_id = gui::create_window("Launcher", 10, 10, 200, 300);
        self.app_windows.push(launcher_id);

        // 创建时钟窗口
        let clock_id = gui::create_window("Clock", 220, 10, 200, 100);
        self.app_windows.push(clock_id);

        println!("desktop: Desktop environment initialized [OK]");
    }

    /// 运行桌面主循环
    pub fn run(&mut self, fb: &FrameBuffer, font: &FontRenderer) {
        println!("desktop: Starting desktop main loop...");

        while self.running {
            // 处理输入事件
            self.handle_events();

            // 绘制所有窗口
            gui::draw_all_windows(fb, font);

            // 简单延迟
            unsafe {
                core::arch::asm!(
                    "nop",
                    options(nostack)
                );
            }
        }
    }

    /// 处理输入事件
    fn handle_events(&mut self) {
        while let Some(event) = poll_event() {
            match event {
                InputEvent::Keyboard(key_event) => {
                    self.handle_keyboard(key_event);
                }
                InputEvent::MouseMove { dx, dy } => {
                    // 处理鼠标移动（TODO: 更新鼠标光标位置）
                    let _ = (dx, dy);
                }
                InputEvent::MouseButton { left, right, middle } => {
                    // 处理鼠标点击（TODO: 检测窗口点击）
                    let _ = (left, right, middle);
                }
            }
        }
    }

    /// 处理键盘事件
    fn handle_keyboard(&mut self, key_event: crate::drivers::keyboard::ps2::KeyEvent) {
        use crate::drivers::keyboard::ps2::KeyEvent;

        match key_event {
            KeyEvent::Press(scancode) => {
                // 检查 ESC 键退出
                if scancode == 0x01 {
                    self.running = false;
                    println!("desktop: Exiting desktop (ESC pressed)");
                }
            }
            KeyEvent::Release(_) => {
                // 忽略按键释放
            }
        }
    }

    /// 停止桌面
    pub fn stop(&mut self) {
        self.running = false;
    }
}

/// 初始化桌面环境
pub fn init() -> Desktop {
    println!("desktop: Initializing desktop subsystem...");
    let mut desktop = Desktop::new();
    desktop.init();
    desktop
}
