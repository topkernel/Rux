//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! ProcFS - 进程信息文件系统
//!
//! 实现类似 Linux 的 /proc 文件系统，提供系统和进程信息
//!
//! 支持的文件：
//! - /proc/meminfo  - 内存信息
//! - /proc/cpuinfo  - CPU 信息
//! - /proc/version  - 内核版本
//! - /proc/uptime   - 系统运行时间
//! - /proc/loadavg  - 系统负载
//! - /proc/cmdline  - 内核启动参数
//! - /proc/self     - 当前进程信息（符号链接）

use alloc::sync::Arc;
use alloc::vec::Vec;
use alloc::string::String;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::fs::superblock::{SuperBlock, SuperBlockFlags, FileSystemType};
use crate::fs::inode::{Inode, InodeMode, Ino};
use crate::fs::mount::{VfsMount, MntFlags};
use crate::println;

/// ProcFS 魔数
const PROCFS_MAGIC: u32 = 0x9fa0;

/// ProcFS 节点类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcFSType {
    /// 目录
    Directory,
    /// 常规文件（动态生成内容）
    RegularFile,
    /// 符号链接
    SymbolicLink,
}

/// 动态内容生成函数类型
type ContentGenerator = fn() -> Vec<u8>;

/// ProcFS 节点
pub struct ProcFSNode {
    /// 节点名称
    pub name: Vec<u8>,
    /// 节点类型
    pub node_type: ProcFSType,
    /// 动态内容生成器（用于常规文件）
    pub content_generator: Option<ContentGenerator>,
    /// 静态内容（如果没有内容生成器）
    pub static_content: Option<Vec<u8>>,
    /// 符号链接目标
    pub link_target: Option<Vec<u8>>,
    /// 子节点（如果是目录）
    pub children: Mutex<Vec<Arc<ProcFSNode>>>,
    /// 引用计数
    ref_count: AtomicU64,
    /// 节点 ID
    pub ino: u64,
}

impl ProcFSNode {
    /// 创建目录节点
    pub fn new_dir(name: Vec<u8>, ino: u64) -> Self {
        Self {
            name,
            node_type: ProcFSType::Directory,
            content_generator: None,
            static_content: None,
            link_target: None,
            children: Mutex::new(Vec::new()),
            ref_count: AtomicU64::new(1),
            ino,
        }
    }

    /// 创建动态内容文件节点
    pub fn new_dynamic_file(name: Vec<u8>, generator: ContentGenerator, ino: u64) -> Self {
        Self {
            name,
            node_type: ProcFSType::RegularFile,
            content_generator: Some(generator),
            static_content: None,
            link_target: None,
            children: Mutex::new(Vec::new()),
            ref_count: AtomicU64::new(1),
            ino,
        }
    }

    /// 创建静态内容文件节点
    pub fn new_static_file(name: Vec<u8>, content: Vec<u8>, ino: u64) -> Self {
        Self {
            name,
            node_type: ProcFSType::RegularFile,
            content_generator: None,
            static_content: Some(content),
            link_target: None,
            children: Mutex::new(Vec::new()),
            ref_count: AtomicU64::new(1),
            ino,
        }
    }

    /// 创建符号链接节点
    pub fn new_symlink(name: Vec<u8>, target: Vec<u8>, ino: u64) -> Self {
        Self {
            name,
            node_type: ProcFSType::SymbolicLink,
            content_generator: None,
            static_content: None,
            link_target: Some(target),
            children: Mutex::new(Vec::new()),
            ref_count: AtomicU64::new(1),
            ino,
        }
    }

    /// 是否是目录
    pub fn is_dir(&self) -> bool {
        self.node_type == ProcFSType::Directory
    }

    /// 是否是常规文件
    pub fn is_file(&self) -> bool {
        self.node_type == ProcFSType::RegularFile
    }

    /// 是否是符号链接
    pub fn is_symlink(&self) -> bool {
        self.node_type == ProcFSType::SymbolicLink
    }

    /// 获取文件内容
    pub fn get_content(&self) -> Vec<u8> {
        if let Some(generator) = self.content_generator {
            generator()
        } else if let Some(ref content) = self.static_content {
            content.clone()
        } else if let Some(ref target) = self.link_target {
            target.clone()
        } else {
            Vec::new()
        }
    }

    /// 获取文件大小
    pub fn size(&self) -> usize {
        self.get_content().len()
    }

    /// 查找子节点
    pub fn find_child(&self, name: &[u8]) -> Option<Arc<ProcFSNode>> {
        let children = self.children.lock();
        for child in children.iter() {
            if child.name.as_slice() == name {
                return Some(child.clone());
            }
        }
        None
    }

    /// 添加子节点
    pub fn add_child(&self, child: Arc<ProcFSNode>) {
        let mut children = self.children.lock();
        children.push(child);
    }

    /// 列出子节点
    pub fn list_children(&self) -> Vec<(Vec<u8>, ProcFSType, u64)> {
        let children = self.children.lock();
        children.iter().map(|c| {
            (c.name.clone(), c.node_type, c.ino)
        }).collect()
    }

    /// 增加引用计数
    pub fn get(&self) {
        self.ref_count.fetch_add(1, Ordering::Relaxed);
    }

    /// 减少引用计数
    pub fn put(&self) -> u64 {
        self.ref_count.fetch_sub(1, Ordering::Relaxed)
    }
}

unsafe impl Send for ProcFSNode {}
unsafe impl Sync for ProcFSNode {}

/// ProcFS 超级块
pub struct ProcFSSuperBlock {
    /// 基础超级块
    pub sb: SuperBlock,
    /// 根节点
    pub root_node: Arc<ProcFSNode>,
    /// 下一个 inode ID
    next_ino: AtomicU64,
}

impl ProcFSSuperBlock {
    /// 创建新的 ProcFS 超级块
    pub fn new() -> Self {
        let sb = SuperBlock::new(4096, PROCFS_MAGIC);

        let root_node = Arc::new(ProcFSNode::new_dir(b"/".to_vec(), 1));

        Self {
            sb,
            root_node,
            next_ino: AtomicU64::new(2),  // 根节点是 1
        }
    }

    /// 分配新的 inode 号
    pub fn alloc_ino(&self) -> u64 {
        self.next_ino.fetch_add(1, Ordering::Relaxed)
    }

    /// 初始化默认文件
    pub fn init_default_files(&self) {
        // 创建 /proc 目录结构
        self.create_dynamic_file("meminfo", generate_meminfo);
        self.create_dynamic_file("cpuinfo", generate_cpuinfo);
        self.create_dynamic_file("version", generate_version);
        self.create_dynamic_file("uptime", generate_uptime);
        self.create_dynamic_file("loadavg", generate_loadavg);
        self.create_static_file("cmdline", generate_cmdline());
        self.create_symlink("self", "/proc/self");

        // 创建 /proc/self 目录（简化实现，指向当前进程信息）
        let self_dir = Arc::new(ProcFSNode::new_dir(b"self".to_vec(), self.alloc_ino()));
        self.root_node.add_child(self_dir.clone());

        // /proc/self/fd 目录
        let fd_ino = self.alloc_ino();
        let fd_dir = Arc::new(ProcFSNode::new_dir(b"fd".to_vec(), fd_ino));
        self_dir.add_child(fd_dir);

        println!("procfs: Default files created");
    }

    /// 创建动态内容文件
    fn create_dynamic_file(&self, name: &str, generator: ContentGenerator) {
        let ino = self.alloc_ino();
        let file = Arc::new(ProcFSNode::new_dynamic_file(
            name.as_bytes().to_vec(),
            generator,
            ino,
        ));
        self.root_node.add_child(file);
    }

    /// 创建静态内容文件
    fn create_static_file(&self, name: &str, content: Vec<u8>) {
        let ino = self.alloc_ino();
        let file = Arc::new(ProcFSNode::new_static_file(
            name.as_bytes().to_vec(),
            content,
            ino,
        ));
        self.root_node.add_child(file);
    }

    /// 创建符号链接
    fn create_symlink(&self, name: &str, target: &str) {
        let ino = self.alloc_ino();
        let link = Arc::new(ProcFSNode::new_symlink(
            name.as_bytes().to_vec(),
            target.as_bytes().to_vec(),
            ino,
        ));
        self.root_node.add_child(link);
    }

    /// 查找文件
    pub fn lookup(&self, path: &str) -> Option<Arc<ProcFSNode>> {
        let components: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

        if components.is_empty() {
            return Some(self.root_node.clone());
        }

        let mut current = self.root_node.clone();
        for component in components {
            match current.find_child(component.as_bytes()) {
                Some(child) => current = child,
                None => return None,
            }
        }
        Some(current)
    }

    /// 读取文件内容
    pub fn read_file(&self, path: &str) -> Option<Vec<u8>> {
        let node = self.lookup(path)?;
        if node.is_file() || node.is_symlink() {
            Some(node.get_content())
        } else {
            None
        }
    }

    /// 列出目录内容
    pub fn list_dir(&self, path: &str) -> Option<Vec<(Vec<u8>, ProcFSType, u64)>> {
        let node = self.lookup(path)?;
        if node.is_dir() {
            Some(node.list_children())
        } else {
            None
        }
    }
}

// ==================== 内容生成函数 ====================

/// 生成 /proc/meminfo 内容
fn generate_meminfo() -> Vec<u8> {
    use crate::mm::meminfo::get_memory_info;

    let info = get_memory_info();
    let mut content = String::new();

    // 转换为 KB
    let mem_total_kb = info.mem_total / 1024;
    let mem_free_kb = info.mem_free / 1024;
    let mem_available_kb = info.mem_available / 1024;
    let mem_used_kb = info.mem_used / 1024;

    content.push_str(&format!("MemTotal:       {} kB\n", mem_total_kb));
    content.push_str(&format!("MemFree:        {} kB\n", mem_free_kb));
    content.push_str(&format!("MemAvailable:   {} kB\n", mem_available_kb));
    content.push_str(&format!("Buffers:               0 kB\n"));
    content.push_str(&format!("Cached:                0 kB\n"));
    content.push_str(&format!("SwapCached:            0 kB\n"));
    content.push_str(&format!("Active:          {} kB\n", mem_used_kb));
    content.push_str(&format!("Inactive:              0 kB\n"));
    content.push_str(&format!("Active(anon):    {} kB\n", mem_used_kb));
    content.push_str(&format!("Inactive(anon):        0 kB\n"));
    content.push_str(&format!("Active(file):          0 kB\n"));
    content.push_str(&format!("Inactive(file):        0 kB\n"));
    content.push_str(&format!("Unevictable:           0 kB\n"));
    content.push_str(&format!("Mlocked:               0 kB\n"));
    content.push_str(&format!("SwapTotal:             0 kB\n"));
    content.push_str(&format!("SwapFree:              0 kB\n"));
    content.push_str(&format!("Dirty:                 0 kB\n"));
    content.push_str(&format!("Writeback:             0 kB\n"));
    content.push_str(&format!("AnonPages:       {} kB\n", mem_used_kb));
    content.push_str(&format!("Mapped:                0 kB\n"));
    content.push_str(&format!("Shmem:                 0 kB\n"));
    content.push_str(&format!("KReclaimable:          0 kB\n"));
    content.push_str(&format!("Slab:                  0 kB\n"));
    content.push_str(&format!("SReclaimable:          0 kB\n"));
    content.push_str(&format!("SUnreclaim:            0 kB\n"));
    content.push_str(&format!("KernelStack:           0 kB\n"));
    content.push_str(&format!("PageTables:            0 kB\n"));
    content.push_str(&format!("NFS_Unstable:          0 kB\n"));
    content.push_str(&format!("Bounce:                0 kB\n"));
    content.push_str(&format!("WritebackTmp:          0 kB\n"));
    content.push_str(&format!("CommitLimit:    {} kB\n", mem_total_kb / 2));
    content.push_str(&format!("Committed_AS:    {} kB\n", mem_used_kb));
    content.push_str(&format!("VmallocTotal:  536870912 kB\n"));  // 512 GB virtual
    content.push_str(&format!("VmallocUsed:           0 kB\n"));
    content.push_str(&format!("VmallocChunk:          0 kB\n"));
    content.push_str(&format!("Percpu:                0 kB\n"));
    content.push_str(&format!("HardwareCorrupted:     0 kB\n"));
    content.push_str(&format!("AnonHugePages:         0 kB\n"));
    content.push_str(&format!("ShmemHugePages:        0 kB\n"));
    content.push_str(&format!("ShmemPmdMapped:        0 kB\n"));
    content.push_str(&format!("FileHugePages:         0 kB\n"));
    content.push_str(&format!("FilePmdMapped:         0 kB\n"));
    content.push_str(&format!("HugePages_Total:       0\n"));
    content.push_str(&format!("HugePages_Free:        0\n"));
    content.push_str(&format!("HugePages_Rsvd:        0\n"));
    content.push_str(&format!("HugePages_Surp:        0\n"));
    content.push_str(&format!("Hugepagesize:       2048 kB\n"));
    content.push_str(&format!("Hugetlb:               0 kB\n"));
    content.push_str(&format!("DirectMap4k:       4096 kB\n"));
    content.push_str(&format!("DirectMap2M:     {} kB\n", mem_total_kb));
    content.push_str(&format!("DirectMap1G:           0 kB\n"));

    content.into_bytes()
}

/// 生成 /proc/cpuinfo 内容
fn generate_cpuinfo() -> Vec<u8> {
    use crate::arch::riscv64::smp::num_started_cpus;
    use core::arch::asm;

    let mut content = String::new();

    let num_cpus = num_started_cpus();

    for cpu in 0..num_cpus {
        // 读取 CPU 信息
        let mvendorid: u64;
        let marchid: u64;
        let mimpid: u64;
        let misa: u64;

        unsafe {
            asm!("csrr {}, mvendorid", out(reg) mvendorid);
            asm!("csrr {}, marchid", out(reg) marchid);
            asm!("csrr {}, mimpid", out(reg) mimpid);
            asm!("csrr {}, misa", out(reg) misa);
        }

        content.push_str(&format!("processor\t: {}\n", cpu));
        content.push_str(&format!("hart\t\t: {}\n", cpu));
        content.push_str(&format!("isa\t\t: rv64imafdch\n"));  // 简化，实际应该解析 misa
        content.push_str(&format!("mmu\t\t: sv39\n"));
        content.push_str(&format!("mvendorid\t: {:#x}\n", mvendorid));
        content.push_str(&format!("marchid\t\t: {:#x}\n", marchid));
        content.push_str(&format!("mimpid\t\t: {:#x}\n", mimpid));

        if cpu < num_cpus - 1 {
            content.push('\n');
        }
    }

    content.into_bytes()
}

/// 生成 /proc/version 内容
fn generate_version() -> Vec<u8> {
    use crate::config::KERNEL_VERSION;

    let rustc_version = option_env!("RUSTC_VERSION").unwrap_or("unknown");
    let build_time = option_env!("BUILD_TIME").unwrap_or("unknown");

    let content = format!(
        "Rux OS version {} (riscv64)\n\
         Compiled with Rust {} at {}\n\
         Copyright (c) 2026 Fei Wang\n",
        KERNEL_VERSION,
        rustc_version,
        build_time
    );

    content.into_bytes()
}

/// 生成 /proc/uptime 内容
fn generate_uptime() -> Vec<u8> {
    // QEMU virt 机器的时钟频率是 10 MHz
    const TIMER_FREQ: u64 = 10_000_000;

    // 读取当前时间（cycles）
    let cycles: u64;
    unsafe {
        core::arch::asm!(
            "rdtime {}",
            out(reg) cycles,
            options(nostack, readonly)
        );
    }

    // 转换为秒
    let uptime_secs = cycles / TIMER_FREQ;

    let content = format!("{}.00 {}.00\n", uptime_secs, uptime_secs);

    content.into_bytes()
}

/// 生成 /proc/loadavg 内容
fn generate_loadavg() -> Vec<u8> {
    // 简化实现：返回 0 负载
    // TODO: 实现真正的负载计算
    b"0.00 0.00 0.00 1/64 0\n".to_vec()
}

/// 生成 /proc/cmdline 内容
fn generate_cmdline() -> Vec<u8> {
    use crate::cmdline;

    match cmdline::get_cmdline() {
        Some(bootargs) if !bootargs.is_empty() => {
            format!("{}\n", bootargs).into_bytes()
        }
        _ => b"BOOT_IMAGE=/boot/rux\n".to_vec()
    }
}

// ==================== 文件系统类型注册 ====================

/// ProcFS 文件系统类型
pub static PROCFS_FS_TYPE: FileSystemType = FileSystemType::new(
    "proc",
    Some(procfs_mount),
    Some(procfs_kill_sb),
    0,
);

/// 全局 ProcFS 超级块指针
static GLOBAL_PROCFS_SB: core::sync::atomic::AtomicPtr<ProcFSSuperBlock> =
    core::sync::atomic::AtomicPtr::new(core::ptr::null_mut());

/// 全局 ProcFS 挂载点指针
static GLOBAL_PROC_MOUNT: core::sync::atomic::AtomicPtr<VfsMount> =
    core::sync::atomic::AtomicPtr::new(core::ptr::null_mut());

/// ProcFS 挂载函数
unsafe extern "C" fn procfs_mount(_fs_context: &crate::fs::superblock::FsContext<'_>) -> Result<*mut SuperBlock, i32> {
    let procfs_sb = alloc::boxed::Box::new(ProcFSSuperBlock::new());
    let procfs_sb_ptr = alloc::boxed::Box::into_raw(procfs_sb) as *mut SuperBlock;
    Ok(procfs_sb_ptr)
}

/// ProcFS 卸载函数
unsafe extern "C" fn procfs_kill_sb(sb: *mut SuperBlock) {
    if !sb.is_null() {
        let _ = alloc::boxed::Box::from_raw(sb as *mut ProcFSSuperBlock);
    }
}

/// 获取 ProcFS 超级块
pub fn get_procfs_sb() -> Option<&'static ProcFSSuperBlock> {
    let ptr = GLOBAL_PROCFS_SB.load(Ordering::Acquire);
    if ptr.is_null() {
        None
    } else {
        Some(unsafe { &*ptr })
    }
}

/// 从 /proc 读取文件
pub fn read_file(path: &str) -> Option<Vec<u8>> {
    get_procfs_sb()?.read_file(path)
}

/// 列出 /proc 目录
pub fn list_dir(path: &str) -> Option<Vec<(Vec<u8>, ProcFSType, u64)>> {
    get_procfs_sb()?.list_dir(path)
}

/// 初始化 ProcFS
pub fn init_procfs() -> Result<(), i32> {
    use crate::fs::superblock::register_filesystem;

    println!("procfs: Initializing ProcFS...");

    // 1. 注册文件系统类型
    register_filesystem(&PROCFS_FS_TYPE)?;
    println!("procfs: File system type registered");

    // 2. 创建超级块
    let procfs_sb = alloc::boxed::Box::new(ProcFSSuperBlock::new());
    let procfs_sb_ptr = alloc::boxed::Box::into_raw(procfs_sb) as *mut ProcFSSuperBlock;

    // 3. 初始化默认文件
    unsafe {
        (*procfs_sb_ptr).init_default_files();
    }

    // 4. 存储全局指针
    GLOBAL_PROCFS_SB.store(procfs_sb_ptr, Ordering::Release);

    println!("procfs: ProcFS initialized [OK]");

    Ok(())
}

/// 挂载 ProcFS 到 /proc
pub fn mount_procfs() -> Result<(), i32> {
    // 获取 RootFS 超级块
    let rootfs_sb = match crate::fs::rootfs::get_rootfs_sb() {
        Some(sb) => sb,
        None => {
            println!("procfs: RootFS not initialized");
            return Err(-1);
        }
    };

    // 在 RootFS 中创建 /proc 目录
    unsafe {
        (*rootfs_sb).create_dir("/proc", 0o755)?;
    }
    println!("procfs: /proc directory created");

    // 创建挂载点
    let procfs_sb_ptr = GLOBAL_PROCFS_SB.load(Ordering::Acquire);
    if procfs_sb_ptr.is_null() {
        println!("procfs: ProcFS superblock not found");
        return Err(-1);
    }

    let mount = alloc::boxed::Box::new(VfsMount::new(
        b"/proc".to_vec(),
        b"/proc".to_vec(),
        MntFlags::new(0),
        Some(procfs_sb_ptr as *mut u8),
    ));
    let mount_ptr = alloc::boxed::Box::into_raw(mount) as *mut VfsMount;
    GLOBAL_PROC_MOUNT.store(mount_ptr, Ordering::Release);

    println!("procfs: Mounted at /proc [OK]");

    Ok(())
}
