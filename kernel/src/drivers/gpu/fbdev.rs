//! Framebuffer 字符设备 (/dev/fb0)
//!
//! 实现 Linux 兼容的 framebuffer 设备接口
//! 参考: linux/include/uapi/linux/fb.h

use super::FrameBufferInfo;

/// ioctl 命令码
/// 获取可变屏幕信息
pub const FBIOGET_VSCREENINFO: u32 = 0x4600;
/// 获取固定屏幕信息
pub const FBIOGET_FSCREENINFO: u32 = 0x4602;

/// Framebuffer 类型
pub const FB_TYPE_PACKED_PIXELS: u32 = 0;

/// Framebuffer 视觉类型
pub const FB_VISUAL_TRUECOLOR: u32 = 2;

/// 颜色位域
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct FbBitfield {
    /// 偏移量（从最低位开始）
    pub offset: u32,
    /// 位数
    pub length: u32,
    /// MSB 优先
    pub msb_right: u32,
}

/// 固定屏幕信息
/// 对应 Linux 的 fb_fix_screeninfo
#[repr(C)]
#[derive(Clone, Copy)]
pub struct FbFixScreeninfo {
    /// 驱动名称 (16 bytes)
    pub id: [u8; 16],
    /// 物理内存起始地址
    pub smem_start: u64,
    /// 物理内存长度
    pub smem_len: u32,
    /// Framebuffer 类型
    pub type_: u32,
    /// 视觉类型
    pub visual: u32,
    /// 行长度（字节）
    pub line_length: u32,
    /// MMIIO 起始地址
    pub mmio_start: u64,
    /// MMIIO 长度
    pub mmio_len: u32,
    /// 加速类型
    pub accel: u32,
    /// 性能信息标志
    pub capabilities: u16,
    /// 保留
    pub reserved: [u16; 2],
}

impl Default for FbFixScreeninfo {
    fn default() -> Self {
        Self {
            id: [0; 16],
            smem_start: 0,
            smem_len: 0,
            type_: FB_TYPE_PACKED_PIXELS,
            visual: FB_VISUAL_TRUECOLOR,
            line_length: 0,
            mmio_start: 0,
            mmio_len: 0,
            accel: 0,
            capabilities: 0,
            reserved: [0; 2],
        }
    }
}

/// 可变屏幕信息
/// 对应 Linux 的 fb_var_screeninfo
#[repr(C)]
#[derive(Clone, Copy)]
pub struct FbVarScreeninfo {
    /// 可见分辨率
    pub xres: u32,
    pub yres: u32,
    /// 虚拟分辨率
    pub xres_virtual: u32,
    pub yres_virtual: u32,
    /// 从虚拟到可见的偏移
    pub xoffset: u32,
    pub yoffset: u32,
    /// 每像素位数
    pub bits_per_pixel: u32,
    /// 灰度级别 (0 = 彩色)
    pub grayscale: u32,
    /// 红色位域
    pub red: FbBitfield,
    /// 绿色位域
    pub green: FbBitfield,
    /// 蓝色位域
    pub blue: FbBitfield,
    /// 透明度位域
    pub transp: FbBitfield,
    /// 非交错模式
    pub nonstd: u32,
    /// 激活标志
    pub activate: u32,
    /// 显示高度 (mm)
    pub height: u32,
    /// 显示宽度 (mm)
    pub width: u32,
    /// 时序标志
    pub accel_flags: u32,
    /// 像素时钟 (ps)
    pub pixclock: u32,
    /// 时序参数
    pub left_margin: u32,
    pub right_margin: u32,
    pub upper_margin: u32,
    pub lower_margin: u32,
    pub hsync_len: u32,
    pub vsync_len: u32,
    /// 同步标志
    pub sync: u32,
    /// 视频模式
    pub vmode: u32,
    /// 旋转角度
    pub rotate: u32,
    /// 颜色空间
    pub colorspace: u32,
    /// 保留
    pub reserved: [u32; 4],
}

impl Default for FbVarScreeninfo {
    fn default() -> Self {
        Self {
            xres: 0,
            yres: 0,
            xres_virtual: 0,
            yres_virtual: 0,
            xoffset: 0,
            yoffset: 0,
            bits_per_pixel: 32,
            grayscale: 0,
            red: FbBitfield { offset: 16, length: 8, msb_right: 0 },
            green: FbBitfield { offset: 8, length: 8, msb_right: 0 },
            blue: FbBitfield { offset: 0, length: 8, msb_right: 0 },
            transp: FbBitfield { offset: 24, length: 8, msb_right: 0 },
            nonstd: 0,
            activate: 0,
            height: 0,
            width: 0,
            accel_flags: 0,
            pixclock: 0,
            left_margin: 0,
            right_margin: 0,
            upper_margin: 0,
            lower_margin: 0,
            hsync_len: 0,
            vsync_len: 0,
            sync: 0,
            vmode: 0,
            rotate: 0,
            colorspace: 0,
            reserved: [0; 4],
        }
    }
}

/// 从 FrameBufferInfo 创建 FbFixScreeninfo
pub fn create_fix_screeninfo(info: &FrameBufferInfo) -> FbFixScreeninfo {
    let mut fix = FbFixScreeninfo::default();

    // 设置驱动名称
    let name = b"virtio-gpu\0";
    let len = name.len().min(16);
    fix.id[..len].copy_from_slice(&name[..len]);

    fix.smem_start = info.addr;
    fix.smem_len = info.size;
    fix.line_length = info.stride * 4; // stride 是像素数，每像素 4 字节

    fix
}

/// 从 FrameBufferInfo 创建 FbVarScreeninfo
pub fn create_var_screeninfo(info: &FrameBufferInfo) -> FbVarScreeninfo {
    let mut var = FbVarScreeninfo::default();

    var.xres = info.width;
    var.yres = info.height;
    var.xres_virtual = info.width;
    var.yres_virtual = info.height;
    var.bits_per_pixel = 32;

    // xRGB 格式 (little-endian)
    var.red = FbBitfield { offset: 16, length: 8, msb_right: 0 };
    var.green = FbBitfield { offset: 8, length: 8, msb_right: 0 };
    var.blue = FbBitfield { offset: 0, length: 8, msb_right: 0 };
    var.transp = FbBitfield { offset: 24, length: 8, msb_right: 0 };

    var
}

/// 处理 framebuffer ioctl 命令
/// 返回: 成功返回 0，失败返回负错误码
pub fn fbdev_ioctl(cmd: u32, arg: usize) -> i64 {
    let info = match super::get_framebuffer_info() {
        Some(info) => info,
        None => return -6, // ENXIO: 设备不存在
    };

    match cmd {
        FBIOGET_FSCREENINFO => {
            let fix = create_fix_screeninfo(&info);
            unsafe {
                // 将结构体复制到用户空间
                let dest = arg as *mut FbFixScreeninfo;
                core::ptr::write_volatile(dest, fix);
            }
            0
        }
        FBIOGET_VSCREENINFO => {
            let var = create_var_screeninfo(&info);
            unsafe {
                let dest = arg as *mut FbVarScreeninfo;
                core::ptr::write_volatile(dest, var);
            }
            0
        }
        _ => -25, // ENOTTY: 不支持的 ioctl 命令
    }
}
