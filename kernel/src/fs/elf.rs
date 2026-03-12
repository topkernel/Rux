//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! ELF File Format Parsing and Loading
//!
//!
//! Supported ELF formats:
//! - 64-bit ELF (ELF64)
//! - Little Endian
//! - Executable files (ET_EXEC)
//! - Dynamic linker support (future)

use core::mem::size_of;
use core::ptr;
extern crate alloc;

pub const ELF_MAGIC: [u8; 4] = [0x7f, b'E', b'L', b'F'];

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Elf64Ehdr {
    /// Magic number and other info
    pub e_ident: [u8; 16],
    /// File type
    pub e_type: u16,
    /// Machine type
    pub e_machine: u16,
    /// Version
    pub e_version: u32,
    /// Entry point address
    pub e_entry: u64,
    /// Program header table offset
    pub e_phoff: u64,
    /// Section header table offset
    pub e_shoff: u64,
    /// Processor-specific flags
    pub e_flags: u32,
    /// ELF header size
    pub e_ehsize: u16,
    /// Program header table entry size
    pub e_phentsize: u16,
    /// Program header table entry count
    pub e_phnum: u16,
    /// Section header table entry size
    pub e_shentsize: u16,
    /// Section header table entry count
    pub e_shnum: u16,
    /// Section header string table index
    pub e_shstrndx: u16,
}

#[repr(u16)]
#[derive(Debug, Copy, Clone, PartialEq)]
#[allow(non_camel_case_types)]
pub enum ElfType {
    /// Unknown type
    ET_NONE = 0,
    /// Relocatable file
    ET_REL = 1,
    /// Executable file
    ET_EXEC = 2,
    /// Shared object file
    ET_DYN = 3,
    /// Core file
    ET_CORE = 4,
}

#[repr(u16)]
#[derive(Debug, Copy, Clone, PartialEq)]
#[allow(non_camel_case_types)]
pub enum ElfMachine {
    /// No machine
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
    /// Hemicyle
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
    /// STMicroelectronics ST7 8 bit mc
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

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Elf64Phdr {
    /// Segment type
    pub p_type: u32,
    /// Segment flags
    pub p_flags: u32,
    /// Segment file offset
    pub p_offset: u64,
    /// Segment virtual address
    pub p_vaddr: u64,
    /// Segment physical address
    pub p_paddr: u64,
    /// Segment file size
    pub p_filesz: u64,
    /// Segment memory size
    pub p_memsz: u64,
    /// Segment alignment
    pub p_align: u64,
}

#[repr(u32)]
#[derive(Debug, Copy, Clone, PartialEq)]
#[allow(non_camel_case_types)]
pub enum ElfPtType {
    /// Unused segment
    PT_NULL = 0,
    /// Loadable segment
    PT_LOAD = 1,
    /// Dynamic linking information
    PT_DYNAMIC = 2,
    /// Interpreter path
    PT_INTERP = 3,
    /// Auxiliary information
    PT_NOTE = 4,
    /// Unused
    PT_SHLIB = 5,
    /// Program header table itself
    PT_PHDR = 6,
    /// Thread-local storage
    PT_TLS = 7,
}

pub const PF_X: u32 = 0x1;  // Executable
pub const PF_W: u32 = 0x2;  // Writable
pub const PF_R: u32 = 0x4;  // Readable

impl Elf64Ehdr {
    /// Parse ELF header from byte buffer
    pub unsafe fn from_bytes(data: &[u8]) -> Option<Elf64Ehdr> {
        // Check minimum length
        if data.len() < size_of::<Elf64Ehdr>() {
            return None;
        }

        // Check magic number
        if &data[0..4] != ELF_MAGIC {
            return None;
        }

        // Check if 64-bit ELF
        if data[4] != 2 {
            return None;
        }

        // Check if little endian
        if data[5] != 1 {
            return None;
        }

        // Check if ELF64 version
        if data[6] != 1 {
            return None;
        }

        // Check ABI (accept System V and GNU ABI)
        // data[7] = EI_OSABI:
        //   0 = ELFOSABI_NONE/ELFOSABI_SYSV
        //   3 = ELFOSABI_GNU (Linux)
        if data[7] != 0 && data[7] != 3 {
            return None;
        }

        // Use read_unaligned to avoid alignment issues
        Some(ptr::read_unaligned(data.as_ptr() as *const Elf64Ehdr))
    }

    /// Check if ELF type is executable
    pub fn is_executable(&self) -> bool {
        self.e_type == ElfType::ET_EXEC as u16
    }

    /// Check if machine type matches
    pub fn check_machine(&self) -> bool {
        // Check if AArch64 or RISC-V
        self.e_machine == ElfMachine::EM_AARCH64 as u16
            || self.e_machine == ElfMachine::EM_RISCV as u16
    }

    /// Get program headers
    pub unsafe fn get_program_headers(&self, data: &[u8]) -> Result<usize, ElfError> {
        // Only return program header count, avoid heap allocation
        if self.e_phoff as usize + self.e_phnum as usize * size_of::<Elf64Phdr>() > data.len() {
            return Err(ElfError::InvalidFormat);
        }
        Ok(self.e_phnum as usize)
    }

    /// Get single program header
    pub unsafe fn get_program_header(&self, data: &[u8], index: usize) -> Option<Elf64Phdr> {
        if index >= self.e_phnum as usize {
            return None;
        }
        let phdr_start = data.as_ptr().add(self.e_phoff as usize) as *const Elf64Phdr;
        Some(ptr::read_unaligned(phdr_start.add(index)))
    }
}

impl Elf64Phdr {
    /// Check if segment is loadable
    pub fn is_load(&self) -> bool {
        self.p_type == ElfPtType::PT_LOAD as u32
    }

    /// Check if segment is readable
    pub fn is_readable(&self) -> bool {
        (self.p_flags & PF_R) != 0
    }

    /// Check if segment is writable
    pub fn is_writable(&self) -> bool {
        (self.p_flags & PF_W) != 0
    }

    /// Check if segment is executable
    pub fn is_executable(&self) -> bool {
        (self.p_flags & PF_X) != 0
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ElfLoadInfo {
    /// Entry point address
    pub entry: u64,
    /// Number of loaded segments
    pub load_count: usize,
    /// Minimum virtual address
    pub min_vaddr: u64,
    /// Maximum virtual address
    pub max_vaddr: u64,
    /// Interpreter path (if PT_INTERP exists)
    pub interp_path: Option<&'static [u8]>,
}

pub struct ElfLoader;

impl ElfLoader {
    /// Check if ELF file is valid
    pub fn validate(data: &[u8]) -> Result<(), ElfError> {
        if data.len() < size_of::<Elf64Ehdr>() {
            return Err(ElfError::InvalidFormat);
        }

        let ehdr = unsafe { Elf64Ehdr::from_bytes(data) }
            .ok_or(ElfError::InvalidHeader)?;

        if !ehdr.is_executable() {
            return Err(ElfError::NotExecutable);
        }

        // Check machine type (AArch64 or RISC-V)
        #[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
        {
            if !ehdr.check_machine() {
                return Err(ElfError::WrongMachine);
            }
        }

        Ok(())
    }

    /// Get entry point address
    pub fn get_entry(data: &[u8]) -> Result<u64, ElfError> {
        let ehdr = unsafe { Elf64Ehdr::from_bytes(data) }
            .ok_or(ElfError::InvalidHeader)?;
        Ok(ehdr.e_entry)
    }

    /// Get program headers
    pub fn get_program_headers(data: &[u8]) -> Result<usize, ElfError> {
        let ehdr = unsafe { Elf64Ehdr::from_bytes(data) }
            .ok_or(ElfError::InvalidHeader)?;
        unsafe { ehdr.get_program_headers(data) }
    }

    /// Load ELF file into memory
    ///
    ///
    /// # Arguments
    /// - `data`: ELF file data
    /// - `base_addr`: Load base address (user virtual address)
    ///
    /// # Returns
    /// Load info on success, error on failure
    pub unsafe fn load(data: &[u8], base_addr: u64) -> Result<ElfLoadInfo, ElfError> {
        // Validate ELF file
        Self::validate(data)?;

        let ehdr = Elf64Ehdr::from_bytes(data)
            .ok_or(ElfError::InvalidHeader)?;

        // Get program header count
        let phdr_count = Self::get_program_headers(data)?;

        let mut load_count = 0;
        let mut min_vaddr = u64::MAX;
        let mut max_vaddr = 0u64;
        let mut interp_path: Option<&'static [u8]> = None;

        // First pass: calculate address range
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
                    // Extract interpreter path
                    let offset = phdr.p_offset as usize;
                    let size = phdr.p_filesz as usize;

                    if offset + size <= data.len() {
                        // Find null terminator
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

        // Second pass: actually load segments
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

    /// Load single PT_LOAD segment
    ///
    /// ...
    ///
    /// # Arguments
    /// - `data`: ELF file data
    /// - `phdr`: Program header
    /// - `base_addr`: Load base address
    unsafe fn load_segment(data: &[u8], phdr: &Elf64Phdr, base_addr: u64) -> Result<(), ElfError> {
        let offset = phdr.p_offset as usize;
        let filesz = phdr.p_filesz as usize;
        let memsz = phdr.p_memsz as usize;
        let vaddr = base_addr + phdr.p_vaddr;

        // Check boundaries
        if offset + filesz > data.len() {
            return Err(ElfError::InvalidSegment);
        }

        // Copy segment data to memory
        if filesz > 0 {
            let src = data.as_ptr().add(offset);
            let dst = vaddr as *mut u8;

            // Copy data from file
            core::ptr::copy_nonoverlapping(src, dst, filesz);
        }

        // Zero BSS segment (p_memsz > p_filesz portion)
        if memsz > filesz {
            let bss_start = vaddr + filesz as u64;
            let bss_size = memsz - filesz;

            // Zero BSS segment
            core::ptr::write_bytes(bss_start as *mut u8, 0, bss_size);
        }

        Ok(())
    }

    /// Get interpreter path (if exists)
    pub fn get_interpreter(data: &[u8]) -> Option<&'static [u8]> {
        let ehdr = unsafe { Elf64Ehdr::from_bytes(data) }?;
        let phdr_count = Self::get_program_headers(data).ok()?;

        for i in 0..phdr_count {
            let phdr = unsafe { ehdr.get_program_header(data, i) }?;
            if phdr.p_type == ElfPtType::PT_INTERP as u32 {
                let offset = phdr.p_offset as usize;
                let size = phdr.p_filesz as usize;

                if offset + size <= data.len() {
                    // Find null terminator
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

#[derive(Debug, Copy, Clone, PartialEq)]
pub enum ElfError {
    /// Invalid ELF format
    InvalidFormat,
    /// Invalid ELF header
    InvalidHeader,
    /// Not executable file
    NotExecutable,
    /// Machine type mismatch
    WrongMachine,
    /// Invalid program headers
    InvalidProgramHeaders,
    /// Out of memory
    OutOfMemory,
    /// Invalid segment
    InvalidSegment,
    /// No PT_LOAD segments
    NoLoadSegments,
}
