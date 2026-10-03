//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! Framebuffer character device (/dev/fb0)
//!
//! Implements framebuffer device interface

use super::FrameBufferInfo;

use core::sync::atomic::{AtomicBool, Ordering};

/// Set once userspace has mmap'ed /dev/fb0 (Xorg's fbdev driver maps the
/// scanout and then NEVER issues FBIO_FLUSH — virtio-gpu needs an explicit
/// TRANSFER_TO_HOST_2D + RESOURCE_FLUSH to show the pixels). The
/// kfbflush thread keys off this flag so idle systems pay nothing.
static FB_USER_MAPPED: AtomicBool = AtomicBool::new(false);

/// Record that a userspace process mapped /dev/fb0 (called from the fbdev
/// mmap path in syscall::memory).
pub fn mark_fb_user_mapped() {
    FB_USER_MAPPED.store(true, Ordering::Release);
}

/// kfbflush: periodically push the framebuffer to the virtio-gpu scanout.
///
/// Rux's virtio-gpu framebuffer is guest memory: user writes via the /dev/fb0
/// mapping land in RAM, but the host only re-reads them after a flush. Native
/// Rux apps call FBIO_FLUSH themselves; stock Xorg (xf86-video-fbdev) never
/// does — without a periodic flush its output would stay invisible forever.
extern "C" fn kfbflush_fn(_arg: *mut core::ffi::c_void) -> i32 {
    use crate::process::task::TaskState;

    // 3 jiffies = 30 ms @ KERNEL_HZ=100 (≈33 fps presentation).
    let interval_jiffies: u64 = 3;

    loop {
        if crate::process::kthread::kthread_should_stop() {
            break;
        }

        // --- Periodic sleep with a real waker (khungtaskd discipline). ---
        let current = match crate::sched::current() {
            Some(t) => t as *mut crate::process::task::Task,
            None => break,
        };
        // SAFETY: current is this kthread's own task pointer.
        let my_pid = unsafe { (*current).pid() };
        let target = crate::drivers::timer::get_jiffies() + interval_jiffies;
        let timer_id = crate::timer::add_timer_wakeup(target, my_pid);

        if timer_id != 0 {
            // SAFETY: current is the running task's pointer.
            unsafe {
                (*current).set_state(TaskState::new(TaskState::INTERRUPTIBLE));
            }
            if crate::drivers::timer::get_jiffies() >= target {
                // SAFETY: current is the running task's pointer.
                unsafe {
                    (*current).set_state(TaskState::new(TaskState::RUNNING));
                    crate::sched::dequeue_task(&*current);
                }
                crate::timer::del_timer(timer_id);
            } else {
                crate::arch::riscv64::cpu::restore_irq(true);
                crate::sched::schedule();
                if crate::timer::timer_pending(timer_id) {
                    crate::timer::del_timer(timer_id);
                }
            }
        } else {
            crate::sched::schedule();
        }

        if FB_USER_MAPPED.load(Ordering::Acquire) {
            fb_canary();
            super::flush_framebuffer();
        }
    }
    0
}

// ---------------------------------------------------------------------------
// FB canary (fb0-corruption hunt): sample the first words of each 2MB half
// of the scanout buffer every flush tick. Once a half HAS held content, it
// must never read back all-zero — the fb is not anonymous memory, nothing
// legitimately clears it. On a nonzero->zero transition dump the buddy
// state of the fb block plus the recent big-block alloc/free ring.
// ---------------------------------------------------------------------------
static CANARY_LO_NZ: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
static CANARY_HI_NZ: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
static CANARY_FIRED: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

fn fb_canary() {
    use crate::mm::buddy_allocator as buddy;
    let lo = buddy::FB_GUARD_LO.load(Ordering::Relaxed);
    let hi = buddy::FB_GUARD_HI.load(Ordering::Relaxed);
    if lo == 0 || hi <= lo {
        return;
    }
    let half = (hi - lo) / 2;
    // 256 samples per half, spread over the first 1MB of each half.
    let nz = |base: usize| -> usize {
        let mut n = 0;
        for k in 0..256 {
            let p = (base + k * 4096) as *const u64;
            let v = unsafe { core::ptr::read_volatile(p) };
            if v != 0 {
                n += 1;
            }
        }
        n
    };
    let lo_nz = nz(lo);
    let hi_nz = nz(lo + half);
    let prev_lo = CANARY_LO_NZ.load(Ordering::Relaxed);
    let prev_hi = CANARY_HI_NZ.load(Ordering::Relaxed);
    CANARY_LO_NZ.store(lo_nz, Ordering::Relaxed);
    CANARY_HI_NZ.store(hi_nz, Ordering::Relaxed);
    let trip = (prev_lo != 0 && lo_nz == 0) || (prev_hi != 0 && hi_nz == 0);
    if !trip || CANARY_FIRED.swap(1, Ordering::Relaxed) != 0 {
        return;
    }
    let (m_lead_free, m_lead_order) = buddy::block_meta_at(lo);
    let (m_mid_free, m_mid_order) = buddy::block_meta_at(lo + half);
    crate::pr_err!(
        "FBCANARY: fb content vanished! lo_nz {}->{} hi_nz {}->{} meta(lead free={} order={} mid free={} order={})",
        prev_lo, lo_nz, prev_hi, hi_nz,
        m_lead_free, m_lead_order, m_mid_free, m_mid_order
    );
    // Dump the recent big-block ring (last events first seen is fine).
    let cur = buddy::bb_ev_cur_load();
    crate::pr_err!("FBCANARY: last {} big-block events (cur={}):", buddy::BB_RING, cur);
    for k in (0..buddy::BB_RING).rev() {
        let i = (cur + buddy::BB_RING - 1 - k) % buddy::BB_RING;
        let ptr = buddy::BB_EV_PTR[i].load(Ordering::Relaxed);
        let op = buddy::BB_EV_OP[i].load(Ordering::Relaxed);
        if ptr == 0 {
            continue;
        }
        let is_alloc = op >> 63;
        let order = (op >> 56) & 0x7F;
        let jiffies = op & 0xFFFF_FFFF_FFFF;
        crate::pr_err!(
            "FBBIG[{}] {} ptr={:#x} order={} jiffies={}",
            i, if is_alloc == 1 { "ALLOC" } else { "FREE " }, ptr, order, jiffies
        );
    }
}

/// ioctl command codes
/// Get variable screen information
pub const FBIOGET_VSCREENINFO: u32 = 0x4600;
/// Set variable screen information
pub const FBIOPUT_VSCREENINFO: u32 = 0x4601;
/// Get fixed screen information
pub const FBIOGET_FSCREENINFO: u32 = 0x4602;
/// Pan/display offset control
pub const FBIOPAN_DISPLAY: u32 = 0x4604;
/// Flush framebuffer (VirtIO-GPU specific)
/// VirtIO-GPU requires explicit flush to display updated content
pub const FBIO_FLUSH: u32 = 0x4610;
/// Blank/unblank the screen
pub const FBIOBLANK: u32 = 0x4611;

/// Framebuffer type
pub const FB_TYPE_PACKED_PIXELS: u32 = 0;

/// Framebuffer visual type
pub const FB_VISUAL_TRUECOLOR: u32 = 2;

/// Color bitfield
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct FbBitfield {
    /// Offset (from LSB)
    pub offset: u32,
    /// Number of bits
    pub length: u32,
    /// MSB first
    pub msb_right: u32,
}

/// Fixed screen information
#[derive(Clone, Copy)]
/// Framebuffer fixed information — layout matches Linux
/// `struct fb_fix_screeninfo` byte-for-byte (review BUG:
/// "FbFixScreeninfo 布局错位"): type_aux and the three pan steps were
/// missing, shifting visual/line_length/mmio_* by 8+ bytes, so user fb
/// programs (fbset, SDL FB_DEV) read garbage strides and MMIO offsets.
///
/// Offsets: id 0, smem_start 16, smem_len 24, type 28, type_aux 32,
/// visual 36, xpanstep 40, ypanstep 42, ywrapstep 44, (pad 46),
/// line_length 48, (pad 52), mmio_start 56, mmio_len 64, accel 68,
/// capabilities 72, reserved 74; sizeof = 80.
#[repr(C)]
pub struct FbFixScreeninfo {
    /// Driver name (16 bytes)
    pub id: [u8; 16],
    /// Physical memory start address
    pub smem_start: u64,
    /// Physical memory length
    pub smem_len: u32,
    /// Framebuffer type
    pub type_: u32,
    /// Interleave for interleaved framebuffers
    pub type_aux: u32,
    /// Visual type
    pub visual: u32,
    /// Zero if no hardware panning
    pub xpanstep: u16,
    pub ypanstep: u16,
    pub ywrapstep: u16,
    /// Line length (bytes)
    pub line_length: u32,
    /// MMIO start address
    pub mmio_start: u64,
    /// MMIO length
    pub mmio_len: u32,
    /// Acceleration type
    pub accel: u32,
    /// Performance info flags
    pub capabilities: u16,
    /// Reserved
    pub reserved: [u16; 2],
}

impl Default for FbFixScreeninfo {
    fn default() -> Self {
        Self {
            id: [0; 16],
            smem_start: 0,
            smem_len: 0,
            type_: FB_TYPE_PACKED_PIXELS,
            type_aux: 0,
            visual: FB_VISUAL_TRUECOLOR,
            xpanstep: 0,
            ypanstep: 0,
            ywrapstep: 0,
            line_length: 0,
            mmio_start: 0,
            mmio_len: 0,
            accel: 0,
            capabilities: 0,
            reserved: [0; 2],
        }
    }
}

/// Variable screen information
#[repr(C)]
#[derive(Clone, Copy)]
pub struct FbVarScreeninfo {
    /// Visible resolution
    pub xres: u32,
    pub yres: u32,
    /// Virtual resolution
    pub xres_virtual: u32,
    pub yres_virtual: u32,
    /// Offset from virtual to visible
    pub xoffset: u32,
    pub yoffset: u32,
    /// Bits per pixel
    pub bits_per_pixel: u32,
    /// Grayscale levels (0 = color)
    pub grayscale: u32,
    /// Red bitfield
    pub red: FbBitfield,
    /// Green bitfield
    pub green: FbBitfield,
    /// Blue bitfield
    pub blue: FbBitfield,
    /// Transparency bitfield
    pub transp: FbBitfield,
    /// Non-standard mode
    pub nonstd: u32,
    /// Activation flags
    pub activate: u32,
    /// Display height (mm)
    pub height: u32,
    /// Display width (mm)
    pub width: u32,
    /// Timing flags
    pub accel_flags: u32,
    /// Pixel clock (ps)
    pub pixclock: u32,
    /// Timing parameters
    pub left_margin: u32,
    pub right_margin: u32,
    pub upper_margin: u32,
    pub lower_margin: u32,
    pub hsync_len: u32,
    pub vsync_len: u32,
    /// Sync flags
    pub sync: u32,
    /// Video mode
    pub vmode: u32,
    /// Rotation angle
    pub rotate: u32,
    /// Color space
    pub colorspace: u32,
    /// Reserved
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

/// Create FbFixScreeninfo from FrameBufferInfo
pub fn create_fix_screeninfo(info: &FrameBufferInfo) -> FbFixScreeninfo {
    let mut fix = FbFixScreeninfo::default();

    // Set driver name
    let name = b"virtio-gpu\0";
    let len = name.len().min(16);
    fix.id[..len].copy_from_slice(&name[..len]);

    fix.smem_start = info.addr;
    fix.smem_len = info.size;
    fix.line_length = info.stride; // stride is already in bytes

    fix
}

/// Create FbVarScreeninfo from FrameBufferInfo
pub fn create_var_screeninfo(info: &FrameBufferInfo) -> FbVarScreeninfo {
    let mut var = FbVarScreeninfo::default();

    var.xres = info.width;
    var.yres = info.height;
    var.xres_virtual = info.width;
    var.yres_virtual = info.height;
    var.bits_per_pixel = 32;

    // Bitfields measured empirically against QEMU's virtio-gpu (resource
    // created with format 3): on-screen R comes from V[15:8], G from
    // V[23:16], B from V[31:24]; V[7:0] is ignored. Advertising these
    // offsets lets generic fbdev programs build correct pixels.
    var.red = FbBitfield { offset: 8, length: 8, msb_right: 0 };
    var.green = FbBitfield { offset: 16, length: 8, msb_right: 0 };
    var.blue = FbBitfield { offset: 24, length: 8, msb_right: 0 };
    var.transp = FbBitfield { offset: 0, length: 0, msb_right: 0 };

    var
}

/// /dev/fb0 file operations: a real char device so ioctls and mmap
/// dispatch on the FILE's ops identity (no fd-number heuristics).
pub static FB_OPS: crate::fs::FileOps = crate::fs::FileOps {
    read: Some(fb_read),
    write: Some(fb_write),
    lseek: None,
    close: None,
    poll: None,
};

fn fb_read(_file: &crate::fs::File, _buf: &mut [u8]) -> isize {
    -29 // ESPIPE-like: reads unsupported (mmap instead)
}

fn fb_write(_file: &crate::fs::File, _buf: &[u8]) -> isize {
    -29
}

/// True when the File is the /dev/fb0 char device.
pub fn is_fb_file(file: &crate::fs::File) -> bool {
    match file.get_ops() {
        Some(ops) => core::ptr::eq(ops as *const _, &FB_OPS as *const _),
        None => false,
    }
}

/// Register the fb char device and create /dev/fb0 (called from boot
/// after devfs is up and the GPU framebuffer exists).
pub fn init_fbdev() -> Result<(), ()> {
    crate::fs::devfs::registry::register_char_device(
        crate::fs::dev_t::DEV_FB0,
        &FB_OPS,
    )?;
    crate::fs::devfs::mknod("/fb0", crate::fs::dev_t::DEV_FB0, 0o666)?;

    // Auto-flush presenter for flush-less renderers (stock Xorg).
    let started = crate::process::kthread::kthread_run(
        kfbflush_fn,
        core::ptr::null_mut(),
        "kfbflush",
    )
    .is_some();
    if !started {
        crate::pr_warn!("fbdev: failed to start kfbflush (auto-flush off; only FBIO_FLUSH apps will display)");
    }

    Ok(())
}

/// FBIOPUTCMAP (Linux 0x4605): the color map is meaningless on a
/// truecolor scanout, but Xorg's fbdev driver programs its gamma ramp
/// through it during screen setup and hits this constantly — answering
/// ENOTTY produced an (EE) storm in Xorg.0.log and slowed setup under
/// TCG. Accept the write and drop it (Linux fbdev accepts cmap ioctls
/// on truecolor frames by ignoring the content).
const FBIOPUTCMAP: u32 = 0x4605;

/// Handle framebuffer ioctl commands
/// Returns: 0 on success, negative error code on failure
pub fn fbdev_ioctl(cmd: u32, arg: usize) -> i64 {
    let info = match super::get_framebuffer_info() {
        Some(info) => info,
        None => return -6, // ENXIO: device does not exist
    };

    match cmd {
        FBIOGET_FSCREENINFO => {
            let fix = create_fix_screeninfo(&info);
            if !crate::arch::riscv64::uaccess::access_ok(arg, core::mem::size_of::<FbFixScreeninfo>()) {
                return -14; // EFAULT
            }
            // SAFETY: access_ok validated the user pointer; fix is a properly initialized value.
            unsafe {
                let uncopied = crate::arch::riscv64::uaccess::copy_to_user(
                    arg as *mut u8,
                    &fix as *const FbFixScreeninfo as *const u8,
                    core::mem::size_of::<FbFixScreeninfo>(),
                );
                if uncopied != 0 { return -14; }
            }
            0
        }
        FBIOGET_VSCREENINFO => {
            let var = create_var_screeninfo(&info);
            if !crate::arch::riscv64::uaccess::access_ok(arg, core::mem::size_of::<FbVarScreeninfo>()) {
                return -14; // EFAULT
            }
            // SAFETY: access_ok validated the user pointer; var is a properly initialized value.
            unsafe {
                let uncopied = crate::arch::riscv64::uaccess::copy_to_user(
                    arg as *mut u8,
                    &var as *const FbVarScreeninfo as *const u8,
                    core::mem::size_of::<FbVarScreeninfo>(),
                );
                if uncopied != 0 { return -14; }
            }
            0
        }
        FBIOPUT_VSCREENINFO => {
            // Xorg's fbdevHWSetMode programs the "current" (builtin) mode
            // via FBIOPUT_VSCREENINFO and requires the ioctl to succeed
            // AND copy back a var equal to what it requested (it compares
            // set_var == req_var, "FBIOPUT_VSCREENINFO succeeded but
            // modified mode" otherwise). The scanout geometry cannot be
            // reprogrammed (QEMU virtio-gpu fixed mode): accept the
            // current geometry/format verbatim (echo the request back),
            // reject anything that would change it.
            if !crate::arch::riscv64::uaccess::access_ok(arg, core::mem::size_of::<FbVarScreeninfo>()) {
                return -14; // EFAULT
            }
            let mut req = FbVarScreeninfo::default();
            // SAFETY: access_ok validated the user pointer; req is a local.
            unsafe {
                let uncopied = crate::arch::riscv64::uaccess::copy_from_user(
                    &mut req as *mut FbVarScreeninfo as *mut u8,
                    arg as *const u8,
                    core::mem::size_of::<FbVarScreeninfo>(),
                );
                if uncopied != 0 { return -14; }
            }
            let cur = create_var_screeninfo(&info);
            let same_geometry = req.xres == cur.xres
                && req.yres == cur.yres
                && req.xres_virtual == cur.xres_virtual
                && req.yres_virtual == cur.yres_virtual
                && req.bits_per_pixel == cur.bits_per_pixel
                && req.red.offset == cur.red.offset
                && req.red.length == cur.red.length
                && req.green.offset == cur.green.offset
                && req.green.length == cur.green.length
                && req.blue.offset == cur.blue.offset
                && req.blue.length == cur.blue.length;
            if !same_geometry {
                return -22; // EINVAL: mode change not supported
            }
            // Echo the accepted request back verbatim.
            // SAFETY: access_ok validated the user pointer.
            unsafe {
                let uncopied = crate::arch::riscv64::uaccess::copy_to_user(
                    arg as *mut u8,
                    &req as *const FbVarScreeninfo as *const u8,
                    core::mem::size_of::<FbVarScreeninfo>(),
                );
                if uncopied != 0 { return -14; }
            }
            0
        }
        FBIOPAN_DISPLAY => {
            // No hardware panning (single scanout); accept the ioctl and
            // report the only pannable offset (0,0). Xorg's
            // fbdevHWAdjustFrame only warns on failure, but echoing back
            // keeps the log clean.
            if !crate::arch::riscv64::uaccess::access_ok(arg, core::mem::size_of::<FbVarScreeninfo>()) {
                return -14; // EFAULT
            }
            let mut var = create_var_screeninfo(&info);
            var.xoffset = 0;
            var.yoffset = 0;
            // SAFETY: access_ok validated the user pointer.
            unsafe {
                let uncopied = crate::arch::riscv64::uaccess::copy_to_user(
                    arg as *mut u8,
                    &var as *const FbVarScreeninfo as *const u8,
                    core::mem::size_of::<FbVarScreeninfo>(),
                );
                if uncopied != 0 { return -14; }
            }
            0
        }
        FBIOBLANK => {
            // Screen blanking is not implemented (single fixed scanout);
            // DPMS-blank requests are accepted as no-ops. Xorg's
            // fbdevHWSaveScreen downgrades to a warning on failure, but
            // success keeps fbdevHW from disabling its blank path.
            0
        }
        FBIO_FLUSH => {
            // Flush framebuffer to display device
            // VirtIO-GPU requires explicit flush to display updated content
            if super::flush_framebuffer() {
                0
            } else {
                -6 // ENXIO: device does not exist
            }
        }
        FBIOPUTCMAP => {
            // fb_cmap is {start, len, red*, green*, blue*, transp*} = 4+4+6*8
            // bytes with pointer-ABI padding — only the header is validated;
            // the palette itself is dropped (see the const's doc comment).
            if !crate::arch::riscv64::uaccess::access_ok(arg, 48) {
                return -14; // EFAULT
            }
            0
        }
        _ => -25, // ENOTTY: unsupported ioctl command
    }
}
