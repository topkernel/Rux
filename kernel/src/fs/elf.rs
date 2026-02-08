//! ELF 文件格式解析和加载
//!
//! 完全遵循 Linux 内核的 ELF 加载器设计 (fs/binfmt_elf.c)
//!
//! 支持的 ELF 格式：
//! - 64-bit ELF (ELF64)
//! - 小端序 (Little Endian)
//! - 可执行文件 (ET_EXEC)
//! - 动态链接器支持（未来）

use core::mem::size_of;
use core::ptr;
extern crate alloc;

/// ELF 识别 magic number
pub const ELF_MAGIC: [u8; 4] = [0x7f, b'E', b'L', b'F'];

/// ELF 文件头 (64-bit)
///
/// 对应 ELF64_Ehdr (include/uapi/linux/elf.h)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Elf64Ehdr {
    /// Magic number 和其他信息
    pub e_ident: [u8; 16],
    /// 文件类型
    pub e_type: u16,
    /// 机器类型
    pub e_machine: u16,
    /// 版本
    pub e_version: u32,
    /// 入口点地址
    pub e_entry: u64,
    /// 程序头表偏移
    pub e_phoff: u64,
    /// 节头表偏移
    pub e_shoff: u64,
    /// 处理器特定标志
    pub e_flags: u32,
    /// ELF 头大小
    pub e_ehsize: u16,
    /// 程序头表条目大小
    pub e_phentsize: u16,
    /// 程序头表条目数量
    pub e_phnum: u16,
    /// 节头表条目大小
    pub e_shentsize: u16,
    /// 节头表条目数量
    pub e_shnum: u16,
    /// 节头字符串表索引
    pub e_shstrndx: u16,
}

/// ELF 文件类型
#[repr(u16)]
#[derive(Debug, Copy, Clone, PartialEq)]
#[allow(non_camel_case_types)]
pub enum ElfType {
    /// 未知类型
    ET_NONE = 0,
    /// 可重定位文件
    ET_REL = 1,
    /// 可执行文件
    ET_EXEC = 2,
    /// 共享目标文件
    ET_DYN = 3,
    /// 核心文件
    ET_CORE = 4,
}

/// ELF 机器类型
#[repr(u16)]
#[derive(Debug, Copy, Clone, PartialEq)]
#[allow(non_camel_case_types)]
pub enum ElfMachine {
    /// 无机器
    EM_NONE = 0,
    /// AT&T WE 32100
    EM_M32 = 1,
    /// SPARC
    EM_SPARC = 2,
    /// x86
    EM_386 = 3,
    /// Motorola 68000
    EM_68K = 4,
    /// Motorola 88000
    EM_88K = 5,
    /// Intel 80860
    EM_860 = 7,
    /// MIPS
    EM_MIPS = 8,
    /// IBM System/370
    EM_S370 = 9,
    /// MIPS RS3000 Little-endian
    EM_MIPS_RS3_LE = 10,
    /// Hewlett-Packard PA-RISC
    EM_PARISC = 15,
    /// Fujitsu VPP500
    EM_VPP500 = 17,
    /// Enhanced instruction set SPARC
    EM_SPARC32PLUS = 18,
    /// Intel 80960
    EM_960 = 19,
    /// PowerPC
    EM_PPC = 20,
    /// PowerPC 64-bit
    EM_PPC64 = 21,
    /// IBM S390
    EM_S390 = 22,
    /// IBM SPU/SPC
    EM_SPU = 23,
    /// NEC V800
    EM_V800 = 36,
    /// Fujitsu FR20
    EM_FR20 = 37,
    /// TRW RH-32
    EM_RH32 = 38,
    /// Motorola RCE
    EM_RCE = 39,
    /// ARM
    EM_ARM = 40,
    /// DEC Alpha
    EM_ALPHA = 41,
    /// Hitachi SH
    EM_SH = 42,
    /// SPARC-V9
    EM_SPARCV9 = 43,
    /// Siemens Tricore
    EM_TRICORE = 44,
    /// Argonaut RISC Core
    EM_ARC = 45,
    /// Hitachi H8/300
    EM_H8_300 = 46,
    /// Hitachi H8/300H
    EM_H8_300H = 47,
    /// Hitachi H8S
    EM_H8S = 48,
    /// Hemicycle
    EM_H8_500 = 49,
    /// Intel IA-64 processor architecture
    EM_IA_64 = 50,
    /// Stanford MIPS-X
    EM_MIPS_X = 51,
    /// Motorola ColdFire
    EM_COLDFIRE = 52,
    /// Motorola M68HC12
    EM_68HC12 = 53,
    /// Fujitsu MMA Multimedia Accelerator
    EM_MMA = 54,
    /// Siemens PCP
    EM_PCP = 55,
    /// Sony nCPU embedded RISC
    EM_NCPU = 56,
    /// Sony nCPU 20-bit
    EM_NDR1 = 57,
    /// Motorola Star*Core processor
    EM_STARCORE = 58,
    /// Toyota ME16 processor
    EM_ME16 = 59,
    /// STMicroelectronics ST100 processor
    EM_ST100 = 60,
    /// Advanced Logic Corp. TinyJ
    EM_TINYJ = 61,
    /// AMD x86-64 architecture
    EM_X86_64 = 62,
    /// Sony DSP Processor
    EM_PDSP = 63,
    /// Siemens FX66
    EM_FX66 = 66,
    /// STMicroelectronics ST9+ 8/16 mc
    EM_ST9PLUS = 67,
    /// STmicroelectronics ST7 8 bit mc
    EM_ST7 = 68,
    /// Motorola MC68HC16 Microcontroller
    EM_68HC16 = 69,
    /// Motorola MC68HC11 Microcontroller
    EM_68HC11 = 70,
    /// Motorola MC68HC08 Microcontroller
    EM_68HC08 = 71,
    /// Motorola MC68HC05 Microcontroller
    EM_68HC05 = 72,
    /// Silicon Graphics SVx
    EM_SVX = 73,
    /// STMicroelectronics ST19 8 bit mc
    EM_ST19 = 74,
    /// Digital VAX
    EM_VAX = 75,
    /// Axis Communications 32-bit embedded processor
    EM_CRIS = 76,
    /// Infineon Technologies 32-bit embedded processor
    EM_JAVELIN = 77,
    /// FirePath
    EM_FIREPATH = 78,
    /// LSI Logic 16-bit DSP Processor
    EM_ZSP = 79,
    /// MMIX
    EM_MMIX = 80,
    /// Harvard University machine-independent object files
    EM_HUANY = 81,
    /// SiTera Prism
    EM_PRISM = 82,
    /// Atmel AVR 8-bit microcontroller
    EM_AVR = 83,
    /// Fujitsu FR30
    EM_FR30 = 84,
    /// Mitsubishi D10V
    EM_D10V = 85,
    /// Mitsubishi D30V
    EM_D30V = 86,
    /// NEC v850
    EM_V850 = 87,
    /// Mitsubishi M32R
    EM_M32R = 88,
    /// Matsushita MN10300
    EM_MN10300 = 89,
    /// Matsushita MN10200
    EM_MN10200 = 90,
    /// picoJava
    EM_PJ = 91,
    /// OpenRISC 32-bit embedded processor
    EM_OPENRISC = 92,
    /// ARC International ARCompact
    EM_ARC_COMPACT = 93,
    /// Tensilica Xtensa Architecture
    EM_XTENSA = 94,
    /// Alphamosaic VideoCore
    EM_VIDEOCORE = 95,
    /// Thompson Multimedia General Purpose Processor
    EM_TMM_GPP = 96,
    /// National Semiconductor 32000
    EM_NS32K = 97,
    /// Tenor Network Technology TinyCPU
    EM_TPC = 98,
    /// Trebia SIP 32-bit
    EM_SNP1K = 99,
    /// STMicroelectronics ST200
    EM_ST200 = 100,
    /// Ubicom IP2xxx
    EM_IP2K = 101,
    /// MAX Processor
    EM_MAX = 102,
    /// National Semiconductor CompactRISC
    EM_CR = 103,
    /// Fujitsu F2MC16
    EM_F2MC16 = 104,
    /// Texas Instruments embedded microcontroller msp430
    EM_MSP430 = 105,
    /// Analog Devices Blackfin (DSP) processor
    EM_BLACKFIN = 106,
    /// S1C33 Embedded Epson SE
    EM_SE_C33 = 107,
    /// Sharp embedded microprocessor
    EM_SEP = 108,
    /// Arca RISC Microprocessor
    EM_ARCA = 109,
    /// Microprocessor Systems from Fujitsu
    EM_UNICORE = 110,
    /// eXcess: 64-bit CPU
    EM_EXCESS = 111,
    /// IXP12000
    EM_DXP = 112,
    /// Altera Nios II
    EM_ALTERA_NIOS2 = 113,
    /// ThreadX
    EM_CRX = 114,
    /// Standard Performance Corporation
    EM_XGATE = 115,
    /// Intel Timelay
    EM_C166 = 116,
    /// Renesas M16C series
    EM_M16C = 117,
    /// Microchip Technology dsPIC30F
    EM_DSPIC30F = 118,
    /// Freescale Communication Engine RISC core
    EM_CE = 119,
    /// Renesas M32C series
    EM_M32C = 120,
    /// Altium TSK3000
    EM_TSK3000 = 131,
    /// FenTeC A32K
    EM_E2K = 132,
    /// Alpha 8-bit
    EM_TS11 = 133,
    /// STMicroelectronics ST100 (duplicate from earlier)
    EM_ST100_2 = 134,
    /// Xilinx MicroBlaze
    EM_MICROBLAZE = 189,
    /// ARM 64-bit (AArch64)
    EM_AARCH64 = 183,
    /// RISC-V
    EM_RISCV = 243,
}

/// ELF 程序头 (64-bit)
///
/// 对应 ELF64_Phdr (include/uapi/linux/elf.h)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Elf64Phdr {
    /// 段类型
    pub p_type: u32,
    /// 段标志
    pub p_flags: u32,
    /// 段文件偏移
    pub p_offset: u64,
    /// 段虚拟地址
    pub p_vaddr: u64,
    /// 段物理地址
    pub p_paddr: u64,
    /// 段文件大小
    pub p_filesz: u64,
    /// 段内存大小
    pub p_memsz: u64,
    /// 段对齐
    pub p_align: u64,
}

/// 程序头段类型
#[repr(u32)]
#[derive(Debug, Copy, Clone, PartialEq)]
#[allow(non_camel_case_types)]
pub enum ElfPtType {
    /// 未使用的段
    PT_NULL = 0,
    /// 可加载段
    PT_LOAD = 1,
    /// 动态链接信息
    PT_DYNAMIC = 2,
    /// 解释器路径
    PT_INTERP = 3,
    /// 辅助信息
    PT_NOTE = 4,
    /// 未使用
    PT_SHLIB = 5,
    /// 程序头表本身
    PT_PHDR = 6,
    /// 线程局部存储
    PT_TLS = 7,
}

/// 程序头段标志
pub const PF_X: u32 = 0x1;  // 可执行
pub const PF_W: u32 = 0x2;  // 可写
pub const PF_R: u32 = 0x4;  // 可读

impl Elf64Ehdr {
    /// 从字节缓冲区解析 ELF 头
    pub unsafe fn from_bytes(data: &[u8]) -> Option<Elf64Ehdr> {
        use crate::console::putchar;
        const MSG: &[u8] = b"from_bytes: step";

        let emit = |step: u8, code: u8| {
            for &b in MSG { putchar(b); }
            putchar(b'0' + step);
            putchar(b':');
            putchar(b'0' + code);
            putchar(b'\n');
        };

        emit(1, 0); // entering

        // 检查最小长度
        if data.len() < size_of::<Elf64Ehdr>() {
            emit(2, 1);
            return None;
        }

        emit(3, 0); // len OK

        // 检查 magic number
        if &data[0..4] != ELF_MAGIC {
            emit(4, 1);
            return None;
        }

        emit(5, 0); // magic OK

        // 检查是否是 64-bit ELF
        if data[4] != 2 {
            emit(6, 1);
            return None;
        }

        emit(7, 0); // class OK

        // 检查是否是小端序
        if data[5] != 1 {
            emit(8, 1);
            return None;
        }

        emit(9, 0); // endian OK

        // 检查是否是 ELF64 版本
        if data[6] != 1 {
            emit(10, 1);
            return None;
        }

        emit(11, 0); // version OK

        // 检查系统V ABI
        if data[7] != 0 {
            emit(12, 1);
            return None;
        }

        emit(13, 0); // OK, returning

        // 使用 read_unaligned 避免对齐问题
        Some(ptr::read_unaligned(data.as_ptr() as *const Elf64Ehdr))
    }

    /// 检查 ELF 类型是否可执行
    pub fn is_executable(&self) -> bool {
        self.e_type == ElfType::ET_EXEC as u16
    }

    /// 检查机器类型是否匹配
    pub fn check_machine(&self) -> bool {
        // 检查是否是 AArch64 或 RISC-V
        self.e_machine == ElfMachine::EM_AARCH64 as u16
            || self.e_machine == ElfMachine::EM_RISCV as u16
    }

    /// 获取程序头表
    pub unsafe fn get_program_headers(&self, data: &[u8]) -> Result<usize, ElfError> {
        // 只返回程序头数量，避免堆分配
        if self.e_phoff as usize + self.e_phnum as usize * size_of::<Elf64Phdr>() > data.len() {
            return Err(ElfError::InvalidFormat);
        }
        Ok(self.e_phnum as usize)
    }

    /// 获取单个程序头
    pub unsafe fn get_program_header(&self, data: &[u8], index: usize) -> Option<Elf64Phdr> {
        if index >= self.e_phnum as usize {
            return None;
        }
        let phdr_start = data.as_ptr().add(self.e_phoff as usize) as *const Elf64Phdr;
        Some(ptr::read_unaligned(phdr_start.add(index)))
    }
}

impl Elf64Phdr {
    /// 检查段是否可加载
    pub fn is_load(&self) -> bool {
        self.p_type == ElfPtType::PT_LOAD as u32
    }

    /// 检查段是否可读
    pub fn is_readable(&self) -> bool {
        (self.p_flags & PF_R) != 0
    }

    /// 检查段是否可写
    pub fn is_writable(&self) -> bool {
        (self.p_flags & PF_W) != 0
    }

    /// 检查段是否可执行
    pub fn is_executable(&self) -> bool {
        (self.p_flags & PF_X) != 0
    }
}

/// ELF 加载结果
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ElfLoadInfo {
    /// 入口点地址
    pub entry: u64,
    /// 加载的段数量
    pub load_count: usize,
    /// 最小虚拟地址
    pub min_vaddr: u64,
    /// 最大虚拟地址
    pub max_vaddr: u64,
    /// 解释器路径（如果有 PT_INTERP）
    pub interp_path: Option<&'static [u8]>,
}

/// ELF 加载器
pub struct ElfLoader;

impl ElfLoader {
    /// 检查 ELF 文件是否有效
    pub fn validate(data: &[u8]) -> Result<(), ElfError> {
        use crate::println;

        println!("ElfLoader::validate: data.len()={}", data.len());

        if data.len() < size_of::<Elf64Ehdr>() {
            println!("ElfLoader::validate: data too small");
            return Err(ElfError::InvalidFormat);
        }

        println!("ElfLoader::validate: calling from_bytes...");
        let ehdr = unsafe { Elf64Ehdr::from_bytes(data) }
            .ok_or(ElfError::InvalidHeader)?;

        println!("ElfLoader::validate: checking if executable...");
        if !ehdr.is_executable() {
            println!("ElfLoader::validate: not executable");
            return Err(ElfError::NotExecutable);
        }

        // 检查机器类型（AArch64 或 RISC-V）
        #[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
        {
            println!("ElfLoader::validate: checking machine type...");
            if !ehdr.check_machine() {
                println!("ElfLoader::validate: wrong machine");
                return Err(ElfError::WrongMachine);
            }
        }

        println!("ElfLoader::validate: OK");
        Ok(())
    }

    /// 获取入口点地址
    pub fn get_entry(data: &[u8]) -> Result<u64, ElfError> {
        let ehdr = unsafe { Elf64Ehdr::from_bytes(data) }
            .ok_or(ElfError::InvalidHeader)?;
        Ok(ehdr.e_entry)
    }

    /// 获取程序头表
    pub fn get_program_headers(data: &[u8]) -> Result<usize, ElfError> {
        let ehdr = unsafe { Elf64Ehdr::from_bytes(data) }
            .ok_or(ElfError::InvalidHeader)?;
        unsafe { ehdr.get_program_headers(data) }
    }

    /// 加载 ELF 文件到内存
    ///
    /// 对应 Linux 的 load_elf_binary() (fs/binfmt_elf.c)
    ///
    /// # 参数
    /// - `data`: ELF 文件数据
    /// - `base_addr`: 加载基地址（用户虚拟地址）
    ///
    /// # 返回
    /// 成功返回加载信息，失败返回错误
    pub unsafe fn load(data: &[u8], base_addr: u64) -> Result<ElfLoadInfo, ElfError> {
        // 验证 ELF 文件
        Self::validate(data)?;

        let ehdr = Elf64Ehdr::from_bytes(data)
            .ok_or(ElfError::InvalidHeader)?;

        // 获取程序头数量
        let phdr_count = Self::get_program_headers(data)?;

        let mut load_count = 0;
        let mut min_vaddr = u64::MAX;
        let mut max_vaddr = 0u64;
        let mut interp_path: Option<&'static [u8]> = None;

        // 第一遍扫描：计算地址范围
        for i in 0..phdr_count {
            if let Some(phdr) = ehdr.get_program_header(data, i) {
                if phdr.p_type == ElfPtType::PT_LOAD as u32 {
                    let vaddr = phdr.p_vaddr;
                    let memsz = phdr.p_memsz;

                    if vaddr < min_vaddr {
                        min_vaddr = vaddr;
                    }

                    let end = vaddr + memsz;
                    if end > max_vaddr {
                        max_vaddr = end;
                    }

                    load_count += 1;
                } else if phdr.p_type == ElfPtType::PT_INTERP as u32 {
                    // 提取解释器路径
                    let offset = phdr.p_offset as usize;
                    let size = phdr.p_filesz as usize;

                    if offset + size <= data.len() {
                        // 找到 null 终止符
                        let mut len = 0;
                        for i in 0..size {
                            if data[offset + i] == 0 {
                                len = i;
                                break;
                            }
                        }

                        if len > 0 {
                            interp_path = Some(core::slice::from_raw_parts(
                                data.as_ptr().add(offset),
                                len,
                            ));
                        }
                    }
                }
            }
        }

        if load_count == 0 {
            return Err(ElfError::NoLoadSegments);
        }

        // 第二遍扫描：实际加载段
        for i in 0..phdr_count {
            if let Some(phdr) = ehdr.get_program_header(data, i) {
                if phdr.p_type == ElfPtType::PT_LOAD as u32 {
                    Self::load_segment(data, &phdr, base_addr)?;
                }
            }
        }

        Ok(ElfLoadInfo {
            entry: ehdr.e_entry,
            load_count,
            min_vaddr,
            max_vaddr,
            interp_path,
        })
    }

    /// 加载单个 PT_LOAD 段
    ///
    /// 对应 Linux 的 load_elf_binary() 中的段加载逻辑
    ///
    /// # 参数
    /// - `data`: ELF 文件数据
    /// - `phdr`: 程序头
    /// - `base_addr`: 加载基地址
    unsafe fn load_segment(data: &[u8], phdr: &Elf64Phdr, base_addr: u64) -> Result<(), ElfError> {
        let offset = phdr.p_offset as usize;
        let filesz = phdr.p_filesz as usize;
        let memsz = phdr.p_memsz as usize;
        let vaddr = base_addr + phdr.p_vaddr;

        // 检查边界
        if offset + filesz > data.len() {
            return Err(ElfError::InvalidSegment);
        }

        // 复制段数据到内存
        if filesz > 0 {
            let src = data.as_ptr().add(offset);
            let dst = vaddr as *mut u8;

            // 复制文件中的数据
            core::ptr::copy_nonoverlapping(src, dst, filesz);
        }

        // BSS 段清零（p_memsz > p_filesz 的部分）
        if memsz > filesz {
            let bss_start = vaddr + filesz as u64;
            let bss_size = memsz - filesz;

            // 清零 BSS 段
            core::ptr::write_bytes(bss_start as *mut u8, 0, bss_size);
        }

        Ok(())
    }

    /// 获取解释器路径（如果有）
    pub fn get_interpreter(data: &[u8]) -> Option<&'static [u8]> {
        let ehdr = unsafe { Elf64Ehdr::from_bytes(data) }?;
        let phdr_count = Self::get_program_headers(data).ok()?;

        for i in 0..phdr_count {
            let phdr = unsafe { ehdr.get_program_header(data, i) }?;
            if phdr.p_type == ElfPtType::PT_INTERP as u32 {
                let offset = phdr.p_offset as usize;
                let size = phdr.p_filesz as usize;

                if offset + size <= data.len() {
                    // 找到 null 终止符
                    let mut len = 0;
                    for i in 0..size {
                        if unsafe { *data.as_ptr().add(offset + i) } == 0 {
                            len = i;
                            break;
                        }
                    }

                    if len > 0 {
                        return Some(unsafe { core::slice::from_raw_parts(data.as_ptr().add(offset), len) });
                    }
                }
            }
        }

        None
    }
}

/// ELF 错误类型
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum ElfError {
    /// 无效的 ELF 格式
    InvalidFormat,
    /// 无效的 ELF 头
    InvalidHeader,
    /// 不可执行文件
    NotExecutable,
    /// 机器类型不匹配
    WrongMachine,
    /// 无效的程序头
    InvalidProgramHeaders,
    /// 内存不足
    OutOfMemory,
    /// 无效的段
    InvalidSegment,
    /// 没有 PT_LOAD 段
    NoLoadSegments,
}
