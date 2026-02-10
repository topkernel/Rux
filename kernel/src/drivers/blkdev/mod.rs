//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! 块设备驱动层
//!
//! 完全遵循 Linux 内核的块设备设计 (block/blk-core.c, include/linux/blkdev.h)
//!
//! 核心概念：
//! - `struct gendisk`: 块设备表示
//! - `struct block_device`: 块设备实例
//! - `struct request_queue`: 请求队列
//! - `struct bio`: I/O 描述符

use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;
use spin::Mutex;
use core::sync::atomic::{AtomicU32, Ordering};

#[repr(C)]
pub struct BlockDeviceOps {
    /// 打开块设备
    pub open: Option<unsafe fn() -> i32>,
    /// 释放块设备
    pub release: Option<unsafe fn() -> i32>,
    /// 获取几何信息
    pub getgeo: Option<unsafe fn(&mut Geo) -> i32>,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Geo {
    /// 磁头数
    pub heads: u8,
    /// 扇区数
    pub sectors: u8,
    /// 柱面数
    pub cylinders: u16,
    /// 起始位置
    pub start: u32,
}

pub struct GenDisk {
    /// 设备名
    pub name: &'static str,
    /// 主设备号
    pub major: u32,
    /// 第一个次设备号
    pub first_minor: u32,
    /// 次设备号数量
    pub minors: u32,
    /// 容量（以 512 字节扇区为单位）
    pub capacity: AtomicU32,
    /// 块大小
    pub block_size: u32,
    /// 块设备操作
    pub ops: Option<&'static BlockDeviceOps>,
    /// 私有数据
    pub private_data: Option<*mut u8>,
    /// 请求处理函数
    pub request_fn: Option<unsafe extern "C" fn(&mut Request)>,
}

unsafe impl Send for GenDisk {}
unsafe impl Sync for GenDisk {}

impl GenDisk {
    /// 创建新的块设备
    pub fn new(
        name: &'static str,
        major: u32,
        minors: u32,
        block_size: u32,
        ops: Option<&'static BlockDeviceOps>,
    ) -> Self {
        Self {
            name,
            major,
            first_minor: 0,
            minors,
            capacity: AtomicU32::new(0),
            block_size,
            ops,
            private_data: None,
            request_fn: None,
        }
    }

    /// 设置容量
    pub fn set_capacity(&self, sectors: u32) {
        self.capacity.store(sectors, Ordering::Release);
    }

    /// 获取容量
    pub fn get_capacity(&self) -> u32 {
        self.capacity.load(Ordering::Acquire)
    }

    /// 设置私有数据
    pub fn set_private_data(&mut self, data: *mut u8) {
        self.private_data = Some(data);
    }

    /// 设置请求处理函数
    pub fn set_request_fn(&mut self, f: unsafe extern "C" fn(&mut Request)) {
        self.request_fn = Some(f);
    }
}

pub struct Request {
    /// 命令类型
    pub cmd_type: ReqCmd,
    /// 起始扇区
    pub sector: u64,
    /// 数据缓冲区
    pub buffer: Vec<u8>,
    /// 完成回调
    pub end_io: Option<unsafe fn(&Request, i32)>,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ReqCmd {
    /// 读
    Read,
    /// 写
    Write,
    /// 刷新
    Flush,
}

struct BlockDeviceManager {
    /// 块设备列表
    disks: Mutex<Vec<Option<Box<GenDisk>>>>,
    /// 设备号分配器
    major_next: AtomicU32,
}

unsafe impl Send for BlockDeviceManager {}
unsafe impl Sync for BlockDeviceManager {}

impl BlockDeviceManager {
    const fn new() -> Self {
        Self {
            disks: Mutex::new(Vec::new()),
            major_next: AtomicU32::new(1),
        }
    }

    /// 注册块设备
    ///
    /// 对应 Linux 的 add_disk (block/genhd.c)
    pub fn register_disk(&self, disk: Box<GenDisk>) -> Result<(), &'static str> {
        let mut disks = self.disks.lock();

        // 检查设备号是否已使用
        for d in disks.iter() {
            if let Some(ref gd) = d {
                if gd.major == disk.major {
                    return Err("Major number already in use");
                }
            }
        }

        disks.push(Some(disk));
        Ok(())
    }

    /// 查找块设备
    pub fn get_disk(&self, major: u32) -> Option<*const GenDisk> {
        let disks = self.disks.lock();

        for d in disks.iter() {
            if let Some(ref gd) = d {
                if gd.major == major {
                    return Some(gd.as_ref() as *const GenDisk);
                }
            }
        }

        None
    }

    /// 处理 I/O 请求
    pub fn submit_request(&self, disk: *const GenDisk, req: &mut Request) -> i32 {
        unsafe {
            let gd = &*disk;

            if let Some(request_fn) = gd.request_fn {
                request_fn(req);
                0  // Success
            } else {
                -6  // ENXIO
            }
        }
    }
}

static BLOCK_MANAGER: BlockDeviceManager = BlockDeviceManager::new();

pub fn register_disk(disk: Box<GenDisk>) -> Result<(), &'static str> {
    BLOCK_MANAGER.register_disk(disk)
}

pub fn get_disk(major: u32) -> Option<*const GenDisk> {
    BLOCK_MANAGER.get_disk(major)
}

pub fn submit_request(disk: *const GenDisk, req: &mut Request) -> i32 {
    BLOCK_MANAGER.submit_request(disk, req)
}

pub fn blkdev_read(disk: *const GenDisk, sector: u64, buf: &mut [u8]) -> Result<usize, i32> {
    unsafe {
        let gd = &*disk;

        let mut req = Request {
            cmd_type: ReqCmd::Read,
            sector,
            buffer: vec![0u8; buf.len()],
            end_io: None,
        };

        let ret = submit_request(disk, &mut req);
        if ret < 0 {
            return Err(ret);
        }

        // 复制数据
        buf.copy_from_slice(&req.buffer);
        Ok(buf.len())
    }
}

pub fn blkdev_write(disk: *const GenDisk, sector: u64, buf: &[u8]) -> Result<usize, i32> {
    unsafe {
        let gd = &*disk;

        let mut req = Request {
            cmd_type: ReqCmd::Write,
            sector,
            buffer: buf.to_vec(),
            end_io: None,
        };

        let ret = submit_request(disk, &mut req);
        if ret < 0 {
            return Err(ret);
        }

        Ok(buf.len())
    }
}
