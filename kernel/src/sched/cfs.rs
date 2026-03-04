//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Completely Fair Scheduler (CFS) 实现
//!
//! 参考: Linux kernel/sched/fair.c
//!
//! CFS 的核心思想：
//! 1. 使用虚拟运行时 (vruntime) 来衡量进程获得的 CPU 时间
//! 2. vruntime = 实际运行时间 * (NICE_0_LOAD / 进程权重)
//! 3. 优先级高的进程权重更大，vruntime 增长更慢
//! 4. 调度时选择 vruntime 最小的进程运行
//!
//! 关键数据结构：
//! - SchedEntity: 调度实体，包含 vruntime、权重等
//! - CfsRunQueue: CFS 运行队列，使用 BTreeMap 按 vruntime 排序
//! - LoadWeight: 进程权重，与 nice 值相关

use alloc::collections::BTreeMap;
use core::sync::atomic::{AtomicU64, AtomicBool, Ordering};
use spin::Mutex;

/// 时钟频率 (HZ)
/// Linux 默认 1000，我们使用 100 以简化计算
const HZ: u64 = 100;

/// 调度粒度 (纳秒)
/// 最小调度时间片，防止过于频繁的上下文切换
/// Linux 默认: 700000 ns (0.7ms)
pub const SCHED_MIN_GRANULARITY_NS: u64 = 700_000;

/// 调度延迟 (纳秒)
/// 目标调度周期，所有可运行进程在这段时间内至少运行一次
/// Linux 默认: 6000000 ns (6ms)
pub const SCHED_LATENCY_NS: u64 = 6_000_000;

/// nice 值为 0 时的权重
/// 参考: Linux kernel/sched/core.c sched_prio_to_weight
pub const NICE_0_LOAD: u64 = 1024;

/// nice 值到权重的映射表
///
/// nice 值范围: -20 到 +19，共 40 个级别
/// 权重按照 1.25 的倍数变化（每 1024 约增加 25%）
///
/// 参考: Linux kernel/sched/core.c sched_prio_to_weight[]
pub const PRIO_TO_WEIGHT: [u64; 40] = [
    /* -20 */ 88761, 71755, 56483, 46273, 36291,
    /* -15 */ 29154, 23254, 18705, 14949, 11916,
    /* -10 */ 9548,  7620,  6100,  4904,  3906,
    /*  -5 */ 3121,  2501,  1991,  1586,  1277,
    /*   0 */ 1024,   820,   655,   526,   423,
    /*   5 */ 335,    272,   215,   172,   137,
    /*  10 */ 110,     87,    70,    56,    45,
    /*  15 */ 36,     29,    23,    18,    15,
];

/// nice 值到权重乘数的映射表（用于快速计算）
///
/// 用于计算 vruntime: delta_exec * weight / lw->weight
/// 这里存储的是 NICE_0_LOAD * 2^32 / weight
///
/// 参考: Linux kernel/sched/core.c sched_prio_to_wmult[]
pub const PRIO_TO_WMULT: [u64; 40] = [
    /* -20 */ 48388, 59856, 76040, 92818, 118348,
    /* -15 */ 147320, 184698, 229616, 288308, 360437,
    /* -10 */ 449829, 563644, 704093, 875809, 1099582,
    /*  -5 */ 1376151, 1717300, 2157191, 2708050, 3363326,
    /*   0 */ 4194304, 5237760, 6557202, 8165337, 10153587,
    /*   5 */ 12820794, 15790321, 19976592, 24970740, 31350126,
    /*  10 */ 39045157, 49367440, 61356676, 76695844, 95443717,
    /*  15 */ 119304647, 148154320, 186737708, 238609294, 286331153,
];

/// 负载权重
///
/// 参考: Linux include/linux/sched.h struct load_weight
#[derive(Debug, Clone, Copy)]
pub struct LoadWeight {
    /// 权重值
    pub weight: u64,
    /// 权重乘数 (用于快速除法)
    pub inv_weight: u64,
}

impl LoadWeight {
    /// 创建新的负载权重
    pub fn new(weight: u64) -> Self {
        Self {
            weight,
            inv_weight: 0,
        }
    }

    /// 从 nice 值创建负载权重
    ///
    /// nice 值范围: -20 到 +19
    /// 默认 nice 值为 0，对应权重 1024
    pub fn from_nice(nice: i32) -> Self {
        // 将 nice 值转换为数组索引 (0-39)
        let idx = (nice + 20) as usize;
        let idx = idx.min(39).max(0);

        Self {
            weight: PRIO_TO_WEIGHT[idx],
            inv_weight: PRIO_TO_WMULT[idx],
        }
    }

    /// 更新 inv_weight（用于快速除法）
    pub fn update_inv_weight(&mut self) {
        if self.inv_weight == 0 {
            if self.weight >= (1u64 << 32) {
                self.inv_weight = 1;
            } else {
                self.inv_weight = (1u64 << 32) / self.weight;
            }
        }
    }
}

impl Default for LoadWeight {
    fn default() -> Self {
        Self::from_nice(0) // 默认 nice 值为 0
    }
}

/// 调度实体
///
/// 参考: Linux include/linux/sched.h struct sched_entity
#[derive(Debug)]
pub struct SchedEntity {
    /// 负载权重
    pub load: LoadWeight,

    /// 虚拟运行时
    ///
    /// vruntime 越小，说明进程获得的 CPU 时间越少
    /// 调度器优先选择 vruntime 小的进程
    pub vruntime: AtomicU64,

    /// 累计执行时间（纳秒）
    pub sum_exec_runtime: AtomicU64,

    /// 上次开始执行的时间（纳秒）
    pub exec_start: AtomicU64,

    /// 上次累计执行时间（用于计算增量）
    pub prev_sum_exec_runtime: AtomicU64,

    /// 是否在运行队列中
    pub on_rq: AtomicBool,

    /// 时间片（纳秒）
    pub slice: AtomicU64,
}

impl SchedEntity {
    /// 创建新的调度实体
    pub fn new() -> Self {
        Self {
            load: LoadWeight::default(),
            vruntime: AtomicU64::new(0),
            sum_exec_runtime: AtomicU64::new(0),
            exec_start: AtomicU64::new(0),
            prev_sum_exec_runtime: AtomicU64::new(0),
            on_rq: AtomicBool::new(false),
            slice: AtomicU64::new(0),
        }
    }

    /// 从 nice 值创建调度实体
    pub fn from_nice(nice: i32) -> Self {
        Self {
            load: LoadWeight::from_nice(nice),
            vruntime: AtomicU64::new(0),
            sum_exec_runtime: AtomicU64::new(0),
            exec_start: AtomicU64::new(0),
            prev_sum_exec_runtime: AtomicU64::new(0),
            on_rq: AtomicBool::new(false),
            slice: AtomicU64::new(0),
        }
    }

    /// 设置 nice 值
    pub fn set_nice(&mut self, nice: i32) {
        self.load = LoadWeight::from_nice(nice);
    }

    /// 获取虚拟运行时
    #[inline]
    pub fn get_vruntime(&self) -> u64 {
        self.vruntime.load(Ordering::Acquire)
    }

    /// 设置虚拟运行时
    #[inline]
    pub fn set_vruntime(&self, vruntime: u64) {
        self.vruntime.store(vruntime, Ordering::Release);
    }

    /// 增加虚拟运行时
    #[inline]
    pub fn add_vruntime(&self, delta: u64) {
        self.vruntime.fetch_add(delta, Ordering::AcqRel);
    }

    /// 更新执行时间
    ///
    /// # 参数
    /// - `now`: 当前时间（纳秒）
    ///
    /// # 返回
    /// 本次执行的时间增量（纳秒）
    pub fn update_exec_runtime(&self, now: u64) -> u64 {
        let exec_start = self.exec_start.load(Ordering::Acquire);

        if exec_start == 0 {
            // 第一次执行，记录开始时间
            self.exec_start.store(now, Ordering::Release);
            return 0;
        }

        let delta = if now > exec_start {
            now - exec_start
        } else {
            0 // 防止时间回绕
        };

        // 更新累计执行时间
        self.sum_exec_runtime.fetch_add(delta, Ordering::AcqRel);

        // 更新开始时间
        self.exec_start.store(now, Ordering::Release);

        delta
    }

    /// 计算虚拟运行时增量
    ///
    /// vruntime += delta_exec * (NICE_0_LOAD / weight)
    ///
    /// 使用乘数避免除法：
    /// vruntime += delta_exec * (inv_weight >> 32)
    ///
    /// # 参数
    /// - `delta_exec`: 实际执行时间（纳秒）
    ///
    /// # 返回
    /// 虚拟运行时增量
    pub fn calc_delta_fair(&self, delta_exec: u64) -> u64 {
        // 如果权重等于 NICE_0_LOAD，直接返回
        if self.load.weight == NICE_0_LOAD {
            return delta_exec;
        }

        // 使用乘法代替除法
        // delta = delta_exec * NICE_0_LOAD / weight
        //       = delta_exec * inv_weight >> 32
        let mut load = self.load;
        load.update_inv_weight();

        // 使用 64 位乘法和移位
        let delta = (delta_exec * load.inv_weight) >> 32;

        delta
    }

    /// 更新虚拟运行时
    ///
    /// # 参数
    /// - `delta_exec`: 实际执行时间（纳秒）
    pub fn update_vruntime(&self, delta_exec: u64) {
        let delta_vruntime = self.calc_delta_fair(delta_exec);
        self.add_vruntime(delta_vruntime);
    }

    /// 检查是否在运行队列中
    #[inline]
    pub fn is_on_rq(&self) -> bool {
        self.on_rq.load(Ordering::Acquire)
    }

    /// 设置运行队列状态
    #[inline]
    pub fn set_on_rq(&self, on_rq: bool) {
        self.on_rq.store(on_rq, Ordering::Release);
    }

    /// 获取时间片
    #[inline]
    pub fn get_slice(&self) -> u64 {
        self.slice.load(Ordering::Acquire)
    }

    /// 设置时间片
    #[inline]
    pub fn set_slice(&self, slice: u64) {
        self.slice.store(slice, Ordering::Release);
    }
}

impl Default for SchedEntity {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for SchedEntity {
    fn clone(&self) -> Self {
        Self {
            load: self.load,
            vruntime: AtomicU64::new(self.vruntime.load(Ordering::Acquire)),
            sum_exec_runtime: AtomicU64::new(self.sum_exec_runtime.load(Ordering::Acquire)),
            exec_start: AtomicU64::new(0), // 重置执行开始时间
            prev_sum_exec_runtime: AtomicU64::new(self.prev_sum_exec_runtime.load(Ordering::Acquire)),
            on_rq: AtomicBool::new(false),
            slice: AtomicU64::new(self.slice.load(Ordering::Acquire)),
        }
    }
}

/// 用于 BTreeMap 的键
///
/// 由于 vruntime 可能重复，我们使用 (vruntime, task_ptr) 作为键
/// 这样可以保证唯一性
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct VruntimeKey {
    vruntime: u64,
    task_id: u64, // 用于区分相同 vruntime 的任务
}

impl VruntimeKey {
    fn new(vruntime: u64, task_id: u64) -> Self {
        Self { vruntime, task_id }
    }
}

/// CFS 运行队列
///
/// 参考: Linux kernel/sched/sched.h struct cfs_rq
pub struct CfsRunQueue {
    /// 按 vruntime 排序的任务队列
    ///
    /// 键: (vruntime, task_id)
    /// 值: 任务指针
    tasks_timeline: BTreeMap<VruntimeKey, *mut crate::process::Task>,

    /// 当前运行的调度实体
    pub curr: *mut crate::process::Task,

    /// 队列中最小的 vruntime
    ///
    /// 用于新任务的 vruntime 初始化
    /// 新任务的 vruntime = min_vruntime，这样可以防止新任务获得过多 CPU 时间
    pub min_vruntime: AtomicU64,

    /// 运行队列中的任务数量
    nr_running: AtomicU64,

    /// 总权重
    load_weight: AtomicU64,

    /// 下一个任务 ID（用于生成唯一键）
    next_task_id: AtomicU64,
}

impl CfsRunQueue {
    /// 创建新的 CFS 运行队列
    pub fn new() -> Self {
        Self {
            tasks_timeline: BTreeMap::new(),
            curr: core::ptr::null_mut(),
            min_vruntime: AtomicU64::new(0),
            nr_running: AtomicU64::new(0),
            load_weight: AtomicU64::new(0),
            next_task_id: AtomicU64::new(0),
        }
    }

    /// 获取运行队列中的任务数量
    #[inline]
    pub fn nr_running(&self) -> u64 {
        self.nr_running.load(Ordering::Acquire)
    }

    /// 检查运行队列是否为空
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.tasks_timeline.is_empty()
    }

    /// 获取最小 vruntime
    #[inline]
    pub fn get_min_vruntime(&self) -> u64 {
        self.min_vruntime.load(Ordering::Acquire)
    }

    /// 更新最小 vruntime
    fn update_min_vruntime(&mut self) {
        // 从队列中获取最小的 vruntime
        if let Some((&key, _)) = self.tasks_timeline.iter().next() {
            let min_vruntime = self.min_vruntime.load(Ordering::Acquire);

            // min_vruntime 只增不减，确保单调递增
            if key.vruntime > min_vruntime {
                self.min_vruntime.store(key.vruntime, Ordering::Release);
            }
        }
    }

    /// 将任务加入运行队列
    ///
    /// 参考: Linux kernel/sched/fair.c enqueue_entity
    ///
    /// # 参数
    /// - `task`: 要加入的任务指针
    ///
    /// # 返回
    /// 成功返回 true，如果任务已在队列中返回 false
    pub fn enqueue(&mut self, task: *mut crate::process::Task) -> bool {
        if task.is_null() {
            return false;
        }

        unsafe {
            let task_ref = &mut *task;

            // 获取调度实体
            let se = task_ref.sched_entity();

            // 如果任务已在运行队列中，不重复入队
            if se.is_on_rq() {
                return false;
            }

            // 新任务的 vruntime 从 min_vruntime 开始
            // 这样新任务不会获得过多 CPU 时间
            let min_vruntime = self.get_min_vruntime();
            se.set_vruntime(min_vruntime);

            // 生成唯一键
            let task_id = self.next_task_id.fetch_add(1, Ordering::AcqRel);
            let key = VruntimeKey::new(se.get_vruntime(), task_id);

            // 加入 BTreeMap
            self.tasks_timeline.insert(key, task);

            // 更新状态
            se.set_on_rq(true);

            // 更新任务计数和总权重
            self.nr_running.fetch_add(1, Ordering::AcqRel);
            self.load_weight.fetch_add(se.load.weight, Ordering::AcqRel);

            // 更新最小 vruntime
            self.update_min_vruntime();

            true
        }
    }

    /// 将任务从运行队列移除
    ///
    /// 参考: Linux kernel/sched/fair.c dequeue_entity
    ///
    /// # 参数
    /// - `task`: 要移除的任务指针
    ///
    /// # 返回
    /// 成功返回 true
    pub fn dequeue(&mut self, task: *mut crate::process::Task) -> bool {
        if task.is_null() {
            return false;
        }

        unsafe {
            let task_ref = &mut *task;
            let se = task_ref.sched_entity();

            // 查找并移除任务
            let vruntime = se.get_vruntime();

            // 遍历查找匹配的任务
            let mut found_key = None;
            for (&key, &ptr) in self.tasks_timeline.iter() {
                if ptr == task && key.vruntime == vruntime {
                    found_key = Some(key);
                    break;
                }
            }

            if let Some(key) = found_key {
                self.tasks_timeline.remove(&key);

                // 更新状态
                se.set_on_rq(false);

                // 更新任务计数和总权重
                self.nr_running.fetch_sub(1, Ordering::AcqRel);
                self.load_weight.fetch_sub(se.load.weight, Ordering::AcqRel);

                // 更新最小 vruntime
                self.update_min_vruntime();

                return true;
            }

            false
        }
    }

    /// 选择下一个要运行的任务
    ///
    /// 参考: Linux kernel/sched/fair.c pick_next_entity
    ///
    /// 选择 vruntime 最小的任务
    ///
    /// # 返回
    /// 下一个要运行的任务指针，如果队列为空返回 None
    pub fn pick_next(&mut self) -> Option<*mut crate::process::Task> {
        // 获取 vruntime 最小的任务
        if let Some((&_key, &task)) = self.tasks_timeline.iter().next() {
            // 从队列中移除（调度时移除，时间片用完或主动让出时重新加入）
            let _ = self.dequeue(task);
            return Some(task);
        }

        None
    }

    /// 选择下一个要运行的任务（不移除）
    ///
    /// 用于查看下一个任务但不改变队列状态
    pub fn peek_next(&self) -> Option<*mut crate::process::Task> {
        if let Some((&_key, &task)) = self.tasks_timeline.iter().next() {
            return Some(task);
        }
        None
    }

    /// 更新当前任务的运行时间
    ///
    /// 参考: Linux kernel/sched/fair.c update_curr
    ///
    /// # 参数
    /// - `now`: 当前时间（纳秒）
    pub fn update_curr(&mut self, now: u64) {
        if self.curr.is_null() {
            return;
        }

        unsafe {
            let task = &mut *self.curr;
            let se = task.sched_entity();

            // 更新执行时间
            let delta_exec = se.update_exec_runtime(now);

            if delta_exec > 0 {
                // 更新虚拟运行时
                se.update_vruntime(delta_exec);

                // 更新最小 vruntime
                self.update_min_vruntime();
            }
        }
    }

    /// 计算时间片
    ///
    /// 参考: Linux kernel/sched/fair.c sched_slice
    ///
    /// 时间片 = 调度延迟 * 进程权重 / 总权重
    ///
    /// # 参数
    /// - `se`: 调度实体
    ///
    /// # 返回
    /// 时间片（纳秒）
    pub fn sched_slice(&self, se: &SchedEntity) -> u64 {
        let nr_running = self.nr_running.load(Ordering::Acquire);

        if nr_running == 0 {
            return SCHED_MIN_GRANULARITY_NS;
        }

        // 计算调度周期
        // 如果进程数量较多，使用 min_granularity * nr_running
        // 否则使用固定的调度延迟
        let sched_period = if nr_running > SCHED_LATENCY_NS / SCHED_MIN_GRANULARITY_NS {
            SCHED_MIN_GRANULARITY_NS * nr_running
        } else {
            SCHED_LATENCY_NS
        };

        // 计算时间片
        // slice = period * weight / total_weight
        let load_weight = self.load_weight.load(Ordering::Acquire);

        if load_weight == 0 {
            return SCHED_MIN_GRANULARITY_NS;
        }

        // 使用乘法避免除法精度问题
        let slice = (sched_period * se.load.weight) / load_weight;

        // 确保不小于最小粒度
        slice.max(SCHED_MIN_GRANULARITY_NS)
    }

    /// 检查是否需要抢占当前任务
    ///
    /// 参考: Linux kernel/sched/fair.c check_preempt_wakeup
    ///
    /// # 参数
    /// - `curr`: 当前任务
    /// - `se`: 新唤醒的任务
    ///
    /// # 返回
    /// 如果需要抢占返回 true
    pub fn check_preempt(&self, curr: &SchedEntity, se: &SchedEntity) -> bool {
        // 如果新任务的 vruntime 小于当前任务，应该抢占
        let curr_vruntime = curr.get_vruntime();
        let se_vruntime = se.get_vruntime();

        // 使用 "wakeup granularity" 作为阈值
        // 如果差距超过这个值，才进行抢占
        let wakeup_granularity = SCHED_MIN_GRANULARITY_NS;

        // 防止 vruntime 回绕
        if se_vruntime < curr_vruntime {
            let delta = curr_vruntime - se_vruntime;
            delta > wakeup_granularity
        } else {
            false
        }
    }

    /// 设置当前运行的任务
    pub fn set_curr(&mut self, task: *mut crate::process::Task) {
        self.curr = task;
    }

    /// 获取当前运行的任务
    #[inline]
    pub fn get_curr(&self) -> *mut crate::process::Task {
        self.curr
    }

    /// 清空运行队列
    pub fn clear(&mut self) {
        // 标记所有任务为不在队列中
        for (_, &task) in self.tasks_timeline.iter() {
            if !task.is_null() {
                unsafe {
                    let task_ref = &mut *task;
                    task_ref.sched_entity().set_on_rq(false);
                }
            }
        }

        self.tasks_timeline.clear();
        self.curr = core::ptr::null_mut();
        self.nr_running.store(0, Ordering::Release);
        self.load_weight.store(0, Ordering::Release);
    }
}

impl Default for CfsRunQueue {
    fn default() -> Self {
        Self::new()
    }
}

unsafe impl Send for CfsRunQueue {}
unsafe impl Sync for CfsRunQueue {}

/// 计算时间片（毫秒）
///
/// 将纳秒时间片转换为毫秒，用于时钟中断
pub fn sched_slice_to_ms(slice_ns: u64) -> u32 {
    (slice_ns / 1_000_000) as u32
}

/// 从毫秒转换为纳秒
pub fn ms_to_ns(ms: u32) -> u64 {
    (ms as u64) * 1_000_000
}

/// 获取当前时间（纳秒）
///
/// 使用 RISC-V 时间寄存器
pub fn sched_clock() -> u64 {
    // 读取 time 寄存器
    let time: u64;
    unsafe {
        core::arch::asm!(
            "rdtime {time}",
            time = out(reg) time,
            options(nomem, nostack)
        );
    }
    // 假设时钟频率为 10MHz (100ns 精度)
    // 实际需要根据平台调整
    time * 100
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_weight() {
        // nice = 0 的权重应该是 1024
        let lw = LoadWeight::from_nice(0);
        assert_eq!(lw.weight, 1024);

        // nice = -20 的权重应该最大
        let lw_high = LoadWeight::from_nice(-20);
        assert!(lw_high.weight > lw.weight);

        // nice = 19 的权重应该最小
        let lw_low = LoadWeight::from_nice(19);
        assert!(lw_low.weight < lw.weight);
    }

    #[test]
    fn test_vruntime_calculation() {
        let se = SchedEntity::new();

        // nice = 0 时，vruntime 应该等于实际运行时间
        let delta = 1_000_000; // 1ms
        let vruntime = se.calc_delta_fair(delta);
        assert_eq!(vruntime, delta);
    }

    #[test]
    fn test_cfs_rq_enqueue_dequeue() {
        let mut rq = CfsRunQueue::new();

        // 创建测试任务结构
        // 注意：实际测试需要有效的 Task 指针
        assert!(rq.is_empty());
        assert_eq!(rq.nr_running(), 0);
    }
}
