//! 信号处理机制
//!
//! 完全遵循 Linux 内核的信号设计 (kernel/signal.c, include/linux/signal.h)
//!
//! 核心概念：
//! - `struct signal_struct`: 信号处理描述符
//! - `struct sigpending`: 待处理信号队列
//! - `struct sigaction`: 信号处理动作
//! - 信号发送 (kill) 和处理 (do_signal)

use core::sync::atomic::{AtomicU64, Ordering};
use alloc::boxed::Box;
use alloc::vec::Vec;

/// 信号编号类型
pub type SigType = i32;

/// 标准信号定义 (1-31)
///
/// 对应 Linux 的 signal 定义 (include/uapi/asm-generic/signal.h)
#[repr(i32)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum Signal {
    /// SIGHUP - 挂起
    SIGHUP = 1,
    /// SIGINT - 中断 (Ctrl+C)
    SIGINT = 2,
    /// SIGQUIT - 退出
    SIGQUIT = 3,
    /// SIGILL - 非法指令
    SIGILL = 4,
    /// SIGTRAP - 断点陷阱
    SIGTRAP = 5,
    /// SIGABRT - 异常终止
    SIGABRT = 6,
    /// SIGBUS - 总线错误
    SIGBUS = 7,
    /// SIGFPE - 浮点异常
    SIGFPE = 8,
    /// SIGKILL - 强制杀死 (不可捕获/忽略)
    SIGKILL = 9,
    /// SIGUSR1 - 用户定义信号1
    SIGUSR1 = 10,
    /// SIGSEGV - 段错误
    SIGSEGV = 11,
    /// SIGUSR2 - 用户定义信号2
    SIGUSR2 = 12,
    /// SIGPIPE - 管道破裂
    SIGPIPE = 13,
    /// SIGALRM - 定时器
    SIGALRM = 14,
    /// SIGTERM - 终止
    SIGTERM = 15,
    /// SIGSTKFLT - 栈错误
    SIGSTKFLT = 16,
    /// SIGCHLD - 子进程状态改变
    SIGCHLD = 17,
    /// SIGCONT - 继续
    SIGCONT = 18,
    /// SIGSTOP - 停止 (不可捕获/忽略)
    SIGSTOP = 19,
    /// SIGTSTP - 终端停止 (Ctrl+Z)
    SIGTSTP = 20,
    /// SIGTTIN - 后台读
    SIGTTIN = 21,
    /// SIGTTOU - 后台写
    SIGTTOU = 22,
}

/// 实时信号范围 (32-64)
pub const SIGRTMIN: i32 = 32;
pub const SIGRTMAX: i32 = 64;

/// 信号标志
///
/// 对应 Linux 的 siginfo_t::si_flags
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct SigFlags(u32);

impl SigFlags {
    pub const SA_NOCLDSTOP: u32 = 0x00000001;  // 子进程停止时不发送 SIGCHLD
    pub const SA_NOCLDWAIT: u32 = 0x00000002;  // 子进程退出时不变成僵尸
    pub const SA_SIGINFO: u32 = 0x00000004;    // 提供额外信息
    pub const SA_ONSTACK: u32 = 0x08000000;    // 使用备用栈
    pub const SA_RESTART: u32 = 0x10000000;    // 重启系统调用
    pub const SA_NODEFER: u32 = 0x40000000;    // 信号处理期间不阻塞自身
    pub const SA_RESETHAND: u32 = 0x80000000;  // 处理后重置为默认

    pub fn new(flags: u32) -> Self {
        Self(flags)
    }

    pub fn bits(&self) -> u32 {
        self.0
    }
}

/// 信号处理动作
///
/// 对应 Linux 的 sigaction (include/uapi/asm-generic/sigcontext.h)
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum SigActionKind {
    /// 默认处理
    Default = 0,
    /// 忽略信号
    Ignore = 1,
    /// 捕获信号 (处理函数指针)
    Handler = 2,
}

/// 信号处理函数类型
pub type SigHandler = unsafe extern "C" fn(i32);

/// sigaction 结构体
///
/// 对应 Linux 的 struct sigaction (include/uapi/asm-generic/signal.h)
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct SigAction {
    /// 信号处理函数指针
    pub sa_handler: usize,
    /// 信号标志
    pub sa_flags: SigFlags,
    /// 信号掩码
    pub sa_mask: u64,
}

impl SigAction {
    /// 创建默认 sigaction
    pub fn new() -> Self {
        Self {
            sa_handler: SigAction::default_handler() as usize,
            sa_flags: SigFlags::new(0),
            sa_mask: 0,
        }
    }

    /// 创建忽略动作
    pub fn ignore() -> Self {
        Self {
            sa_handler: SigAction::ignore_handler() as usize,
            sa_flags: SigFlags::new(0),
            sa_mask: 0,
        }
    }

    /// 创建捕获动作
    pub fn handler(handler: SigHandler, flags: SigFlags) -> Self {
        Self {
            sa_handler: handler as usize,
            sa_flags: flags,
            sa_mask: 0,
        }
    }

    /// 默认处理函数地址
    fn default_handler() -> usize {
        SigActionKind::Default as usize
    }

    /// 忽略处理函数地址
    fn ignore_handler() -> usize {
        SigActionKind::Ignore as usize
    }

    /// 获取动作类型
    pub fn action(&self) -> SigActionKind {
        if self.sa_handler == SigAction::default_handler() as usize {
            SigActionKind::Default
        } else if self.sa_handler == SigAction::ignore_handler() as usize {
            SigActionKind::Ignore
        } else {
            SigActionKind::Handler
        }
    }

    /// 检查是否有自定义处理函数
    pub fn has_handler(&self) -> bool {
        self.action() == SigActionKind::Handler
    }
}

/// 待处理信号集合
///
/// 对应 Linux 的 struct sigpending (include/linux/signal.h)
#[repr(C)]
pub struct SigPending {
    /// 待处理信号位图 (64位，支持信号1-64)
    pub signal: AtomicU64,
}

impl SigPending {
    /// 创建新的待处理信号集合
    pub fn new() -> Self {
        Self {
            signal: AtomicU64::new(0),
        }
    }

    /// 添加信号
    pub fn add(&self, sig: i32) {
        if sig < 1 || sig > 64 {
            return;
        }
        let mask = 1u64 << (sig - 1);
        self.signal.fetch_or(mask, Ordering::AcqRel);
    }

    /// 删除信号
    pub fn remove(&self, sig: i32) {
        if sig < 1 || sig > 64 {
            return;
        }
        let mask = 1u64 << (sig - 1);
        self.signal.fetch_and(!mask, Ordering::AcqRel);
    }

    /// 检查是否有待处理信号
    pub fn has(&self, sig: i32) -> bool {
        if sig < 1 || sig > 64 {
            return false;
        }
        let mask = 1u64 << (sig - 1);
        (self.signal.load(Ordering::Acquire) & mask) != 0
    }

    /// 获取第一个待处理信号
    pub fn first(&self) -> Option<i32> {
        let signals = self.signal.load(Ordering::Acquire);
        if signals == 0 {
            return None;
        }
        // 找到最低的设置位
        let sig = signals.trailing_zeros() as i32 + 1;
        Some(sig)
    }

    /// 清空所有信号
    pub fn clear(&self) {
        self.signal.store(0, Ordering::Release);
    }

    /// 获取所有待处理信号
    pub fn get_all(&self) -> u64 {
        self.signal.load(Ordering::Acquire)
    }
}

/// 信号处理结构
///
/// 对应 Linux 的 struct signal_struct (include/linux/sched/signal.h)
#[repr(C)]
pub struct SignalStruct {
    /// 每个信号的动作 (64个信号)
    pub action: [SigAction; 64],
    /// 信号掩码
    pub mask: AtomicU64,
}

impl SignalStruct {
    /// 创建新的信号处理结构
    pub fn new() -> Self {
        let mut actions = [SigAction::new(); 64];

        // 设置默认动作
        use crate::signal::Signal::*;
        actions[Signal::SIGKILL as usize - 1] = SigAction::new();  // SIGKILL: 默认杀死
        actions[Signal::SIGSTOP as usize - 1] = SigAction::new();  // SIGSTOP: 默认停止

        // SIGCHLD 默认忽略
        actions[Signal::SIGCHLD as usize - 1] = SigAction::ignore();

        Self {
            action: actions,
            mask: AtomicU64::new(0),
        }
    }

    /// 设置信号处理动作
    pub fn set_action(&mut self, sig: i32, action: SigAction) -> Result<(), ()> {
        if sig < 1 || sig > 64 {
            return Err(());
        }

        // SIGKILL 和 SIGSTOP 不能被捕获或忽略
        if sig == Signal::SIGKILL as i32 || sig == Signal::SIGSTOP as i32 {
            return Err(());
        }

        self.action[(sig - 1) as usize] = action;
        Ok(())
    }

    /// 获取信号处理动作
    pub fn get_action(&self, sig: i32) -> Option<&SigAction> {
        if sig < 1 || sig > 64 {
            return None;
        }
        Some(&self.action[(sig - 1) as usize])
    }

    /// 添加信号掩码
    pub fn add_mask(&self, sig: i32) {
        if sig < 1 || sig > 64 {
            return;
        }
        let mask = 1u64 << (sig - 1);
        self.mask.fetch_or(mask, Ordering::AcqRel);
    }

    /// 删除信号掩码
    pub fn remove_mask(&self, sig: i32) {
        if sig < 1 || sig > 64 {
            return;
        }
        let mask = 1u64 << (sig - 1);
        self.mask.fetch_and(!mask, Ordering::AcqRel);
    }

    /// 检查信号是否被屏蔽
    pub fn is_masked(&self, sig: i32) -> bool {
        if sig < 1 || sig > 64 {
            return false;
        }
        let mask = 1u64 << (sig - 1);
        (self.mask.load(Ordering::Acquire) & mask) != 0
    }
}

/// 信号信息结构
///
/// 对应 Linux 的 siginfo_t (include/uapi/asm-generic/siginfo.h)
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct SigInfo {
    /// 信号编号
    pub si_signo: i32,
    /// 信号代码
    pub si_code: i32,
    /// 发送进程的 PID
    pub si_pid: u32,
    /// 发送进程的 UID
    pub si_uid: u32,
    /// 退出状态或错误值
    pub si_status: i32,
}

impl SigInfo {
    /// 创建新的信号信息
    pub fn new(signo: i32, code: i32, pid: u32, uid: u32) -> Self {
        Self {
            si_signo: signo,
            si_code: code,
            si_pid: pid,
            si_uid: uid,
            si_status: 0,
        }
    }

    /// 创建子进程退出信号信息
    pub fn child(pid: u32, uid: u32, status: i32) -> Self {
        Self {
            si_signo: Signal::SIGCHLD as i32,
            si_code: 1, // CLD_EXITED
            si_pid: pid,
            si_uid: uid,
            si_status: status,
        }
    }
}

/// kill 系统调用使用的代码值
pub mod si_code {
    /// 用户发送的信号 (kill)
    pub const SI_USER: i32 = 0;
    /// 内核发送的信号
    pub const SI_KERNEL: i32 = 0x80;
    /// 子进程退出
    pub const CLD_EXITED: i32 = 1;
    /// 子进程被杀死
    pub const CLD_KILLED: i32 = 2;
    /// 子进程异常终止
    pub const CLD_DUMPED: i32 = 3;
}

// ============================================================================
// 信号帧结构 (Signal Frame)
// ============================================================================

/// 用户上下文 - 信号处理时保存的寄存器状态
///
/// 对应 Linux 的 ucontext_t (include/uapi/asm-generic/ucontext.h)
/// aarch64 特定版本 (arch/arm64/include/uapi/asm/sigcontext.h)
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct UContext {
    /// 信号掩码
    pub uc_sigmask: u64,
    /// 保留字段
    pub uc_flags: u64,
    /// 链接到下一个 ucontext (用于 swapcontext)
    pub uc_link: u64,
    /// 栈指针
    pub uc_stack: u64,
    /// 寄存器上下文 (在未来扩展)
    pub uc_mcontext: [u64; 32],  // x0-x30 + sp
}

impl UContext {
    /// 创建新的用户上下文
    pub fn new() -> Self {
        Self {
            uc_sigmask: 0,
            uc_flags: 0,
            uc_link: 0,
            uc_stack: 0,
            uc_mcontext: [0; 32],
        }
    }
}

/// 信号返回 trampoline 代码
///
/// 当信号处理函数返回时，会跳转到这个地址，
/// 然后执行 rt_sigreturn 系统调用恢复上下文
const SIGRETURN_TRAMPOLINE: &[u8] = &[
    0x00, 0x00, 0x00, 0x00,  // 魔术字
];

/// 信号帧 - 在用户栈上构建
///
/// 对应 Linux 的 sigframe (arch/arm64/kernel/signal.c)
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct SignalFrame {
    /// 保留字（对齐和魔术）
    pub reserved: [u64; 4],
    /// 信号信息
    pub info: SigInfo,
    /// 用户上下文
    pub uc: UContext,
    /// trampoline 代码（占位）
    pub trampoline: [u8; 4],
}

impl SignalFrame {
    /// 计算信号帧的总大小
    pub const fn size() -> usize {
        core::mem::size_of::<SignalFrame>()
    }
}

/// 信号处理相关常量
pub mod consts {
    /// 信号处理时的备用栈大小
    pub const SIGSTKSZ: usize = 8192;
    /// 最小备用栈大小
    pub const MINSIGSTKSZ: usize = 2048;

    /// 默认信号栈大小
    pub const DEFAULT_SIGSTACK_SIZE: usize = SIGSTKSZ;
}

// ============================================================================
// 信号处理和传递
// ============================================================================

/// 检查并处理待处理的信号
///
/// 对应 Linux 内核的 do_signal() (kernel/signal.c)
///
/// # Returns
///
/// * `true` - 如果有待处理的信号
/// * `false` - 如果没有待处理的信号
pub fn do_signal() -> bool {
    use crate::process::sched;

    unsafe {
        let current = sched::RQ.current;
        if current.is_null() {
            return false;
        }

        // 获取待处理信号队列
        let pending = &(*current).pending;

        // 检查是否有待处理信号
        if let Some(sig) = pending.first() {
            use crate::console::putchar;
            const MSG: &[u8] = b"do_signal: processing signal\n";
            for &b in MSG {
                putchar(b);
            }

            // 获取信号处理动作
            if let Some(signal_struct) = (*current).signal.as_ref() {
                if let Some(action) = signal_struct.get_action(sig) {
                    // 检查是否有自定义处理函数
                    if action.has_handler() {
                        const MSG2: &[u8] = b"do_signal: setting up signal frame\n";
                        for &b in MSG2 {
                            putchar(b);
                        }

                        // 调用信号处理函数
                        if setup_frame(current, sig, action) {
                            const MSG3: &[u8] = b"do_signal: frame setup successful\n";
                            for &b in MSG3 {
                                putchar(b);
                            }
                        } else {
                            const MSG4: &[u8] = b"do_signal: frame setup failed\n";
                            for &b in MSG4 {
                                putchar(b);
                            }
                            // 设置失败，执行默认动作
                            handle_default_signal(sig);
                        }
                    } else {
                        const MSG3: &[u8] = b"do_signal: signal has default action\n";
                        for &b in MSG3 {
                            putchar(b);
                        }
                        // 执行默认动作
                        handle_default_signal(sig);
                    }
                }
            }

            // 从待处理队列中删除信号
            pending.remove(sig);

            true
        } else {
            false
        }
    }
}

/// 设置信号帧并准备调用信号处理函数
///
/// 对应 Linux 内核的 setup_frame() (arch/arm64/kernel/signal.c)
///
/// # Arguments
///
/// * `task` - 当前任务
/// * `sig` - 信号编号
/// * `action` - 信号处理动作
///
/// # Returns
///
/// * `true` - 设置成功
/// * `false` - 设置失败
unsafe fn setup_frame(
    task: *mut crate::process::task::Task,
    sig: i32,
    action: &SigAction,
) -> bool {
    use crate::console::putchar;

    // 获取任务的 CPU 上下文
    let ctx = (*task).context_mut();

    // 定义用户栈地址范围（假设的用户空间栈）
    const USER_STACK_TOP: u64 = 0x0000_7fff_f000_0000;
    const SIGNAL_FRAME_SIZE: u64 = SignalFrame::size() as u64;

    // 计算信号帧位置（在用户栈顶部）
    let frame_addr = USER_STACK_TOP - SIGNAL_FRAME_SIZE;

    const MSG1: &[u8] = b"setup_frame: allocating signal frame\n";
    for &b in MSG1 {
        putchar(b);
    }

    // TODO: 在实际的用户栈内存上构建信号帧
    // 当前简化实现：暂时不真正构建信号帧
    // 完整实现需要：
    // 1. 验证用户栈地址有效
    // 2. 分配信号帧空间
    // 3. 使用 copy_to_user 填充信号帧内容

    // 创建信号信息
    let _info = SigInfo::new(sig, crate::signal::si_code::SI_KERNEL, (*task).pid(), 0);

    const MSG2: &[u8] = b"setup_frame: modifying cpu context\n";
    for &b in MSG2 {
        putchar(b);
    }

    // 保存旧的上下文（用于恢复）
    let _old_pc = ctx.pc;
    let _old_sp = ctx.sp;

    // 设置信号处理函数参数
    ctx.x0 = sig as u64;              // 第一个参数：信号编号
    ctx.x1 = frame_addr + 32;         // 第二个参数：&info (偏移到 info 字段)
    ctx.x2 = frame_addr + 32 + 104;   // 第三个参数：&uc (偏移到 uc 字段)

    // 设置返回地址为信号处理函数
    ctx.pc = action.sa_handler as u64;

    // 设置用户栈指针到信号帧位置
    ctx.user_sp = frame_addr;

    const MSG3: &[u8] = b"setup_frame: context configured\n";
    for &b in MSG3 {
        putchar(b);
    }

    // TODO: 保存旧上下文到信号帧（用于 sigreturn 恢复）
    // 完整实现需要将 old_pc, old_sp 和其他寄存器保存到信号帧的 uc_mcontext 中

    true  // 成功
}

/// 处理信号的默认动作
///
/// 对应 Linux 内核的 do_default()
fn handle_default_signal(sig: i32) {
    use crate::signal::Signal::*;
    use crate::console::putchar;

    const MSG_TERM: &[u8] = b"handle_default_signal: terminating on signal\n";
    const MSG_IGNORE: &[u8] = b"handle_default_signal: ignoring signal\n";
    const MSG_STOP: &[u8] = b"handle_default_signal: stopping on signal\n";
    const MSG_CONTINUE: &[u8] = b"handle_default_signal: continuing on signal\n";
    const MSG_KILL: &[u8] = b"handle_default_signal: force kill\n";
    const MSG_UNKNOWN: &[u8] = b"handle_default_signal: unknown signal\n";

    match sig {
        // 忽略这些信号
        17 => {  // SIGCHLD
            for &b in MSG_IGNORE {
                putchar(b);
            }
        }
        // 终止进程
        1 | 2 | 3 | 4 | 5 | 6   // SIGHUP | SIGINT | SIGQUIT | SIGILL | SIGTRAP | SIGABRT
        | 7 | 8 | 11 | 13 | 14 | 15  // SIGBUS | SIGFPE | SIGSEGV | SIGPIPE | SIGALRM | SIGTERM
        | 16 | 10 | 12 => {          // SIGSTKFLT | SIGUSR1 | SIGUSR2
            for &b in MSG_TERM {
                putchar(b);
            }
            // TODO: 调用 exit 系统调用或直接终止进程
            // crate::process::sched::do_exit(sig);
        }
        // 停止进程
        20 | 21 | 22 => {  // SIGTSTP | SIGTTIN | SIGTTOU
            for &b in MSG_STOP {
                putchar(b);
            }
            // TODO: 实现进程停止
        }
        // 强制杀死（不应该到达这里）
        9 => {  // SIGKILL
            for &b in MSG_KILL {
                putchar(b);
            }
            // 强制终止，不能被捕获
            // crate::process::sched::do_exit(sig);
        }
        // 继续进程
        18 | 19 => {  // SIGCONT | SIGSTOP
            for &b in MSG_CONTINUE {
                putchar(b);
            }
            // TODO: 如果进程被停止，恢复它
        }
        _ => {
            for &b in MSG_UNKNOWN {
                putchar(b);
            }
        }
    }
}

/// 发送信号到进程
///
/// 对应 Linux 内核的 send_signal()
///
/// # Arguments
///
/// * `pid` - 目标进程 PID
/// * `sig` - 信号编号
/// * `info` - 信号信息
///
/// # Returns
///
/// * `true` - 信号发送成功
/// * `false` - 信号发送失败
pub fn send_signal(pid: u32, sig: i32) -> bool {
    use crate::process::sched;
    use crate::console::putchar;

    unsafe {
        // 查找目标进程
        let task = sched::find_task_by_pid(pid);
        if task.is_null() {
            const MSG: &[u8] = b"send_signal: failed to find PID\n";
            for &b in MSG {
                putchar(b);
            }
            return false;
        }

        // 添加到待处理信号队列
        (*task).pending.add(sig);

        const MSG2: &[u8] = b"send_signal: sent signal to PID\n";
        for &b in MSG2 {
            putchar(b);
        }
        true
    }
}

/// 检查并处理信号（在内核返回用户空间前调用）
///
/// 对应 Linux 内核的 exit_to_usermode()
pub fn check_and_deliver_signals() {
    use crate::process::sched;
    use crate::console::putchar;

    unsafe {
        let current = sched::RQ.current;
        if !current.is_null() {
            let pending = (*current).pending();

            // 如果有待处理信号，处理它们
            if pending.get_all() != 0 {
                const MSG: &[u8] = b"check_and_deliver_signals: pending signals\n";
                for &b in MSG {
                    putchar(b);
                }
                do_signal();
            }
        }
    }
}

