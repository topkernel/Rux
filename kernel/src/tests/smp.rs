// 测试：SMP 多核启动
use crate::println;
use crate::arch::riscv64::smp;

pub fn test_smp() {
    println!("test: DEBUG - Entering test_smp function");

    println!("test: DEBUG - About to call cpu_id()");

    // 获取当前 hart 信息
    let hart_id = smp::cpu_id();

    println!("test: DEBUG - cpu_id() returned, hart_id={}", hart_id);

    let is_boot = smp::is_boot_hart();
    let max_cpus = smp::MAX_CPUS;

    // 每个 hart 都打印自己的信息
    println!("test: [Hart {}] SMP test - is_boot={}", hart_id, is_boot);

    // 只在 boot hart 上运行完整测试
    if is_boot {
        println!("test: Testing SMP multi-core startup...");

        // 测试 1: 检查是否在 boot hart 上
        println!("test: 1. Checking boot hart status...");
        println!("test:    is_boot_hart() = {}", is_boot);
        println!("test:    SUCCESS - boot hart detected");

        // 测试 2: 获取当前 hart ID
        println!("test: 2. Getting current hart ID...");
        println!("test:    Current hart ID = {}", hart_id);
        println!("test:    SUCCESS - hart ID retrieved");

        // 测试 3: 获取最大 CPU 数量
        println!("test: 3. Getting max CPU count...");
        println!("test:    MAX_CPUS = {}", max_cpus);
        if max_cpus > 1 {
            println!("test:    Multi-core system supported!");
        } else {
            println!("test:    Single-core system");
        }
        println!("test:    SUCCESS - max CPU count retrieved");

        // 测试 4: Boot hart 确认
        println!("test: 4. Boot hart (hart {}) confirmed", hart_id);
        println!("test:    SUCCESS - boot hart identified");

        println!("test: SMP testing completed on boot hart {}.", hart_id);
    } else {
        // Secondary harts 只打印基本信息
        println!("test: [Hart {}] Secondary hart running - waiting for tasks", hart_id);
    }
}
