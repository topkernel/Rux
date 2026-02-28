//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! Framebuffer 内核态测试
//!
//! 直接在内核中测试 VirtIO-GPU 帧缓冲区绘制功能
//! 验证从内核到显示设备的通路是否正常

use crate::println;
use crate::drivers::gpu;

/// 颜色常量 (XRGB 格式，与 VirtIO-GPU B8G8R8A8_UNORM 兼容)
const COLOR_BLACK: u32 = 0xFF_00_00_00;
const COLOR_WHITE: u32 = 0xFF_FF_FF_FF;
const COLOR_RED: u32 = 0xFF_00_00_FF;
const COLOR_GREEN: u32 = 0xFF_00_FF_00;
const COLOR_BLUE: u32 = 0xFF_FF_00_00;
const COLOR_YELLOW: u32 = 0xFF_00_FF_FF;
const COLOR_CYAN: u32 = 0xFF_FF_FF_00;
const COLOR_MAGENTA: u32 = 0xFF_FF_00_FF;
const COLOR_GRAY: u32 = 0xFF_80_80_80;

/// 在帧缓冲区上绘制一个像素
/// stride 是每行的字节数
fn put_pixel(fb_ptr: *mut u8, _width: u32, stride: u32, x: u32, y: u32, color: u32) {
    unsafe {
        let offset = (y * stride + x * 4) as usize;
        let pixel_ptr = fb_ptr.add(offset) as *mut u32;
        core::ptr::write_volatile(pixel_ptr, color);
    }
}

/// 填充矩形
fn fill_rect(fb_ptr: *mut u8, width: u32, stride: u32,
             x: u32, y: u32, rect_w: u32, rect_h: u32, color: u32) {
    let x_end = (x + rect_w).min(width);
    let y_end = y + rect_h;

    for py in y..y_end {
        for px in x..x_end {
            put_pixel(fb_ptr, width, stride, px, py, color);
        }
    }
}

/// 绘制字符串 (使用简单的 8x8 字体)
fn draw_char(fb_ptr: *mut u8, width: u32, stride: u32,
             x: u32, y: u32, c: char, color: u32) {
    // 简单的 8x8 字体数据 (只包含部分字符)
    const FONT_8X8: [[u8; 8]; 128] = {
        let mut font = [[0u8; 8]; 128];
        // 'R'
        font['R' as usize] = [0x7C, 0xC6, 0xC6, 0x7C, 0xD8, 0xCC, 0xC6, 0x00];
        // 'u'
        font['u' as usize] = [0x00, 0x00, 0xCC, 0xCC, 0xCC, 0xCC, 0x76, 0x00];
        // 'x'
        font['x' as usize] = [0x00, 0x00, 0xC6, 0x6C, 0x38, 0x6C, 0xC6, 0x00];
        // ' '
        font[' ' as usize] = [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
        // 'G'
        font['G' as usize] = [0x7C, 0xC6, 0xC0, 0xDE, 0xC6, 0xC6, 0x7C, 0x00];
        // 'P'
        font['P' as usize] = [0x7C, 0xC6, 0xC6, 0x7C, 0x30, 0x30, 0x30, 0x00];
        // 'U'
        font['U' as usize] = [0xC6, 0xC6, 0xC6, 0xC6, 0xC6, 0xC6, 0x7C, 0x00];
        // 'T'
        font['T' as usize] = [0x7E, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x00];
        // 'O'
        font['O' as usize] = [0x7C, 0xC6, 0xC6, 0xC6, 0xC6, 0xC6, 0x7C, 0x00];
        // 'K'
        font['K' as usize] = [0xC6, 0xCC, 0xD8, 0xF0, 0xD8, 0xCC, 0xC6, 0x00];
        // '!'
        font['!' as usize] = [0x18, 0x18, 0x18, 0x18, 0x18, 0x00, 0x18, 0x00];
        // 'T'
        font['T' as usize] = [0x7E, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x00];
        // 'E'
        font['E' as usize] = [0x7E, 0x60, 0x60, 0x7C, 0x60, 0x60, 0x7E, 0x00];
        // 'S'
        font['S' as usize] = [0x7C, 0xC6, 0x60, 0x38, 0x0C, 0xC6, 0x7C, 0x00];
        font
    };

    let idx = c as usize;
    if idx >= 128 {
        return;
    }

    let glyph = FONT_8X8[idx];
    for (row, &bits) in glyph.iter().enumerate() {
        for col in 0..8 {
            if (bits >> (7 - col)) & 1 != 0 {
                put_pixel(fb_ptr, width, stride, x + col, y + row as u32, color);
            }
        }
    }
}

/// 绘制字符串
fn draw_string(fb_ptr: *mut u8, width: u32, stride: u32,
               mut x: u32, y: u32, s: &str, color: u32) {
    for c in s.chars() {
        draw_char(fb_ptr, width, stride, x, y, c, color);
        x += 8;
    }
}

/// 测试帧缓冲区绘制
pub fn test_framebuffer() {
    println!("test_framebuffer: Starting framebuffer test...");

    // 获取帧缓冲区信息
    let fb_info = match gpu::get_framebuffer_info() {
        Some(info) => info,
        None => {
            println!("test_framebuffer: No framebuffer available!");
            return;
        }
    };

    println!("test_framebuffer: Framebuffer {}x{}, stride={}, size={}",
        fb_info.width, fb_info.height, fb_info.stride, fb_info.size);

    let fb_ptr = fb_info.addr as *mut u8;
    let width = fb_info.width;
    let height = fb_info.height;
    let stride = fb_info.stride;

    // 清屏 (黑色)
    println!("test_framebuffer: Clearing screen to black...");
    fill_rect(fb_ptr, width, stride, 0, 0, width, height, COLOR_BLACK);

    // 刷新到显示设备
    println!("test_framebuffer: Flushing to display...");
    gpu::flush_framebuffer();

    // 等待一下
    for _ in 0..1000000 {
        core::hint::spin_loop();
    }

    // 绘制彩色条纹 (测试基本绘制功能)
    println!("test_framebuffer: Drawing color bars...");
    let bar_height = height / 8;
    let colors = [COLOR_RED, COLOR_GREEN, COLOR_BLUE, COLOR_YELLOW,
                  COLOR_CYAN, COLOR_MAGENTA, COLOR_GRAY, COLOR_WHITE];

    for (i, &color) in colors.iter().enumerate() {
        let y = (i as u32) * bar_height;
        fill_rect(fb_ptr, width, stride, 0, y, width, bar_height, color);
    }

    // 刷新到显示设备
    gpu::flush_framebuffer();

    // 等待一下
    for _ in 0..2000000 {
        core::hint::spin_loop();
    }

    // 清屏
    fill_rect(fb_ptr, width, stride, 0, 0, width, height, COLOR_BLUE);

    // 绘制标题文字
    println!("test_framebuffer: Drawing text...");
    draw_string(fb_ptr, width, stride, 10, 10, "Rux OS GPU TEST OK!", COLOR_WHITE);

    // 绘制一个白色边框
    let margin = 50u32;
    let border_width = 4u32;

    // 上边框
    fill_rect(fb_ptr, width, stride, margin, margin,
              width - 2 * margin, border_width, COLOR_WHITE);
    // 下边框
    fill_rect(fb_ptr, width, stride, margin, height - margin - border_width,
              width - 2 * margin, border_width, COLOR_WHITE);
    // 左边框
    fill_rect(fb_ptr, width, stride, margin, margin,
              border_width, height - 2 * margin, COLOR_WHITE);
    // 右边框
    fill_rect(fb_ptr, width, stride, width - margin - border_width, margin,
              border_width, height - 2 * margin, COLOR_WHITE);

    // 在中间绘制一个绿色矩形
    let rect_size = 200u32;
    let rect_x = (width - rect_size) / 2;
    let rect_y = (height - rect_size) / 2;
    fill_rect(fb_ptr, width, stride, rect_x, rect_y, rect_size, rect_size, COLOR_GREEN);

    // 在绿色矩形中间绘制一个红色矩形
    let inner_size = 100u32;
    let inner_x = (width - inner_size) / 2;
    let inner_y = (height - inner_size) / 2;
    fill_rect(fb_ptr, width, stride, inner_x, inner_y, inner_size, inner_size, COLOR_RED);

    // 刷新到显示设备
    println!("test_framebuffer: Final flush...");
    gpu::flush_framebuffer();

    println!("test_framebuffer: Test completed! Check the QEMU window.");
}
