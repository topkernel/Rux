//! Rux 桌面环境
//!
//! 用户态桌面环境应用

use rux_gui::{
    FramebufferDevice, FontRenderer, DoubleBuffer, MouseCursor,
    WindowManager, SimplePanel, color,
    InputDevice, InputDeviceType, InputState,
};

/// 桌面环境
struct Desktop {
    fb: FramebufferDevice,
    double_buffer: DoubleBuffer,
    font: FontRenderer,
    cursor: MouseCursor,
    wm: WindowManager,
    launcher_panel: SimplePanel,
    clock_panel: SimplePanel,
    keyboard: InputDevice,
    mouse: InputDevice,
    input_state: InputState,
    running: bool,
}

impl Desktop {
    fn new() -> Self {
        // 打开 framebuffer 设备 (使用 ioctl + mmap)
        let fb = FramebufferDevice::open()
            .expect("Failed to open framebuffer device");

        // 获取屏幕尺寸
        let screen_width = fb.width();
        let screen_height = fb.height();

        // 初始化双缓冲
        let mut double_buffer = DoubleBuffer::new();
        double_buffer.init(screen_width, screen_height, screen_width);

        // 初始化字体
        let font = FontRenderer::new_8x8();

        // 初始化光标
        let cursor = MouseCursor::new(screen_width, screen_height);

        // 初始化窗口管理器
        let mut wm = WindowManager::new();
        wm.create_window("Launcher", 10, 10, 200, 300);
        wm.create_window("Clock", 220, 10, 200, 100);

        // 创建启动器面板
        let mut launcher_panel = SimplePanel::new(10, 40, 180, 260);
        launcher_panel.add_label(10, 10, "Applications:");
        launcher_panel.add_button(10, 40, 160, 30, "Calculator");
        launcher_panel.add_button(10, 80, 160, 30, "Terminal");
        launcher_panel.add_button(10, 120, 160, 30, "File Manager");

        // 创建时钟面板
        let mut clock_panel = SimplePanel::new(220, 40, 180, 60);
        clock_panel.add_label(20, 10, "00:00:00");
        clock_panel.add_label(20, 30, "2026-02-15");

        // 初始化输入设备
        let keyboard = InputDevice::keyboard();
        let mouse = InputDevice::pointer();
        let input_state = InputState::new(screen_width, screen_height);

        Self {
            fb,
            double_buffer,
            font,
            cursor,
            wm,
            launcher_panel,
            clock_panel,
            keyboard,
            mouse,
            input_state,
            running: true,
        }
    }

    fn handle_events(&mut self) {
        // 处理键盘事件
        while let Some(event) = self.keyboard.read_event() {
            self.input_state.process_event(&event);

            // 处理键盘快捷键
            if event.is_key() && event.is_press() {
                match event.code {
                    rux_gui::input::KEY_ESC => {
                        // ESC 退出桌面
                        self.running = false;
                    }
                    _ => {}
                }
            }
        }

        // 处理鼠标事件
        while let Some(event) = self.mouse.read_event() {
            self.input_state.process_event(&event);

            // 更新光标位置
            let (x, y) = self.input_state.mouse_position();
            self.cursor.set_position(x, y);

            // 处理鼠标点击
            if event.is_left_button() && event.is_press() {
                self.handle_click(x, y);
            }
        }
    }

    fn handle_click(&mut self, _x: i32, _y: i32) {
        // TODO: 处理点击事件
        // 检查是否点击了面板上的按钮
        // 如果点击了按钮，执行相应操作
    }

    fn run(&mut self) {
        while self.running {
            // 处理输入事件
            self.handle_events();

            // 绘制
            self.draw();

            // 刷新屏幕
            self.double_buffer.swap_buffers(&self.fb);

            // 延迟 (~60 FPS)
            std::thread::sleep(std::time::Duration::from_millis(16));
        }
    }

    fn draw(&self) {
        // 清空背景
        self.double_buffer.clear(color::BLUE);

        // 绘制任务栏
        let taskbar_height = 30u32;
        let screen_width = self.fb.width();
        let screen_height = self.fb.height();

        self.double_buffer.fill_rect(
            0,
            screen_height - taskbar_height,
            screen_width,
            taskbar_height,
            0xFF303030,
        );
        self.font.draw_string(
            &self.double_buffer,
            10,
            screen_height - taskbar_height + 10,
            "Rux OS Desktop",
            color::WHITE,
        );

        // 绘制窗口
        self.wm.draw_all(&self.double_buffer, &self.font);

        // 绘制面板
        self.launcher_panel.draw(&self.double_buffer, &self.font);
        self.clock_panel.draw(&self.double_buffer, &self.font);

        // 绘制光标
        self.cursor.draw(&self.double_buffer);
    }
}

fn main() {
    let mut desktop = Desktop::new();
    desktop.run();
}
