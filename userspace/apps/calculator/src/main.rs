//! Rux Calculator Application
//!
//! Simple graphical calculator

use rux_gui::{
    FramebufferDevice, FontRenderer, DoubleBuffer, MouseCursor,
    color, InputDevice, InputDeviceType, InputState,
    widgets::{SimplePanel, WidgetEvent},
    input::{BTN_LEFT, EV_KEY, KEY_PRESS},
};

/// Calculator state
struct Calculator {
    /// Display content
    display: String,
    /// Current operand
    current: f64,
    /// Previous operand
    previous: f64,
    /// Current operator
    operator: Option<char>,
    /// Whether to clear the display
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

    /// Input a digit
    fn input_digit(&mut self, digit: char) {
        if self.should_clear {
            self.display.clear();
            self.should_clear = false;
        }

        if self.display == "0" && digit != '.' {
            self.display.clear();
        }

        // Limit decimal points
        if digit == '.' && self.display.contains('.') {
            return;
        }

        // Limit display length
        if self.display.len() >= 16 {
            return;
        }

        self.display.push(digit);
    }

    /// Input an operator
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

    /// Calculate the result
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

        // Format the result
        if result.fract() == 0.0 && result.abs() < 1e15 {
            self.display = format!("{}", result as i64);
        } else {
            self.display = format!("{:.10}", result);
            // Remove trailing zeros
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

    /// Clear
    fn clear(&mut self) {
        self.display = String::from("0");
        self.current = 0.0;
        self.previous = 0.0;
        self.operator = None;
        self.should_clear = false;
    }

    /// Backspace
    fn backspace(&mut self) {
        if self.display.len() > 1 {
            self.display.pop();
        } else {
            self.display = String::from("0");
        }
    }

    /// Toggle sign
    fn toggle_sign(&mut self) {
        if self.display.starts_with('-') {
            self.display.remove(0);
        } else if self.display != "0" {
            self.display.insert(0, '-');
        }
    }
}

/// Calculator application
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
        let window_width = 260u32;
        let window_height = 380u32;
        let window_x = (screen_width - window_width) / 2;
        let window_y = (screen_height - window_height) / 2;

        // Create button panel
        let mut panel = SimplePanel::new(window_x + 10, window_y + 70, 240, 300);

        // Button area below the display
        let btn_w = 55u32;
        let btn_h = 45u32;
        let gap = 5u32;
        let start_y = 0u32;

        // Row 1: C, +/-, %, /
        panel.add_button(0, start_y, btn_w, btn_h, "C");
        panel.add_button(btn_w + gap, start_y, btn_w, btn_h, "+/-");
        panel.add_button(2 * (btn_w + gap), start_y, btn_w, btn_h, "%");
        panel.add_button(3 * (btn_w + gap), start_y, btn_w, btn_h, "/");

        // Row 2: 7, 8, 9, *
        let row1 = start_y + btn_h + gap;
        panel.add_button(0, row1, btn_w, btn_h, "7");
        panel.add_button(btn_w + gap, row1, btn_w, btn_h, "8");
        panel.add_button(2 * (btn_w + gap), row1, btn_w, btn_h, "9");
        panel.add_button(3 * (btn_w + gap), row1, btn_w, btn_h, "*");

        // Row 3: 4, 5, 6, -
        let row2 = row1 + btn_h + gap;
        panel.add_button(0, row2, btn_w, btn_h, "4");
        panel.add_button(btn_w + gap, row2, btn_w, btn_h, "5");
        panel.add_button(2 * (btn_w + gap), row2, btn_w, btn_h, "6");
        panel.add_button(3 * (btn_w + gap), row2, btn_w, btn_h, "-");

        // Row 4: 1, 2, 3, +
        let row3 = row2 + btn_h + gap;
        panel.add_button(0, row3, btn_w, btn_h, "1");
        panel.add_button(btn_w + gap, row3, btn_w, btn_h, "2");
        panel.add_button(2 * (btn_w + gap), row3, btn_w, btn_h, "3");
        panel.add_button(3 * (btn_w + gap), row3, btn_w, btn_h, "+");

        // Row 5: 0, ., =, <- (backspace)
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
        // Handle keyboard events
        while let Some(event) = self.keyboard.read_event() {
            self.input_state.process_event(&event);

            if event.type_ == EV_KEY && event.value == KEY_PRESS {
                // Use constants from rux_gui::input
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

        // Handle mouse events
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

                // Check button click
                if event.value == KEY_PRESS {
                    // Handle mouse down
                } else {
                    // Check click on mouse up
                    self.handle_button_click(x as u32, y as u32);
                }
            } else if event.type_ == rux_gui::input::EV_REL {
                // Mouse movement
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
            0xFF0066CC,
            2,
        );

        // Draw title bar
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

        // Draw display
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

        // Right-align displayed number
        let text_width = self.font.measure_text(&self.calc.display);
        let text_x = display_x + display_w.saturating_sub(text_width + 8);
        let text_y = display_y + (display_h - self.font.height()) / 2;

        // Limit display length
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

        // Draw buttons
        self.panel.draw(&self.double_buffer, &self.font);

        // Draw cursor
        self.cursor.draw(&self.double_buffer);
    }

    fn run(&mut self) {
        while self.running {
            self.handle_events();
            self.draw();
            self.double_buffer.swap_buffers_fast(&self.fb);
            self.fb.flush();
            std::thread::sleep(std::time::Duration::from_millis(16));
        }
    }
}

fn main() {
    let mut app = CalculatorApp::new();
    app.run();
}
