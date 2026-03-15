//! Rux Desktop Environment
//!
//! Userspace desktop environment application

use rux_gui::{FramebufferDevice, FontRenderer, MouseCursor, WindowManager, SimplePanel, color};

fn main() {
    let fb = match FramebufferDevice::open() {
        Some(fb) => fb,
        None => loop {}
    };

    let screen_width = fb.width();
    let screen_height = fb.height();

    let font = FontRenderer::new_8x8();
    let cursor = MouseCursor::new(screen_width, screen_height);

    // Initialize window manager with windows
    let mut wm = WindowManager::new();
    wm.create_window("Launcher", 10, 10, 200, 300);
    wm.create_window("Clock", 220, 10, 200, 100);

    // Create panels inside windows
    let mut launcher_panel = SimplePanel::new(10, 40, 180, 250);
    launcher_panel.add_label(10, 10, "Applications:");
    launcher_panel.add_button(10, 40, 160, 30, "Calculator");
    launcher_panel.add_button(10, 80, 160, 30, "Terminal");
    launcher_panel.add_button(10, 120, 160, 30, "Files");

    let mut clock_panel = SimplePanel::new(220, 40, 180, 50);
    clock_panel.add_label(10, 10, "00:00:00");

    loop {
        // Clear background
        fb.clear(color::BLUE);

        // Draw taskbar
        let taskbar_height = 30u32;
        fb.fill_rect(
            0,
            screen_height - taskbar_height,
            screen_width,
            taskbar_height,
            0xFF303030,
        );
        font.draw_string(
            &fb,
            10,
            screen_height - taskbar_height + 10,
            "Rux OS Desktop",
            color::WHITE,
        );

        // Draw windows
        wm.draw_all(&fb, &font);

        // Draw panels
        launcher_panel.draw(&fb, &font);
        clock_panel.draw(&fb, &font);

        // Draw cursor
        cursor.draw(&fb);

        // Flush to display
        fb.flush();

        // Simple delay (~60 FPS)
        for _ in 0..1_000_000 {
            core::hint::spin_loop();
        }
    }
}
