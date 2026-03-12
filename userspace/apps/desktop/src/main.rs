//! Rux Desktop Environment
//!
//! Userspace desktop environment application

use rux_gui::{
    FramebufferDevice, FontRenderer, DoubleBuffer, MouseCursor,
    WindowManager, SimplePanel, color,
    InputDevice, InputState,
};

/// Desktop environment
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
        // Open framebuffer device
        let fb = match FramebufferDevice::open() {
            Some(fb) => fb,
            None => panic!("Failed to open framebuffer device"),
        };

        // Get screen dimensions
        let screen_width = fb.width();
        let screen_height = fb.height();

        // Initialize double buffering
        let mut double_buffer = DoubleBuffer::new();
        double_buffer.init(screen_width, screen_height, screen_width);

        // Initialize font
        let font = FontRenderer::new_8x8();

        // Initialize cursor
        let cursor = MouseCursor::new(screen_width, screen_height);

        // Initialize window manager
        let mut wm = WindowManager::new();
        wm.create_window("Launcher", 10, 10, 200, 300);
        wm.create_window("Clock", 220, 10, 200, 100);

        // Create launcher panel
        let mut launcher_panel = SimplePanel::new(10, 40, 180, 260);
        launcher_panel.add_label(10, 10, "Applications:");
        launcher_panel.add_button(10, 40, 160, 30, "Calculator");
        launcher_panel.add_button(10, 80, 160, 30, "Terminal");
        launcher_panel.add_button(10, 120, 160, 30, "File Manager");

        // Create clock panel
        let mut clock_panel = SimplePanel::new(220, 40, 180, 60);
        clock_panel.add_label(20, 10, "00:00:00");
        clock_panel.add_label(20, 30, "2026-02-15");

        // Initialize input devices
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
        // Handle keyboard events
        while let Some(event) = self.keyboard.read_event() {
            self.input_state.process_event(&event);

            // Handle keyboard shortcuts
            if event.is_key() && event.is_press() {
                match event.code {
                    rux_gui::input::KEY_ESC => {
                        self.running = false;
                    }
                    _ => {}
                }
            }
        }

        // Handle mouse events
        while let Some(event) = self.mouse.read_event() {
            self.input_state.process_event(&event);

            // Update cursor position
            let (x, y) = self.input_state.mouse_position();
            self.cursor.set_position(x, y);

            // Handle mouse click
            if event.is_left_button() && event.is_press() {
                self.handle_click(x, y);
            }
        }
    }

    fn handle_click(&mut self, _x: i32, _y: i32) {
        // TODO: Handle click events
    }

    fn run(&mut self) {
        while self.running {
            // Handle input events
            self.handle_events();

            // Draw
            self.draw();

            // Refresh screen
            self.double_buffer.swap_buffers_fast(&self.fb);
            self.fb.flush();

            // Delay (~60 FPS)
            std::thread::sleep(std::time::Duration::from_millis(16));
        }
    }

    fn draw(&self) {
        // Clear background
        self.double_buffer.clear(color::BLUE);

        // Draw taskbar
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

        // Draw windows
        self.wm.draw_all(&self.double_buffer, &self.font);

        // Draw panels
        self.launcher_panel.draw(&self.double_buffer, &self.font);
        self.clock_panel.draw(&self.double_buffer, &self.font);

        // Draw cursor
        self.cursor.draw(&self.double_buffer);
    }
}

fn main() {
    let mut desktop = Desktop::new();
    desktop.run();
}
