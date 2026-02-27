//! Rux 可视化 Shell 应用
//!
//! 图形化终端模拟器

use rux_gui::{
    FramebufferDevice, FontRenderer, DoubleBuffer, MouseCursor,
    color, InputDevice, InputDeviceType, InputState,
    input::{EV_KEY, KEY_PRESS, KEY_RELEASE, BTN_LEFT},
};

/// 终端行数
const TERMINAL_ROWS: usize = 25;
/// 每行字符数
const TERMINAL_COLS: usize = 80;

/// 可视化 Shell
struct VisualShell {
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
    /// 终端缓冲区
    buffer: [[char; TERMINAL_COLS]; TERMINAL_ROWS],
    /// 当前输入行
    input_line: String,
    /// 光标位置
    cursor_col: usize,
    /// 输出行数
    output_rows: usize,
    /// 滚动偏移
    scroll_offset: usize,
}

impl VisualShell {
    fn new() -> Self {
        // 打开 framebuffer
        let fb = FramebufferDevice::open()
            .expect("Failed to open framebuffer device");

        let screen_width = fb.width();
        let screen_height = fb.height();

        // 初始化双缓冲
        let mut double_buffer = DoubleBuffer::new();
        double_buffer.init(screen_width, screen_height, screen_width);

        // 初始化字体
        let font = FontRenderer::new_8x8();

        // 初始化光标
        let cursor = MouseCursor::new(screen_width, screen_height);

        // 初始化输入设备
        let keyboard = InputDevice::keyboard();
        let mouse = InputDevice::pointer();
        let input_state = InputState::new(screen_width, screen_height);

        // 窗口尺寸
        let window_width = 680u32;
        let window_height = 260u32;
        let window_x = (screen_width - window_width) / 2;
        let window_y = (screen_height - window_height) / 2;

        // 初始化终端缓冲区
        let mut buffer = [[' '; TERMINAL_COLS]; TERMINAL_ROWS];

        // 显示欢迎信息
        let welcome = [
            "Rux Visual Shell v0.1",
            "Type 'help' for available commands",
            "",
        ];

        for (i, line) in welcome.iter().enumerate() {
            for (j, ch) in line.chars().enumerate() {
                if j < TERMINAL_COLS {
                    buffer[i][j] = ch;
                }
            }
        }

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
            buffer,
            input_line: String::new(),
            cursor_col: 0,
            output_rows: welcome.len(),
            scroll_offset: 0,
        }
    }

    fn handle_events(&mut self) {
        use rux_gui::input::*;

        // 处理键盘事件
        while let Some(event) = self.keyboard.read_event() {
            self.input_state.process_event(&event);

            if event.type_ == EV_KEY && event.value == KEY_PRESS {
                match event.code {
                    KEY_ESC => self.running = false,
                    KEY_ENTER => self.execute_command(),
                    KEY_BACKSPACE => self.backspace(),
                    KEY_UP => self.scroll_up(),
                    KEY_DOWN => self.scroll_down(),
                    KEY_LEFT => self.cursor_left(),
                    KEY_RIGHT => self.cursor_right(),
                    _ => {
                        // 输入字符
                        if let Some(ch) = self.keycode_to_char(event.code) {
                            self.input_char(ch);
                        }
                    }
                }
            }
        }

        // 处理鼠标事件
        while let Some(event) = self.mouse.read_event() {
            self.input_state.process_event(&event);
            let (x, y) = self.input_state.mouse_position();
            self.cursor.set_position(x, y);
        }
    }

    /// 将键码转换为字符
    fn keycode_to_char(&self, code: u16) -> Option<char> {
        use rux_gui::input::*;

        // 检查 Shift 状态
        let shift = self.input_state.shift_pressed;

        match code {
            KEY_A => Some(if shift { 'A' } else { 'a' }),
            KEY_B => Some(if shift { 'B' } else { 'b' }),
            KEY_C => Some(if shift { 'C' } else { 'c' }),
            KEY_D => Some(if shift { 'D' } else { 'd' }),
            KEY_E => Some(if shift { 'E' } else { 'e' }),
            KEY_F => Some(if shift { 'F' } else { 'f' }),
            KEY_G => Some(if shift { 'G' } else { 'g' }),
            KEY_H => Some(if shift { 'H' } else { 'h' }),
            KEY_I => Some(if shift { 'I' } else { 'i' }),
            KEY_J => Some(if shift { 'J' } else { 'j' }),
            KEY_K => Some(if shift { 'K' } else { 'k' }),
            KEY_L => Some(if shift { 'L' } else { 'l' }),
            KEY_M => Some(if shift { 'M' } else { 'm' }),
            KEY_N => Some(if shift { 'N' } else { 'n' }),
            KEY_O => Some(if shift { 'O' } else { 'o' }),
            KEY_P => Some(if shift { 'P' } else { 'p' }),
            KEY_Q => Some(if shift { 'Q' } else { 'q' }),
            KEY_R => Some(if shift { 'R' } else { 'r' }),
            KEY_S => Some(if shift { 'S' } else { 's' }),
            KEY_T => Some(if shift { 'T' } else { 't' }),
            KEY_U => Some(if shift { 'U' } else { 'u' }),
            KEY_V => Some(if shift { 'V' } else { 'v' }),
            KEY_W => Some(if shift { 'W' } else { 'w' }),
            KEY_X => Some(if shift { 'X' } else { 'x' }),
            KEY_Y => Some(if shift { 'Y' } else { 'y' }),
            KEY_Z => Some(if shift { 'Z' } else { 'z' }),
            KEY_1 => Some(if shift { '!' } else { '1' }),
            KEY_2 => Some(if shift { '@' } else { '2' }),
            KEY_3 => Some(if shift { '#' } else { '3' }),
            KEY_4 => Some(if shift { '$' } else { '4' }),
            KEY_5 => Some(if shift { '%' } else { '5' }),
            KEY_6 => Some(if shift { '^' } else { '6' }),
            KEY_7 => Some(if shift { '&' } else { '7' }),
            KEY_8 => Some(if shift { '*' } else { '8' }),
            KEY_9 => Some(if shift { '(' } else { '9' }),
            KEY_0 => Some(if shift { ')' } else { '0' }),
            KEY_SPACE => Some(' '),
            _ => None,
        }
    }

    fn input_char(&mut self, ch: char) {
        if self.input_line.len() < TERMINAL_COLS - 2 {
            self.input_line.insert(self.cursor_col, ch);
            self.cursor_col += 1;
        }
    }

    fn backspace(&mut self) {
        if self.cursor_col > 0 {
            self.cursor_col -= 1;
            self.input_line.remove(self.cursor_col);
        }
    }

    fn cursor_left(&mut self) {
        if self.cursor_col > 0 {
            self.cursor_col -= 1;
        }
    }

    fn cursor_right(&mut self) {
        if self.cursor_col < self.input_line.len() {
            self.cursor_col += 1;
        }
    }

    fn scroll_up(&mut self) {
        if self.scroll_offset > 0 {
            self.scroll_offset -= 1;
        }
    }

    fn scroll_down(&mut self) {
        if self.scroll_offset < self.output_rows.saturating_sub(TERMINAL_ROWS - 2) {
            self.scroll_offset += 1;
        }
    }

    fn execute_command(&mut self) {
        // Clone the command to avoid borrow issues
        let cmd = self.input_line.trim().to_string();

        // 将输入行添加到缓冲区
        self.add_to_buffer(&format!("$ {}", cmd));

        if cmd.is_empty() {
            self.show_prompt();
            return;
        }

        // 解析命令
        let parts: Vec<String> = cmd.split_whitespace().map(|s| s.to_string()).collect();
        if parts.is_empty() {
            self.show_prompt();
            return;
        }

        match parts[0].as_str() {
            "help" => self.cmd_help(),
            "clear" => self.cmd_clear(),
            "echo" => {
                let args: Vec<&str> = parts[1..].iter().map(|s| s.as_str()).collect();
                self.cmd_echo(&args);
            }
            "date" => self.cmd_date(),
            "whoami" => self.add_to_buffer("root"),
            "uname" => {
                let args: Vec<&str> = parts[1..].iter().map(|s| s.as_str()).collect();
                self.cmd_uname(&args);
            }
            "ls" => {
                let args: Vec<&str> = parts[1..].iter().map(|s| s.as_str()).collect();
                self.cmd_ls(&args);
            }
            "pwd" => self.add_to_buffer("/"),
            "exit" | "quit" => self.running = false,
            _ => self.add_to_buffer(&format!("Unknown command: {}", parts[0])),
        }

        self.show_prompt();
    }

    fn show_prompt(&mut self) {
        self.input_line.clear();
        self.cursor_col = 0;
    }

    fn add_to_buffer(&mut self, line: &str) {
        // 滚动缓冲区
        if self.output_rows >= TERMINAL_ROWS - 1 {
            // 向上滚动一行
            for i in 0..TERMINAL_ROWS - 1 {
                self.buffer[i] = self.buffer[i + 1];
            }
            // 清空最后一行
            self.buffer[TERMINAL_ROWS - 1] = [' '; TERMINAL_COLS];
        } else {
            self.output_rows += 1;
        }

        // 添加新行
        let row = if self.output_rows >= TERMINAL_ROWS {
            TERMINAL_ROWS - 1
        } else {
            self.output_rows - 1
        };

        for (i, ch) in line.chars().enumerate() {
            if i < TERMINAL_COLS {
                self.buffer[row][i] = ch;
            }
        }
    }

    // 命令实现
    fn cmd_help(&mut self) {
        let help = [
            "Available commands:",
            "  help     - Show this help",
            "  clear    - Clear screen",
            "  echo     - Print text",
            "  date     - Show current date/time",
            "  whoami   - Show current user",
            "  uname    - Show system info",
            "  ls       - List files",
            "  pwd      - Print working directory",
            "  exit     - Exit shell",
        ];
        for line in help {
            self.add_to_buffer(line);
        }
    }

    fn cmd_clear(&mut self) {
        self.buffer = [[' '; TERMINAL_COLS]; TERMINAL_ROWS];
        self.output_rows = 0;
        self.scroll_offset = 0;
    }

    fn cmd_echo(&mut self, args: &[&str]) {
        let text = args.join(" ");
        self.add_to_buffer(&text);
    }

    fn cmd_date(&mut self) {
        use std::time::{SystemTime, UNIX_EPOCH};

        let duration = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();

        let total_secs = duration.as_secs();
        let days = total_secs / 86400;
        let secs = total_secs % 86400;

        let years = 1970 + days / 365;
        let day_of_year = days % 365;
        let (month, day) = Self::day_to_month_day(day_of_year as u32);

        let hours = (secs / 3600 + 8) % 24;
        let minutes = (secs % 3600) / 60;
        let seconds = secs % 60;

        self.add_to_buffer(&format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            years, month, day, hours, minutes, seconds
        ));
    }

    fn day_to_month_day(day: u32) -> (u32, u32) {
        let month_days = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
        let mut remaining = day;
        for (i, &days) in month_days.iter().enumerate() {
            if remaining < days {
                return ((i + 1) as u32, remaining + 1);
            }
            remaining -= days;
        }
        (12, 31)
    }

    fn cmd_uname(&mut self, args: &[&str]) {
        if args.contains(&"-a") {
            self.add_to_buffer("Rux 0.1.0 riscv64 GNU/Linux");
        } else {
            self.add_to_buffer("Rux");
        }
    }

    fn cmd_ls(&mut self, _args: &[&str]) {
        let files = [
            "bin/",
            "dev/",
            "etc/",
            "home/",
            "lib/",
            "proc/",
            "tmp/",
            "usr/",
        ];
        for file in files {
            self.add_to_buffer(file);
        }
    }

    fn draw(&self) {
        // 清空背景（半透明效果）
        self.double_buffer.fill_rect(
            self.window_x - 5,
            self.window_y - 5,
            self.window_width + 10,
            self.window_height + 10,
            0x80000000,
        );

        // 绘制窗口背景
        self.double_buffer.fill_rect(
            self.window_x,
            self.window_y,
            self.window_width,
            self.window_height,
            0xFF1A1A1A,
        );

        // 绘制窗口边框
        self.double_buffer.blit_rect(
            self.window_x,
            self.window_y,
            self.window_width,
            self.window_height,
            0xFF333333,
            2,
        );

        // 绘制标题栏
        self.double_buffer.fill_rect(
            self.window_x,
            self.window_y,
            self.window_width,
            25,
            0xFF333333,
        );
        self.font.draw_string(
            &self.double_buffer,
            self.window_x + 10,
            self.window_y + 8,
            "Visual Shell",
            color::WHITE,
        );

        // 绘制关闭按钮
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

        // 绘制终端内容区域
        let content_x = self.window_x + 5;
        let content_y = self.window_y + 30;
        let char_width = self.font.width();
        let char_height = self.font.height();

        // 绘制终端文本
        for row in 0..TERMINAL_ROWS - 1 {
            let buffer_row = row + self.scroll_offset;
            if buffer_row < TERMINAL_ROWS {
                for col in 0..TERMINAL_COLS {
                    let ch = self.buffer[buffer_row][col];
                    if ch != ' ' {
                        let x = content_x + (col as u32) * char_width;
                        let y = content_y + (row as u32) * char_height;
                        self.font.draw_char(&self.double_buffer, x, y, ch as u8, color::WHITE);
                    }
                }
            }
        }

        // 绘制输入行（最后一行）
        let input_row = TERMINAL_ROWS - 1;
        let prompt = "$ ";
        self.font.draw_string(
            &self.double_buffer,
            content_x,
            content_y + (input_row as u32) * char_height,
            prompt,
            0xFF00FF00,
        );

        // 绘制输入内容
        let input_x = content_x + (prompt.len() as u32) * char_width;
        let input_y = content_y + (input_row as u32) * char_height;
        self.font.draw_string(
            &self.double_buffer,
            input_x,
            input_y,
            &self.input_line,
            color::WHITE,
        );

        // 绘制光标
        let cursor_x = input_x + (self.cursor_col as u32) * char_width;
        self.double_buffer.fill_rect(
            cursor_x,
            input_y,
            char_width,
            char_height,
            0xFFFFFFFF,
        );

        // 绘制鼠标光标
        self.cursor.draw(&self.double_buffer);
    }

    fn run(&mut self) {
        while self.running {
            self.handle_events();
            self.draw();
            self.double_buffer.swap_buffers(&self.fb);
            std::thread::sleep(std::time::Duration::from_millis(16));
        }
    }
}

fn main() {
    let mut app = VisualShell::new();
    app.run();
}
