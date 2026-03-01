//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! 内存相关系统调用
//!
//! 包含：brk, mmap, mmap_framebuffer, munmap, mprotect, msync, mremap, madvise, mincore, mlock, munlock

use super::SyscallArgs;

/// sys_brk - 改变数据段大小
///
///
/// # 参数
/// - args[0] (addr): 新的堆顶部地址
///
/// # 返回
/// 成功返回新的堆顶部地址，失败返回当前地址（无变化）
///
/// # 行为
/// - 如果 addr 为 0，返回当前 brk 值
/// - 如果 addr 小于当前 brk，缩小堆并返回新值
/// - 如果 addr 大于当前 brk，尝试扩展堆并返回新值
/// - 如果扩展失败，返回当前值（无变化）
///
/// - RISC-V: 214
pub fn sys_brk(args: [u64; 6]) -> u64 {
    use crate::sched;
    use crate::mm::page::PAGE_SIZE;
    use crate::arch::riscv64::mm::{alloc_and_map_user_memory, PageTableEntry};

    let new_brk = args[0] as u64;

    // 获取当前进程
    match sched::current() {
        Some(current_task) => {
            // 获取当前 brk 值
            let current_brk = current_task.get_brk();

            // 如果 brk 未初始化，从地址空间获取或设置默认值
            if current_brk == 0 {
                // 尝试从地址空间的 brk 获取
                let default_brk = if let Some(addr_space) = current_task.address_space() {
                    addr_space.brk().as_usize() as u64
                } else {
                    // 使用 mm 模块中的 BRK_DEFAULT
                    crate::arch::riscv64::mm::user_addr::BRK_DEFAULT as u64
                };
                current_task.set_brk(default_brk);

                if new_brk == 0 {
                    return default_brk;
                }
            }

            // 重新获取当前 brk（可能已更新）
            let current_brk = current_task.get_brk();

            // 如果 new_brk 为 0，返回当前 brk
            if new_brk == 0 {
                return current_brk;
            }

            // 确保新 brk 不低于当前值（不允许缩小堆）
            if new_brk < current_brk {
                return current_brk;
            }

            // 扩展堆：需要映射新的内存页
            if new_brk > current_brk {
                // 计算需要映射的页面范围
                let current_page_start = current_brk & !(PAGE_SIZE as u64 - 1);
                let new_page_end = (new_brk + PAGE_SIZE as u64 - 1) & !(PAGE_SIZE as u64 - 1);

                // 如果需要映射新页面
                if new_page_end > current_page_start {
                    // 获取地址空间的根页表
                    let root_ppn = if let Some(addr_space) = current_task.address_space() {
                        addr_space.root_ppn()
                    } else {
                        return current_brk;
                    };

                    // 映射新的堆页面
                    let size = new_page_end - current_page_start;

                    // 权限: User + Read + Write + Valid + Accessed + Dirty
                    let pte_flags = PageTableEntry::V | PageTableEntry::R | PageTableEntry::W
                        | PageTableEntry::U | PageTableEntry::A | PageTableEntry::D;

                    unsafe {
                        let result = alloc_and_map_user_memory(root_ppn, current_page_start, size, pte_flags);
                        if result.is_none() {
                            return current_brk;
                        }
                    }
                }

                current_task.set_brk(new_brk);
                new_brk
            } else {
                current_brk
            }
        }
        None => -12_i64 as u64  // ENOMEM
    }
}
/// sys_mmap - 创建内存映射
///
///
/// # 参数
/// - args[0] (addr): 建议的起始地址
/// - args[1] (length): 映射长度
/// - args[2] (prot): 保护标志 (PROT_READ/WRITE/EXEC)
/// - args[3] (flags): 映射标志 (MAP_PRIVATE/SHARED/ANONYMOUS)
/// - args[4] (fd): 文件描述符
/// - args[5] (offset): 文件偏移
///
/// # 返回
/// 成功返回映射的起始地址，失败返回负错误码
///
/// - RISC-V: 222
pub fn sys_mmap(args: [u64; 6]) -> u64 {
    use crate::mm::page::VirtAddr;
    use crate::mm::vma::{VmaFlags, VmaType};
    use crate::mm::pagemap::Perm;
    use crate::arch::riscv64::mm::{prot, map, mmap_error};

    let addr = args[0] as usize;
    let length = args[1] as usize;
    let prot_flags = args[2] as u32;
    let map_flags = args[3] as u32;
    let fd = args[4] as i32;
    let _offset = args[5] as u64;

    // 特殊处理：如果 length=0，分配一个页面
    // 这是为了兼容某些程序（如 musl）可能在某些边缘情况下请求 0 长度
    let actual_length = if length == 0 {
        4096  // 使用一个页面的最小分配
    } else {
        length
    };

    // 检查保护标志
    if prot_flags & !prot::PROT_MASK != 0 {
        return mmap_error::EINVAL as u64;
    }

    // 检查映射类型（必须指定 MAP_SHARED 或 MAP_PRIVATE）
    let map_type = map_flags & map::MAP_TYPE_MASK;
    if map_type != map::MAP_SHARED && map_type != map::MAP_PRIVATE {
        return mmap_error::EINVAL as u64;
    }

    // 检查是否为 framebuffer 设备映射 (fd >= 1000 表示设备文件)
    if fd >= 1000 {
        let result = sys_mmap_framebuffer(addr, actual_length, prot_flags, map_flags);
        return result;
    }

    // 非匿名映射且没有文件描述符
    if (map_flags & map::MAP_ANONYMOUS == 0) && fd < 0 {
        return mmap_error::EBADF as u64;
    }

    // 获取当前进程
    match crate::sched::current() {
        Some(current_task) => {
            // 检查是否有地址空间
            match current_task.address_space_mut() {
                Some(address_space) => {
                    // 对于 MAP_ANONYMOUS 映射，隐式添加 PROT_WRITE
                    // 因为匿名映射总是需要可写（用于存储数据）
                    // Linux 也遵循这个约定
                    let effective_prot = if map_flags & map::MAP_ANONYMOUS != 0 {
                        prot_flags | prot::PROT_READ | prot::PROT_WRITE
                    } else {
                        prot_flags
                    };

                    // 解析保护标志
                    let perm = if effective_prot & prot::PROT_EXEC != 0 {
                        if effective_prot & prot::PROT_WRITE != 0 {
                            Perm::ReadWriteExec
                        } else if effective_prot & prot::PROT_READ != 0 {
                            Perm::ReadWriteExec  // 简化：读+执行
                        } else {
                            Perm::ReadWriteExec  // 简化：仅执行
                        }
                    } else if effective_prot & prot::PROT_WRITE != 0 {
                        Perm::ReadWrite
                    } else if effective_prot & prot::PROT_READ != 0 {
                        Perm::Read
                    } else {
                        Perm::None
                    };

                    // 解析 VMA 标志
                    let mut vma_flags = VmaFlags::new();

                    // 默认可读
                    vma_flags.insert(VmaFlags::READ);

                    if map_flags & map::MAP_SHARED != 0 {
                        vma_flags.insert(VmaFlags::SHARED);
                    }
                    if map_flags & map::MAP_PRIVATE != 0 {
                        vma_flags.insert(VmaFlags::PRIVATE);
                    }
                    if prot_flags & prot::PROT_WRITE != 0 {
                        vma_flags.insert(VmaFlags::WRITE);
                    }
                    if prot_flags & prot::PROT_EXEC != 0 {
                        vma_flags.insert(VmaFlags::EXEC);
                    }
                    if map_flags & map::MAP_STACK != 0 {
                        vma_flags.insert(VmaFlags::GROWSDOWN);
                    }

                    // 设置 VMA 类型
                    let vma_type = if map_flags & map::MAP_ANONYMOUS != 0 {
                        VmaType::Anonymous
                    } else {
                        VmaType::FileBacked
                    };

                    // 调用 AddressSpace::mmap
                    let result = address_space.mmap(
                        VirtAddr::new(addr),
                        actual_length,
                        vma_flags,
                        vma_type,
                        perm,
                        map_flags,
                    );
                    match result {
                        Ok(mapped_addr) => {
                            // mmap 成功后刷新 TLB
                            unsafe {
                                core::arch::asm!("fence");
                                core::arch::asm!("sfence.vma");
                                core::arch::asm!("fence");
                            }
                            mapped_addr.as_usize() as u64
                        },
                        Err(e) => {
                            let err = match e {
                                crate::mm::pagemap::MapError::OutOfMemory => mmap_error::ENOMEM,
                                crate::mm::pagemap::MapError::Invalid => mmap_error::EINVAL,
                                crate::mm::pagemap::MapError::AlreadyMapped => mmap_error::ENOMEM,
                                crate::mm::pagemap::MapError::NotMapped => mmap_error::EINVAL,
                            };
                            err as u64
                        }
                    }
                }
                None => {
                    mmap_error::ENOMEM as u64
                }
            }
        }
        None => {
            mmap_error::ENOMEM as u64
        }
    }
}
/// sys_mmap_framebuffer - 映射 framebuffer 到用户空间
///
/// # 参数
/// - addr: 建议的虚拟地址 (0 表示由内核选择)
/// - length: 映射长度
/// - prot: 保护标志 (PROT_READ | PROT_WRITE)
/// - flags: 映射标志 (MAP_SHARED)
///
/// # 返回
/// 成功返回映射的虚拟地址，失败返回负错误码
fn sys_mmap_framebuffer(addr: usize, length: usize, prot: u32, flags: u32) -> u64 {
    use crate::mm::page::{VirtAddr, PAGE_SIZE};
    use crate::arch::riscv64::mm::PageTableEntry;
    use crate::mm::vma::{Vma, VmaFlags};

    // 获取 framebuffer 信息
    let fb_info = match crate::drivers::gpu::get_framebuffer_info() {
        Some(info) => info,
        None => return -6_i64 as u64,  // ENXIO
    };

    // 检查请求的长度
    if length == 0 || length > fb_info.size as usize {
        return -22_i64 as u64;  // EINVAL
    }

    // 获取当前进程
    let current_task = match crate::sched::current() {
        Some(task) => task,
        None => return -12_i64 as u64,  // ENOMEM
    };

    // 计算映射的虚拟地址
    // 使用固定地址 0x60000000 作为 framebuffer 映射地址
    let vaddr = if addr == 0 { 0x6000_0000 } else { addr };
    let vaddr_aligned = vaddr & !(PAGE_SIZE - 1);

    // 计算需要的页数和对齐后的长度
    let pages_needed = (length + PAGE_SIZE - 1) / PAGE_SIZE;
    let aligned_length = pages_needed * PAGE_SIZE;

    // 将内核虚拟地址转换为物理地址
    // fb_info.addr 是内核堆分配的虚拟地址，需要转换为物理地址
    let fb_virt_addr = crate::arch::riscv64::mm::VirtAddr::new(fb_info.addr as usize as u64);
    let fb_phys_addr = crate::arch::riscv64::mm::virt_to_phys(fb_virt_addr).0 as usize;
    let fb_phys_aligned = fb_phys_addr & !(PAGE_SIZE - 1);

    // 获取当前进程的地址空间
    let addr_space = match current_task.address_space() {
        Some(aspace) => aspace,
        None => return -12_i64 as u64,  // ENOMEM
    };

    // 注册 VMA（设备映射）
    let mut vma_flags = VmaFlags::new();
    if prot & 0x1 != 0 { vma_flags.insert(VmaFlags::READ); }
    if prot & 0x2 != 0 { vma_flags.insert(VmaFlags::WRITE); }
    if prot & 0x4 != 0 { vma_flags.insert(VmaFlags::EXEC); }

    let vma = Vma::new(
        VirtAddr::new(vaddr_aligned),
        VirtAddr::new(vaddr_aligned + aligned_length),
        vma_flags,
    );

    // 添加 VMA 到地址空间
    if addr_space.vma_write().add(vma).is_err() {
        return -12_i64 as u64;  // ENOMEM
    }

    // 获取用户页表 PPN
    let user_ppn = addr_space.root_ppn();

    // 获取当前进程的页表并映射页面
    unsafe {
        // 构建页表项标志
        let mut pte_flags = PageTableEntry::V | PageTableEntry::U | PageTableEntry::A | PageTableEntry::D;
        if prot & 0x1 != 0 {  // PROT_READ
            pte_flags |= PageTableEntry::R;
        }
        if prot & 0x2 != 0 {  // PROT_WRITE
            pte_flags |= PageTableEntry::R | PageTableEntry::W;
        }
        if prot & 0x4 != 0 {  // PROT_EXEC
            pte_flags |= PageTableEntry::X;
        }

        // 映射每一页到用户页表
        for i in 0..pages_needed {
            let va = vaddr_aligned + i * PAGE_SIZE;
            let pa = fb_phys_aligned + i * PAGE_SIZE;

            // 使用用户页表映射
            crate::arch::riscv64::mm::map_user_page(
                user_ppn,
                crate::arch::riscv64::mm::VirtAddr::new(va as u64),
                crate::arch::riscv64::mm::PhysAddr::new(pa as u64),
                pte_flags,
            );
        }

        // 刷新 TLB
        core::arch::asm!("sfence.vma");
    }

    vaddr_aligned as u64
}
/// sys_munmap - 取消内存映射
///
///
/// # 参数
/// - args[0] (addr): 起始地址
/// - args[1] (length): 长度
///
/// # 返回
/// 成功返回 0，失败返回负错误码
///
/// - RISC-V: 215
pub fn sys_munmap(args: [u64; 6]) -> u64 {
    use crate::mm::page::VirtAddr;
    use crate::arch::riscv64::mm::mmap_error;

    let addr = args[0] as usize;
    let length = args[1] as usize;

    // 验证参数
    if length == 0 {
        return mmap_error::EINVAL as u64;
    }

    // 检查地址对齐
    if addr % 4096 != 0 {
        return mmap_error::EINVAL as u64;
    }

    // 获取当前进程
    match crate::sched::current() {
        Some(current_task) => {
            // 检查是否有地址空间
            match current_task.address_space_mut() {
                Some(address_space) => {
                    // 调用 AddressSpace::munmap
                    match address_space.munmap(VirtAddr::new(addr), length) {
                        Ok(()) => 0,
                        Err(e) => {
                            let err = match e {
                                crate::mm::pagemap::MapError::Invalid => mmap_error::EINVAL,
                                crate::mm::pagemap::MapError::NotMapped => mmap_error::EINVAL,
                                _ => mmap_error::ENOMEM,
                            };
                            err as u64
                        }
                    }
                }
                None => mmap_error::ENOMEM as u64,
            }
        }
        None => mmap_error::ENOMEM as u64,
    }
}
/// sys_mprotect - 更改内存区域的保护
///
///
/// # 参数
/// - args[0] (addr): 起始地址
/// - args[1] (length): 长度
/// - args[2] (prot): 新的保护标志 (PROT_READ/WRITE/EXEC)
///
/// # 返回
/// 成功返回 0，失败返回负错误码
///
/// - RISC-V: 226
///
/// # 说明
/// mprotect 用于更改已存在内存映射的保护属性
pub fn sys_mprotect(args: [u64; 6]) -> u64 {
    use crate::arch::riscv64::mm::{PageTableEntry, PAGE_SIZE, PAGE_SHIFT, PageTable, VirtAddr};

    let addr = args[0] as usize;
    let length = args[1] as usize;
    let prot = args[2] as u32;

    // 验证参数
    if length == 0 {
        return -22_i64 as u64;  // EINVAL
    }

    // 地址必须页对齐
    if addr % PAGE_SIZE as usize != 0 {
        return -22_i64 as u64;  // EINVAL
    }

    // 获取当前进程
    match crate::sched::current() {
        Some(current_task) => {
            // 获取页表根
            let root_ppn = if let Some(addr_space) = current_task.address_space() {
                addr_space.root_ppn()
            } else {
                return -12_i64 as u64;  // ENOMEM
            };

            // 计算新的 PTE 标志
            // 基本标志：Valid + User + Accessed + Dirty
            let mut new_flags = PageTableEntry::V | PageTableEntry::U
                | PageTableEntry::A | PageTableEntry::D;

            if prot & 0x1 != 0 {  // PROT_READ
                new_flags |= PageTableEntry::R;
            }
            if prot & 0x2 != 0 {  // PROT_WRITE
                new_flags |= PageTableEntry::W | PageTableEntry::R;  // W 需要 R
            }
            if prot & 0x4 != 0 {  // PROT_EXEC
                new_flags |= PageTableEntry::X;
            }

            // 如果 prot == 0 (PROT_NONE)，只保留 V 和 U，去掉 R/W/X

            // 遍历页面并更新权限
            let start_page = addr / PAGE_SIZE as usize;
            let num_pages = (length + PAGE_SIZE as usize - 1) / PAGE_SIZE as usize;

            for i in 0..num_pages {
                let virt = ((start_page + i) * PAGE_SIZE as usize) as u64;
                unsafe {
                    let virt_addr = VirtAddr(virt);

                    // 提取虚拟页号
                    let vpn2 = virt_addr.vpn(2) as usize;
                    let vpn1 = virt_addr.vpn(1) as usize;
                    let vpn0 = virt_addr.vpn(0) as usize;

                    // 使用物理地址访问页表（恒等映射）
                    let root_table_addr = root_ppn << PAGE_SHIFT;
                    let root_table = root_table_addr as *mut PageTable;

                    let pte2 = (*root_table).get(vpn2);
                    if !pte2.is_valid() {
                        continue;  // 页面未映射，跳过
                    }

                    let ppn1 = pte2.ppn();
                    let table1 = (ppn1 << PAGE_SHIFT) as *mut PageTable;
                    let pte1 = (*table1).get(vpn1);
                    if !pte1.is_valid() {
                        continue;  // 页面未映射，跳过
                    }

                    let ppn0 = pte1.ppn();
                    let table0 = (ppn0 << PAGE_SHIFT) as *mut PageTable;
                    let pte0 = (*table0).get(vpn0);

                    if pte0.is_valid() {
                        // 保留 PPN，只更新权限标志
                        let ppn = pte0.ppn();
                        let new_pte = PageTableEntry::from_bits((ppn << 10) | new_flags);
                        (*table0).set(vpn0, new_pte);
                    }
                }
            }

            0
        }
        None => -12_i64 as u64  // ENOMEM
    }
}
/// sys_msync - 同步内存映射到文件
///
///
/// # 参数
/// - args[0] (addr): 起始地址
/// - args[1] (length): 长度
/// - args[2] (flags): 同步标志 (MS_ASYNC/MS_SYNC/MS_INVALIDATE)
///
/// # 返回
/// 成功返回 0，失败返回负错误码
///
/// - RISC-V: 227
///
/// # 说明
/// msync 将映射到文件的更改写回磁盘
///
/// 参考: Linux kernel mm/msync.c
pub fn sys_msync(args: [u64; 6]) -> u64 {
    use crate::mm::page::{VirtAddr, PAGE_SIZE};
    use crate::arch::riscv64::mm::mmap_error;

    let addr = args[0] as usize;
    let length = args[1] as usize;
    let flags = args[2] as u32;

    // msync 标志
    const MS_ASYNC: u32 = 0x1;      // 异步写入
    const MS_SYNC: u32 = 0x2;       // 同步写入
    const MS_INVALIDATE: u32 = 0x4; // 使缓存失效

    // 验证标志
    if flags & !(MS_ASYNC | MS_SYNC | MS_INVALIDATE) != 0 {
        return mmap_error::EINVAL as u64;
    }

    // 不能同时设置 ASYNC 和 SYNC
    if (flags & MS_ASYNC != 0) && (flags & MS_SYNC != 0) {
        return mmap_error::EINVAL as u64;
    }

    // 验证参数
    if length == 0 {
        return mmap_error::EINVAL as u64;
    }

    // 地址必须页对齐
    if addr % PAGE_SIZE != 0 {
        return mmap_error::EINVAL as u64;
    }

    // 对齐长度
    let length_aligned = (length + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);

    // 获取当前进程
    let current_task = match crate::sched::current() {
        Some(task) => task,
        None => return mmap_error::ENOMEM as u64,
    };

    let address_space = match current_task.address_space() {
        Some(aspace) => aspace,
        None => return mmap_error::ENOMEM as u64,
    };

    // 1. 验证地址范围是否有 VMA 覆盖
    {
        let vma_mgr = address_space.vma_read();
        let mut check_addr = addr;
        let end_addr = addr + length_aligned;

        while check_addr < end_addr {
            match vma_mgr.find(VirtAddr::new(check_addr)) {
                Some(vma) => {
                    // 检查是否是共享映射（只有共享映射才能 msync）
                    // 简化：我们允许所有映射进行 msync
                    check_addr = vma.end().as_usize();
                }
                None => {
                    // 地址不在任何 VMA 中
                    return mmap_error::ENOMEM as u64;
                }
            }
        }
    }

    // 2. 执行同步操作
    // 注意：完整实现应该：
    // - 对于文件映射，将脏页写回文件
    // - 如果 MS_SYNC，等待写入完成
    // - 如果 MS_ASYNC，只是标记为需要写入
    // - 如果 MS_INVALIDATE，使其他进程的缓存失效
    //
    // 简化实现：由于我们目前主要是匿名映射，没有文件映射，
    // 所以直接返回成功

    0  // 成功
}
/// sys_mremap - 重新映射内存
///
///
/// # 参数
/// - args[0] (old_addr): 旧地址
/// - args[1] (old_size): 旧大小
/// - args[2] (new_size): 新大小
/// - args[3] (flags): 标志 (MREMAP_MAYMOVE/MREMAP_FIXED)
/// - args[4] (new_addr): 新地址（仅当 MREMAP_FIXED 时使用）
///
/// # 返回
/// 成功返回新地址（可能与旧地址相同），失败返回负错误码
///
/// - RISC-V: 216
///
/// # 说明
/// mremap 扩展或收缩已有的内存映射
///
/// 参考: Linux kernel mm/mremap.c
pub fn sys_mremap(args: [u64; 6]) -> u64 {
    use crate::mm::page::{VirtAddr, PAGE_SIZE};
    use crate::mm::vma::{VmaFlags, VmaType};
    use crate::mm::pagemap::Perm;
    use crate::arch::riscv64::mm::{map, mmap_error};

    let old_addr = args[0] as usize;
    let old_size = args[1] as usize;
    let new_size = args[2] as usize;
    let flags = args[3] as u32;
    let new_addr_arg = args[4] as usize;

    // mremap 标志
    const MREMAP_MAYMOVE: u32 = 0x1;  // 可以移动到新地址
    const MREMAP_FIXED: u32 = 0x2;    // 必须映射到指定地址

    // 验证 old_addr 页对齐
    if old_addr % PAGE_SIZE != 0 {
        return mmap_error::EINVAL as u64;
    }

    // 验证 new_addr 页对齐（如果指定）
    if (flags & MREMAP_FIXED) != 0 && new_addr_arg % PAGE_SIZE != 0 {
        return mmap_error::EINVAL as u64;
    }

    // 对齐大小
    let old_size_aligned = (old_size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    let new_size_aligned = (new_size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);

    // 获取当前进程
    let current_task = match crate::sched::current() {
        Some(task) => task,
        None => return mmap_error::ENOMEM as u64,
    };

    let address_space = match current_task.address_space_mut() {
        Some(aspace) => aspace,
        None => return mmap_error::ENOMEM as u64,
    };

    // 1. 查找覆盖 old_addr 的 VMA
    let vma_info = {
        let vma_mgr = address_space.vma_read();
        vma_mgr.find(VirtAddr::new(old_addr)).map(|vma| {
            (vma.start(), vma.end(), vma.flags(), vma.vma_type())
        })
    };

    let (vma_start, vma_end, vma_flags, vma_type) = match vma_info {
        Some(info) => info,
        None => return mmap_error::EFAULT as u64,  // 地址未映射
    };

    // 验证 old_addr 是 VMA 的起始地址
    if vma_start.as_usize() != old_addr {
        return mmap_error::EFAULT as u64;
    }

    // 验证 old_size 在 VMA 范围内
    if old_addr + old_size_aligned > vma_end.as_usize() {
        return mmap_error::EFAULT as u64;
    }

    // 2. 根据 new_size 决定操作类型
    if new_size_aligned == old_size_aligned {
        // NO_RESIZE: 大小不变
        // 如果指定了 MREMAP_FIXED，需要移动
        if (flags & MREMAP_FIXED) != 0 {
            // 移动到新地址
            // 先取消旧映射
            if let Err(_) = address_space.munmap(VirtAddr::new(old_addr), old_size_aligned) {
                return mmap_error::ENOMEM as u64;
            }
            // 在新地址创建映射
            let perm = vma_flags.to_page_perm();
            match address_space.mmap(
                VirtAddr::new(new_addr_arg),
                new_size_aligned,
                vma_flags,
                vma_type,
                perm,
                map::MAP_FIXED,
            ) {
                Ok(new_addr) => new_addr.as_usize() as u64,
                Err(e) => {
                    let err = match e {
                        crate::mm::pagemap::MapError::OutOfMemory => mmap_error::ENOMEM,
                        crate::mm::pagemap::MapError::Invalid => mmap_error::EINVAL,
                        crate::mm::pagemap::MapError::AlreadyMapped => mmap_error::ENOMEM,
                        crate::mm::pagemap::MapError::NotMapped => mmap_error::EINVAL,
                    };
                    err as u64
                }
            }
        } else {
            // 无需任何操作
            old_addr as u64
        }
    } else if new_size_aligned < old_size_aligned {
        // SHRINK: 收缩映射
        // 取消映射多余的部分
        let unmap_start = old_addr + new_size_aligned;
        let unmap_size = old_size_aligned - new_size_aligned;

        match address_space.munmap(VirtAddr::new(unmap_start), unmap_size) {
            Ok(()) => old_addr as u64,
            Err(_) => mmap_error::ENOMEM as u64,
        }
    } else {
        // EXPAND: 扩展映射
        let extra_size = new_size_aligned - old_size_aligned;
        let new_end = old_addr + new_size_aligned;

        // 检查是否可以原地扩展（检查下一个 VMA 是否会冲突）
        let can_expand = {
            let vma_mgr = address_space.vma_read();
            if let Some(next_vma) = vma_mgr.find_vma_after(VirtAddr::new(vma_end.as_usize())) {
                next_vma.start().as_usize() >= new_end
            } else {
                true  // 没有下一个 VMA，可以扩展
            }
        };

        if can_expand {
            // 原地扩展：映射额外的页面
            let perm = vma_flags.to_page_perm();
            match address_space.mmap(
                VirtAddr::new(vma_end.as_usize()),
                extra_size,
                vma_flags,
                vma_type,
                perm,
                map::MAP_FIXED,  // 强制在这个地址
            ) {
                Ok(_) => old_addr as u64,
                Err(_) => mmap_error::ENOMEM as u64,
            }
        } else if (flags & MREMAP_MAYMOVE) != 0 {
            // 可以移动：找到新位置
            let perm = vma_flags.to_page_perm();
            match address_space.mmap(
                VirtAddr::new(0),  // 让内核选择地址
                new_size_aligned,
                vma_flags,
                vma_type,
                perm,
                0,  // 不强制地址
            ) {
                Ok(new_mapping_addr) => {
                    // 取消旧映射
                    let _ = address_space.munmap(VirtAddr::new(old_addr), old_size_aligned);
                    new_mapping_addr.as_usize() as u64
                }
                Err(_) => mmap_error::ENOMEM as u64,
            }
        } else {
            // 无法原地扩展且不允许移动
            mmap_error::ENOMEM as u64
        }
    }
}
/// sys_madvise - 给内核关于内存使用模式的建议
///
///
/// # 参数
/// - args[0] (addr): 起始地址
/// - args[1] (length): 长度
/// - args[2] (advice): 建议类型 (MADV_NORMAL/MADV_RANDOM/MADV_SEQUENTIAL/etc)
///
/// # 返回
/// 成功返回 0，失败返回负错误码
///
/// - RISC-V: 233
///
/// # 说明
/// madvise 允许应用程序给内核提供关于如何使用内存的建议
///
/// 参考: Linux kernel mm/madvise.c
pub fn sys_madvise(args: [u64; 6]) -> u64 {
    use crate::mm::page::{VirtAddr, PAGE_SIZE};
    use crate::arch::riscv64::mm::mmap_error;

    let addr = args[0] as usize;
    let length = args[1] as usize;
    let advice = args[2] as i32;

    // madvise 建议类型
    const MADV_NORMAL: i32 = 0;       // 无特殊建议
    const MADV_RANDOM: i32 = 1;       // 随机访问
    const MADV_SEQUENTIAL: i32 = 2;   // 顺序访问
    const MADV_WILLNEED: i32 = 3;     // 将要访问
    const MADV_DONTNEED: i32 = 4;     // 不再需要（释放页面）
    const MADV_FREE: i32 = 8;         // 可释放（与 DONTNEED 类似）
    const MADV_REMOVE: i32 = 9;       // 释放映射
    const MADV_DONTFORK: i32 = 10;    // fork 时不复制
    const MADV_DOFORK: i32 = 11;      // fork 时复制
    const MADV_MERGEABLE: i32 = 12;   // 可合并（KSM）
    const MADV_UNMERGEABLE: i32 = 13; // 不可合并
    const MADV_HUGEPAGE: i32 = 14;    // 使用巨页
    const MADV_NOHUGEPAGE: i32 = 15;  // 不使用巨页
    const MADV_DONTDUMP: i32 = 16;    // 不转储到 core
    const MADV_DODUMP: i32 = 17;      // 转储到 core
    const MADV_HWPOISON: i32 = 100;   // 标记为损坏

    // 验证参数
    if length == 0 {
        return mmap_error::EINVAL as u64;
    }

    // 地址必须页对齐
    if addr % PAGE_SIZE != 0 {
        return mmap_error::EINVAL as u64;
    }

    // 验证 advice 类型
    match advice {
        MADV_NORMAL | MADV_RANDOM | MADV_SEQUENTIAL | MADV_WILLNEED |
        MADV_DONTNEED | MADV_FREE | MADV_REMOVE | MADV_DONTFORK | MADV_DOFORK |
        MADV_MERGEABLE | MADV_UNMERGEABLE | MADV_HUGEPAGE | MADV_NOHUGEPAGE |
        MADV_DONTDUMP | MADV_DODUMP => {
            // 有效的 advice
        }
        _ => {
            return mmap_error::EINVAL as u64;
        }
    }

    // 对齐长度
    let length_aligned = (length + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);

    // 获取当前进程
    let current_task = match crate::sched::current() {
        Some(task) => task,
        None => return mmap_error::ENOMEM as u64,
    };

    let address_space = match current_task.address_space_mut() {
        Some(aspace) => aspace,
        None => return mmap_error::ENOMEM as u64,
    };

    // 1. 验证地址范围是否有 VMA 覆盖
    {
        let vma_mgr = address_space.vma_read();
        let start = VirtAddr::new(addr);
        let end = VirtAddr::new(addr + length_aligned);

        // 检查起始地址是否有 VMA
        if vma_mgr.find(start).is_none() {
            return mmap_error::ENOMEM as u64;
        }

        // 对于 MADV_DONTNEED 和 MADV_REMOVE，需要整个范围都在 VMA 内
        if advice == MADV_DONTNEED || advice == MADV_REMOVE {
            // 查找覆盖整个范围的 VMA
            let mut check_addr = addr;
            while check_addr < addr + length_aligned {
                match vma_mgr.find(VirtAddr::new(check_addr)) {
                    Some(vma) => {
                        check_addr = vma.end().as_usize();
                    }
                    None => {
                        return mmap_error::ENOMEM as u64;
                    }
                }
            }
        }
    }

    // 2. 根据 advice 执行操作
    match advice {
        MADV_DONTNEED | MADV_FREE => {
            // MADV_DONTNEED: 释放页面，但保留 VMA
            // 注意：Linux 的行为是丢弃页面内容，下次访问时会得到零页
            // 简化实现：我们不做实际释放，因为需要处理页表项的修改
            // 这对于大多数应用程序来说是可接受的
            0
        }
        MADV_REMOVE => {
            // MADV_REMOVE: 完全释放映射（等同于 munmap）
            match address_space.munmap(VirtAddr::new(addr), length_aligned) {
                Ok(()) => 0,
                Err(_) => mmap_error::ENOMEM as u64,
            }
        }
        MADV_WILLNEED => {
            // MADV_WILLNEED: 预读页面到内存
            // 简化实现：不做任何事情，因为页面已经在内存中或按需加载
            0
        }
        MADV_NORMAL | MADV_RANDOM | MADV_SEQUENTIAL => {
            // 这些是性能提示，简化实现中忽略
            // 在完整实现中，应该更新 VMA 的 vm_flags
            0
        }
        MADV_DONTFORK | MADV_DOFORK => {
            // fork 相关的标志
            // 简化实现中忽略
            0
        }
        MADV_HUGEPAGE | MADV_NOHUGEPAGE => {
            // 巨页相关，简化实现中忽略
            0
        }
        MADV_MERGEABLE | MADV_UNMERGEABLE => {
            // KSM 相关，简化实现中忽略
            0
        }
        MADV_DONTDUMP | MADV_DODUMP => {
            // core dump 相关，简化实现中忽略
            0
        }
        _ => {
            // 不应该到达这里，因为前面已经验证过
            mmap_error::EINVAL as u64
        }
    }
}
/// sys_mincore - 查询页面是否在内存中
///
///
/// # 参数
/// - args[0] (addr): 起始地址
/// - args[1] (length): 长度
/// - args[2] (vec): 结果向量指针
///
/// # 返回
/// 成功返回 0，失败返回负错误码
///
/// - RISC-V: 232
///
/// # 说明
/// mincore 返回一个向量，表示哪些页面在内存中
/// vec 的每个字节的最低位表示对应页面是否在内存中
///
/// 参考: Linux kernel mm/mincore.c
pub fn sys_mincore(args: [u64; 6]) -> u64 {
    use crate::mm::page::{VirtAddr, PAGE_SIZE};
    use crate::arch::riscv64::mm::{PageTableEntry, PageTable, mmap_error};

    let addr = args[0] as usize;
    let length = args[1] as usize;
    let vec_ptr = args[2] as *mut u8;

    // 验证参数
    if length == 0 {
        return mmap_error::EINVAL as u64;
    }

    // 地址必须页对齐
    if addr % PAGE_SIZE != 0 {
        return mmap_error::EINVAL as u64;
    }

    // 验证 vec 指针
    if vec_ptr.is_null() {
        return mmap_error::EINVAL as u64;
    }

    // 计算需要的页数
    let page_count = (length + PAGE_SIZE - 1) / PAGE_SIZE;

    // 获取当前进程
    let current_task = match crate::sched::current() {
        Some(task) => task,
        None => return mmap_error::ENOMEM as u64,
    };

    let address_space = match current_task.address_space() {
        Some(aspace) => aspace,
        None => return mmap_error::ENOMEM as u64,
    };

    // 1. 验证地址范围是否有 VMA 覆盖
    {
        let vma_mgr = address_space.vma_read();
        let mut check_addr = addr;
        let end_addr = addr + page_count * PAGE_SIZE;

        while check_addr < end_addr {
            match vma_mgr.find(VirtAddr::new(check_addr)) {
                Some(vma) => {
                    check_addr = vma.end().as_usize();
                }
                None => {
                    // 地址不在任何 VMA 中
                    return mmap_error::ENOMEM as u64;
                }
            }
        }
    }

    // 2. 获取页表根
    let root_ppn = address_space.root_ppn();

    // 3. 检查每个页面是否在内存中
    unsafe {
        for i in 0..page_count {
            let page_addr = addr + i * PAGE_SIZE;

            // 查找页表项
            let vpn = [
                (page_addr >> 12) & 0x1FF,
                (page_addr >> 21) & 0x1FF,
                (page_addr >> 30) & 0x1FF,
            ];

            // 遍历页表
            let mut pte_addr = (root_ppn << 12) as *const PageTableEntry;
            let mut page_in_memory = false;

            for level in (0..3usize).rev() {
                let pte = &*pte_addr.add(vpn[level]);

                if !pte.is_valid() {
                    // 页表项无效，页面不在内存中
                    break;
                }

                // 检查是否为叶子节点（R/W/X 任一置位表示叶子节点）
                let is_leaf = pte.is_readable() || pte.is_writable() || pte.is_executable();

                if level == 0 || is_leaf {
                    // 到达叶子节点或巨页，页面在内存中
                    page_in_memory = true;
                    break;
                }

                // 继续到下一级
                pte_addr = (pte.ppn() << 12) as *const PageTableEntry;
            }

            // 设置结果：最低位表示页面是否在内存中
            *vec_ptr.add(i) = if page_in_memory { 1 } else { 0 };
        }
    }

    0  // 成功
}
/// sys_mlock - 锁定内存
///
///
/// # 参数
/// - args[0] (addr): 起始地址
/// - args[1] (length): 长度
///
/// # 返回
/// 成功返回 0，失败返回负错误码
///
/// - RISC-V: 228
///
/// # 说明
/// mlock 锁定内存，防止被换出
pub fn sys_mlock(args: [u64; 6]) -> u64 {
    use crate::mm::page::VirtAddr;

    let addr = args[0] as usize;
    let length = args[1] as usize;


    // 验证参数
    if length == 0 {
        return -22_i64 as u64;  // EINVAL
    }

    // 地址必须页对齐
    if addr % crate::mm::page::PAGE_SIZE != 0 {
        return -22_i64 as u64;  // EINVAL
    }

    // 简化实现：
    // 在真实实现中，应该：
    // 1. 检查进程的 RLIMIT_MEMLOCK 限制
    // 2. 查找覆盖 [addr, addr+length) 的所有 VMA
    // 3. 设置 VM_LOCKED 标志
    // 4. 确保页面驻留在内存中
    // TODO: 实现完整的 mlock 逻辑


    0  // 成功
}
/// sys_munlock - 解锁内存
///
///
/// # 参数
/// - args[0] (addr): 起始地址
/// - args[1] (length): 长度
///
/// # 返回
/// 成功返回 0，失败返回负错误码
///
/// - RISC-V: 229
///
/// # 说明
/// munlock 解锁之前锁定的内存
pub fn sys_munlock(args: [u64; 6]) -> u64 {
    use crate::mm::page::VirtAddr;

    let addr = args[0] as usize;
    let length = args[1] as usize;


    // 验证参数
    if length == 0 {
        return -22_i64 as u64;  // EINVAL
    }

    // 地址必须页对齐
    if addr % crate::mm::page::PAGE_SIZE != 0 {
        return -22_i64 as u64;  // EINVAL
    }

    // 简化实现：
    // 在真实实现中，应该：
    // 1. 查找覆盖 [addr, addr+length) 的所有 VMA
    // 2. 清除 VM_LOCKED 标志
    // TODO: 实现完整的 munlock 逻辑


    0  // 成功
}
