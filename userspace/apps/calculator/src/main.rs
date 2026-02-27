//! Rux 计算器应用
//!
//! 简单的图形化计算器

use rux_gui::{
    FramebufferDevice, FontRenderer, DoubleBuffer, MouseCursor,
    color, InputDevice, InputDeviceType, InputState,
    widgets::{SimplePanel, WidgetEvent},
    input::{BTN_LEFT, EV_KEY, KEY_PRESS},
};

/// 计算器状态
struct Calculator {
    /// 显示屏内容
    display: String,
    /// 当前操作数
    current: f64,
    /// 上一个操作数
    previous: f64,
    /// 当前运算符
    operator: Option<char>,
    /// 是否需要清空显示屏
    should_clear: bool,
}

impl Calculator {
    fn new() -> Self {
        Self {
            display: String::from("0"),
            current: 0.0,
            previous: 0.0,
            operator: None,
            should_clear: false,
        }
    }

    /// 输入数字
    fn input_digit(&mut self, digit: char) {
        if self.should_clear {
            self.display.clear();
            self.should_clear = false;
        }

        if self.display == "0" && digit != '.' {
            self.display.clear();
        }

        // 限制小数点
        if digit == '.' && self.display.contains('.') {
            return;
        }

        // 限制显示长度
        if self.display.len() >= 16 {
            return;
        }

        self.display.push(digit);
    }

    /// 输入运算符
    fn input_operator(&mut self, op: char) {
        if let Ok(value) = self.display.parse::<f64>() {
            self.current = value;
        }

        if self.operator.is_some() {
            self.calculate();
        }

        self.previous = self.current;
        self.operator = Some(op);
        self.should_clear = true;
    }

    /// 计算结果
    fn calculate(&mut self) {
        if let Ok(value) = self.display.parse::<f64>() {
            self.current = value;
        }

        let result = match self.operator {
            Some('+') => self.previous + self.current,
            Some('-') => self.previous - self.current,
            Some('*') => self.previous * self.current,
            Some('/') => {
                if self.current != 0.0 {
                    self.previous / self.current
                } else {
                    self.display = String::from("Error");
                    self.operator = None;
                    self.should_clear = true;
                    return;
                }
            }
            Some('%') => self.previous % self.current,
            _ => self.current,
        };

        // 格式化结果
        if result.fract() == 0.0 && result.abs() < 1e15 {
            self.display = format!("{}", result as i64);
        } else {
            self.display = format!("{:.10}", result);
            // 移除末尾的零
            while self.display.ends_with('0') && self.display.contains('.') {
                self.display.pop();
            }
            if self.display.ends_with('.') {
                self.display.pop();
            }
        }

        self.operator = None;
        self.should_clear = true;
    }

    /// 清空
    fn clear(&mut self) {
        self.display = String::from("0");
        self.current = 0.0;
        self.previous = 0.0;
        self.operator = None;
        self.should_clear = false;
    }

    /// 退格
    fn backspace(&mut self) {
        if self.display.len() > 1 {
            self.display.pop();
        } else {
            self.display = String::from("0");
        }
    }

    /// 正负号切换
    fn toggle_sign(&mut self) {
        if self.display.starts_with('-') {
            self.display.remove(0);
        } else if self.display != "0" {
            self.display.insert(0, '-');
        }
    }
}

/// 计算器应用
struct CalculatorApp {
    fb: FramebufferDevice,
    double_buffer: DoubleBuffer,
    font: FontRenderer,
    cursor: MouseCursor,
    keyboard: InputDevice,
    mouse: InputDevice,
    input_state: InputState,
    panel: SimplePanel,
    calc: Calculator,
    running: bool,
    window_x: u32,
    window_y: u32,
    window_width: u32,
    window_height: u32,
}

impl CalculatorApp {
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
        let window_width = 260u32;
        let window_height = 380u32;
        let window_x = (screen_width - window_width) / 2;
        let window_y = (screen_height - window_height) / 2;

        // 创建按钮面板
        let mut panel = SimplePanel::new(window_x + 10, window_y + 70, 240, 300);

        // 显示屏下方是按钮区域
        let btn_w = 55u32;
        let btn_h = 45u32;
        let gap = 5u32;
        let start_y = 0u32;

        // 第一行: C, +/-, %, /
        panel.add_button(0, start_y, btn_w, btn_h, "C");
        panel.add_button(btn_w + gap, start_y, btn_w, btn_h, "+/-");
        panel.add_button(2 * (btn_w + gap), start_y, btn_w, btn_h, "%");
        panel.add_button(3 * (btn_w + gap), start_y, btn_w, btn_h, "/");

        // 第二行: 7, 8, 9, *
        let row1 = start_y + btn_h + gap;
        panel.add_button(0, row1, btn_w, btn_h, "7");
        panel.add_button(btn_w + gap, row1, btn_w, btn_h, "8");
        panel.add_button(2 * (btn_w + gap), row1, btn_w, btn_h, "9");
        panel.add_button(3 * (btn_w + gap), row1, btn_w, btn_h, "*");

        // 第三行: 4, 5, 6, -
        let row2 = row1 + btn_h + gap;
        panel.add_button(0, row2, btn_w, btn_h, "4");
        panel.add_button(btn_w + gap, row2, btn_w, btn_h, "5");
        panel.add_button(2 * (btn_w + gap), row2, btn_w, btn_h, "6");
        panel.add_button(3 * (btn_w + gap), row2, btn_w, btn_h, "-");

        // 第四行: 1, 2, 3, +
        let row3 = row2 + btn_h + gap;
        panel.add_button(0, row3, btn_w, btn_h, "1");
        panel.add_button(btn_w + gap, row3, btn_w, btn_h, "2");
        panel.add_button(2 * (btn_w + gap), row3, btn_w, btn_h, "3");
        panel.add_button(3 * (btn_w + gap), row3, btn_w, btn_h, "+");

        // 第五行: 0, ., =, <- (退格)
        let row4 = row3 + btn_h + gap;
        panel.add_button(0, row4, btn_w, btn_h, "0");
        panel.add_button(btn_w + gap, row4, btn_w, btn_h, ".");
        panel.add_button(2 * (btn_w + gap), row4, btn_w, btn_h, "=");
        panel.add_button(3 * (btn_w + gap), row4, btn_w, btn_h, "<-");

        Self {
            fb,
            double_buffer,
            font,
            cursor,
            keyboard,
            mouse,
            input_state,
            panel,
            calc: Calculator::new(),
            running: true,
            window_x,
            window_y,
            window_width,
            window_height,
        }
    }

    fn handle_events(&mut self) {
        // 处理键盘事件
        while let Some(event) = self.keyboard.read_event() {
            self.input_state.process_event(&event);

            if event.type_ == EV_KEY && event.value == KEY_PRESS {
                // 使用 rux_gui::input 中的常量
                use rux_gui::input::*;
                match event.code {
                    KEY_ESC => self.running = false,
                    KEY_1 => self.calc.input_digit('1'),
                    KEY_2 => self.calc.input_digit('2'),
                    KEY_3 => self.calc.input_digit('3'),
                    KEY_4 => self.calc.input_digit('4'),
                    KEY_5 => self.calc.input_digit('5'),
                    KEY_6 => self.calc.input_digit('6'),
                    KEY_7 => self.calc.input_digit('7'),
                    KEY_8 => self.calc.input_digit('8'),
                    KEY_9 => self.calc.input_digit('9'),
                    KEY_0 => self.calc.input_digit('0'),
                    KEY_BACKSPACE => self.calc.backspace(),
                    KEY_ENTER => self.calc.calculate(),
                    KEY_A => self.calc.clear(), // A for All Clear
                    _ => {}
                }
            }
        }

        // 处理鼠标事件
        while let Some(event) = self.mouse.read_event() {
            self.input_state.process_event(&event);

            let (x, y) = self.input_state.mouse_position();
            self.cursor.set_position(x, y);

            if event.type_ == EV_KEY && event.code == BTN_LEFT {
                let widget_event = if event.value == KEY_PRESS {
                    WidgetEvent::MouseDown { x: x as u32, y: y as u32 }
                } else {
                    WidgetEvent::MouseUp { x: x as u32, y: y as u32 }
                };
                self.panel.handle_mouse(widget_event);

                // 检查按钮点击
                if event.value == KEY_PRESS {
                    // 鼠标按下时处理
                } else {
                    // 鼠标释放时检查点击
                    self.handle_button_click(x as u32, y as u32);
                }
            } else if event.type_ == rux_gui::input::EV_REL {
                // 鼠标移动
                let widget_event = WidgetEvent::MouseMove { x: x as u32, y: y as u32 };
                self.panel.handle_mouse(widget_event);
            }
        }
    }

    fn handle_button_click(&mut self, _x: u32, _y: u32) {
        let buttons = &mut self.panel.buttons;
        for i in 0..buttons.len() {
            if buttons[i].was_clicked() {
                let text = buttons[i].text.clone();
                match text.as_str() {
                    "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" => {
                        self.calc.input_digit(text.chars().next().unwrap());
                    }
                    "." => self.calc.input_digit('.'),
                    "+" | "-" | "*" | "/" | "%" => {
                        self.calc.input_operator(text.chars().next().unwrap());
                    }
                    "=" => self.calc.calculate(),
                    "C" => self.calc.clear(),
                    "<-" => self.calc.backspace(),
                    "+/-" => self.calc.toggle_sign(),
                    _ => {}
                }
                break;
            }
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
            0xFF2D2D2D,
        );

        // 绘制窗口边框
        self.double_buffer.blit_rect(
            self.window_x,
            self.window_y,
            self.window_width,
            self.window_height,
            0xFF0066CC,
            2,
        );

        // 绘制标题栏
        self.double_buffer.fill_rect(
            self.window_x,
            self.window_y,
            self.window_width,
            25,
            0xFF0066CC,
        );
        self.font.draw_string(
            &self.double_buffer,
            self.window_x + 10,
            self.window_y + 8,
            "Calculator",
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

        // 绘制显示屏
        let display_x = self.window_x + 10;
        let display_y = self.window_y + 35;
        let display_w = self.window_width - 20;
        let display_h = 30;

        self.double_buffer.fill_rect(
            display_x,
            display_y,
            display_w,
            display_h,
            0xFF1A1A1A,
        );
        self.double_buffer.blit_rect(
            display_x,
            display_y,
            display_w,
            display_h,
            0xFF404040,
            1,
        );

        // 右对齐显示数字
        let text_width = self.font.measure_text(&self.calc.display);
        let text_x = display_x + display_w.saturating_sub(text_width + 8);
        let text_y = display_y + (display_h - self.font.height()) / 2;

        // 限制显示长度
        let display_text = if self.calc.display.len() > 18 {
            format!("{}...", &self.calc.display[..15])
        } else {
            self.calc.display.clone()
        };

        self.font.draw_string(
            &self.double_buffer,
            text_x,
            text_y,
            &display_text,
            color::WHITE,
        );

        // 绘制按钮
        self.panel.draw(&self.double_buffer, &self.font);

        // 绘制光标
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
    let mut app = CalculatorApp::new();
    app.run();
}
