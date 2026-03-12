//! Rux Clock Application
//!
//! Displays current time and date

use rux_gui::{
    FramebufferDevice, FontRenderer, DoubleBuffer, MouseCursor,
    color, InputDevice, InputDeviceType, InputState,
    input::{BTN_LEFT, EV_KEY, KEY_PRESS, KEY_ESC},
};

/// Clock application
struct ClockApp {
    fb: FramebufferDevice,
    double_buffer: DoubleBuffer,
    font: FontRenderer,
    cursor: MouseCursor,
    keyboard: InputDevice,
    mouse: InputDevice,
    input_state: InputState,
    running: bool,
    window_x: u32,
    window_y: u32,
    window_width: u32,
    window_height: u32,
    /// Last displayed time (for change detection)
    last_time: String,
    last_date: String,
}

impl ClockApp {
    fn new() -> Self {
        // Open framebuffer
        let fb = FramebufferDevice::open()
            .expect("Failed to open framebuffer device");

        let screen_width = fb.width();
        let screen_height = fb.height();

        // Initialize double buffering
        let mut double_buffer = DoubleBuffer::new();
        double_buffer.init(screen_width, screen_height, screen_width);

        // Initialize font
        let font = FontRenderer::new_8x8();

        // Initialize cursor
        let cursor = MouseCursor::new(screen_width, screen_height);

        // Initialize input devices
        let keyboard = InputDevice::keyboard();
        let mouse = InputDevice::pointer();
        let input_state = InputState::new(screen_width, screen_height);

        // Window dimensions
        let window_width = 300u32;
        let window_height = 180u32;
        let window_x = (screen_width - window_width) / 2;
        let window_y = (screen_height - window_height) / 2;

        Self {
            fb,
            double_buffer,
            font,
            cursor,
            keyboard,
            mouse,
            input_state,
            running: true,
            window_x,
            window_y,
            window_width,
            window_height,
            last_time: String::new(),
            last_date: String::new(),
        }
    }

    fn handle_events(&mut self) {
        // Handle keyboard events
        while let Some(event) = self.keyboard.read_event() {
            self.input_state.process_event(&event);

            if event.type_ == EV_KEY && event.value == KEY_PRESS {
                if event.code == KEY_ESC {
                    self.running = false;
                }
            }
        }

        // Handle mouse events (only update cursor position)
        while let Some(event) = self.mouse.read_event() {
            self.input_state.process_event(&event);
            let (x, y) = self.input_state.mouse_position();
            self.cursor.set_position(x, y);
        }
    }

    fn get_current_time() -> (String, String) {
        // Use std::time to get time
        use std::time::{SystemTime, UNIX_EPOCH};

        let duration = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();

        let total_secs = duration.as_secs();
        let days = total_secs / 86400;
        let secs = total_secs % 86400;

        // Calculate date (starting from 1970-01-01)
        // Simplified calculation, not accounting for leap seconds
        let years = 1970 + days / 365;
        let day_of_year = days % 365;

        // Simplified month calculation
        let (month, day) = Self::day_to_month_day(day_of_year as u32);

        // Calculate time
        let hours = secs / 3600;
        let minutes = (secs % 3600) / 60;
        let seconds = secs % 60;

        // Timezone offset (UTC+8)
        let hours = (hours + 8) % 24;

        let time_str = format!("{:02}:{:02}:{:02}", hours, minutes, seconds);
        let date_str = format!("{:04}-{:02}-{:02}", years, month, day);

        (time_str, date_str)
    }

    /// Convert day of year to month and day
    fn day_to_month_day(day: u32) -> (u32, u32) {
        // Days per month (non-leap year)
        let month_days = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];

        let mut remaining = day;
        for (i, &days) in month_days.iter().enumerate() {
            if remaining < days {
                return ((i + 1) as u32, remaining + 1);
            }
            remaining -= days;
        }

        // Default return December 31
        (12, 31)
    }

    fn draw(&mut self) {
        // Get current time
        let (time_str, date_str) = Self::get_current_time();

        // Clear background (semi-transparent effect)
        self.double_buffer.fill_rect(
            self.window_x - 5,
            self.window_y - 5,
            self.window_width + 10,
            self.window_height + 10,
            0x80000000,
        );

        // Draw window background
        self.double_buffer.fill_rect(
            self.window_x,
            self.window_y,
            self.window_width,
            self.window_height,
            0xFF2D2D2D,
        );

        // Draw window border
        self.double_buffer.blit_rect(
            self.window_x,
            self.window_y,
            self.window_width,
            self.window_height,
            0xFF00AA00,
            2,
        );

        // Draw title bar
        self.double_buffer.fill_rect(
            self.window_x,
            self.window_y,
            self.window_width,
            25,
            0xFF00AA00,
        );
        self.font.draw_string(
            &self.double_buffer,
            self.window_x + 10,
            self.window_y + 8,
            "Clock",
            color::WHITE,
        );

        // Draw close button
        self.double_buffer.fill_rect(
            self.window_x + self.window_width - 25,
            self.window_y + 5,
            18,
            15,
            0xFFCC0000,
        );
        self.font.draw_string(
            &self.double_buffer,
            self.window_x + self.window_width - 21,
            self.window_y + 8,
            "X",
            color::WHITE,
        );

        // Draw time (large font effect)
        let time_y = self.window_y + 60;
        let time_width = self.font.measure_text(&time_str);
        let time_x = self.window_x + (self.window_width - time_width) / 2;

        // Time background
        self.double_buffer.fill_rect(
            self.window_x + 20,
            time_y - 10,
            self.window_width - 40,
            40,
            0xFF1A1A1A,
        );

        self.font.draw_string(
            &self.double_buffer,
            time_x,
            time_y,
            &time_str,
            0xFF00FF00,
        );

        // Draw date
        let date_y = time_y + 50;
        let date_width = self.font.measure_text(&date_str);
        let date_x = self.window_x + (self.window_width - date_width) / 2;

        self.font.draw_string(
            &self.double_buffer,
            date_x,
            date_y,
            &date_str,
            0xFFA0A0A0,
        );

        // Draw weekday
        let weekday = Self::get_weekday();
        let weekday_y = date_y + 25;
        let weekday_width = self.font.measure_text(&weekday);
        let weekday_x = self.window_x + (self.window_width - weekday_width) / 2;

        self.font.draw_string(
            &self.double_buffer,
            weekday_x,
            weekday_y,
            &weekday,
            0xFF808080,
        );

        // Draw cursor
        self.cursor.draw(&self.double_buffer);

        // Update cache
        self.last_time = time_str;
        self.last_date = date_str;
    }

    /// Get day of week
    fn get_weekday() -> String {
        use std::time::{SystemTime, UNIX_EPOCH};

        let duration = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();

        let total_secs = duration.as_secs();
        let days = total_secs / 86400;

        // 1970-01-01 is Thursday
        let weekday = (days + 4) % 7;

        match weekday {
            0 => String::from("Sunday"),
            1 => String::from("Monday"),
            2 => String::from("Tuesday"),
            3 => String::from("Wednesday"),
            4 => String::from("Thursday"),
            5 => String::from("Friday"),
            6 => String::from("Saturday"),
            _ => String::from("Unknown"),
        }
    }

    fn run(&mut self) {
        while self.running {
            self.handle_events();
            self.draw();
            self.double_buffer.swap_buffers_fast(&self.fb);
            self.fb.flush();
            std::thread::sleep(std::time::Duration::from_millis(100)); // 10 FPS is sufficient
        }
    }
}

fn main() {
    let mut app = ClockApp::new();
    app.run();
}
