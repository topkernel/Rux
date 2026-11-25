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

// The type definitions this file declared live in fs/types.rs; every impl
// and function that uses them stays in this file. Re-exported so the
// existing `fs::elf::…` paths are unchanged.
pub use super::types::{
    Elf64Ehdr,
    ElfType,
    ElfMachine,
    Elf64Phdr,
    ElfPtType,
    ElfLoadInfo,
    ElfLoader,
    ElfError,
};

pub const PF_X: u32 = 0x1;  // Executable
pub const PF_W: u32 = 0x2;  // Writable
pub const PF_R: u32 = 0x4;  // Readable

/// PT_GNU_STACK (UAPI): stack permission request. PF_X in p_flags asks
/// for an executable stack (EXSTACK_ENABLE_X); any other value keeps the
/// secure NX default. A MISSING header also means NX on this port —
/// neither arch maps onto the reference's READ_IMPLIES_EXEC fallback
/// (that path exists for 32-bit x86 binaries only).
pub const PT_GNU_STACK: u32 = 0x6474_e551;

impl Elf64Ehdr {
    /// Parse ELF header from byte buffer
    pub fn from_bytes(data: &[u8]) -> Option<Elf64Ehdr> {
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

        // Program header entry size must be exactly sizeof(Elf64Phdr).
        // Every phdr copy path multiplies e_phoff arithmetic by this value;
        // a forged larger value would read past the file end into kernel
        // memory (review 2R.12).
        if u16::from_le_bytes([data[54], data[55]]) != size_of::<Elf64Phdr>() as u16 {
            return None;
        }

        // Check ABI (accept System V and GNU ABI)
        // data[7] = EI_OSABI:
        //   0 = ELFOSABI_NONE/ELFOSABI_SYSV
        //   3 = ELFOSABI_GNU
        if data[7] != 0 && data[7] != 3 {
            return None;
        }

        // The unaligned header read rides CopyBytes (INV-CBYTES-2).
        crate::safety::bytes::from_bytes::<Elf64Ehdr>(data)
    }

    /// Check if ELF type is executable (ET_EXEC or ET_DYN for shared objects/PIE)
    pub fn is_executable(&self) -> bool {
        self.e_type == ElfType::ET_EXEC as u16
            || self.e_type == ElfType::ET_DYN as u16
    }

    /// Check if ELF type is dynamic (shared object)
    pub fn is_dynamic(&self) -> bool {
        self.e_type == ElfType::ET_DYN as u16
    }

    /// Check if machine type matches the running architecture
    pub fn check_machine(&self) -> bool {
        #[cfg(feature = "riscv64")]
        {
            self.e_machine == ElfMachine::EM_RISCV as u16
        }
        #[cfg(feature = "x86_64")]
        {
            self.e_machine == ElfMachine::EM_X86_64 as u16
        }
        #[cfg(feature = "aarch64")]
        {
            self.e_machine == ElfMachine::EM_AARCH64 as u16
        }
    }

    /// Get program headers
    pub fn get_program_headers(&self, data: &[u8]) -> Result<usize, ElfError> {
        // Only return program header count, avoid heap allocation
        let phdr_end = (self.e_phoff as usize)
            .checked_add((self.e_phnum as usize).wrapping_mul(size_of::<Elf64Phdr>()))
            .filter(|&end| end <= data.len())
            .ok_or(ElfError::InvalidFormat)?;
        let _ = phdr_end; // used only for bounds validation
        Ok(self.e_phnum as usize)
    }

    /// Get single program header
    pub fn get_program_header(&self, data: &[u8], index: usize) -> Option<Elf64Phdr> {
        if index >= self.e_phnum as usize {
            return None;
        }
        // Bounds-check the entry against the file image: e_phoff comes from
        // the (possibly attacker-controlled) ELF header, so it must never be
        // trusted as a raw pointer offset.
        let phdr_off = (self.e_phoff as usize)
            .checked_add(index.checked_mul(size_of::<Elf64Phdr>())?)?;
        let phdr_end = phdr_off.checked_add(size_of::<Elf64Phdr>())?;
        if phdr_end > data.len() {
            return None;
        }
        // The unaligned entry read rides CopyBytes (INV-CBYTES-2).
        crate::safety::bytes::read_at::<Elf64Phdr>(data, phdr_off)
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

impl ElfLoader {
    /// Check if ELF file is valid
    pub fn validate(data: &[u8]) -> Result<(), ElfError> {
        if data.len() < size_of::<Elf64Ehdr>() {
            return Err(ElfError::InvalidFormat);
        }

        let ehdr = Elf64Ehdr::from_bytes(data)
            .ok_or(ElfError::InvalidHeader)?;

        if !ehdr.is_executable() {
            return Err(ElfError::NotExecutable);
        }

        // Machine check on every platform: a foreign-arch image must
        // fail here with ENOEXEC, never load-and-jump into garbage.
        // This guard was once cfg-limited to aarch64/riscv64 (written
        // before the x86_64 port), which let an x86_64 kernel exec a
        // riscv64 ELF straight through (review CR-2).
        if !ehdr.check_machine() {
            return Err(ElfError::WrongMachine);
        }

        Ok(())
    }

    /// Get entry point address
    pub fn get_entry(data: &[u8]) -> Result<u64, ElfError> {
        let ehdr = Elf64Ehdr::from_bytes(data)
            .ok_or(ElfError::InvalidHeader)?;
        Ok(ehdr.e_entry)
    }

    /// Get program headers
    pub fn get_program_headers(data: &[u8]) -> Result<usize, ElfError> {
        let ehdr = Elf64Ehdr::from_bytes(data)
            .ok_or(ElfError::InvalidHeader)?;
        ehdr.get_program_headers(data)
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
    pub fn load(data: &[u8], base_addr: u64) -> Result<ElfLoadInfo<'_>, ElfError> {
        // Validate ELF file
        Self::validate(data)?;

        let ehdr = Elf64Ehdr::from_bytes(data)
            .ok_or(ElfError::InvalidHeader)?;

        // Get program header count
        let phdr_count = Self::get_program_headers(data)?;

        let mut load_count = 0;
        let mut min_vaddr = u64::MAX;
        let mut max_vaddr = 0u64;
        let mut interp_path: Option<&[u8]> = None;

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
                            interp_path = Some(&data[offset..offset + len]);
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
    fn load_segment(data: &[u8], phdr: &Elf64Phdr, base_addr: u64) -> Result<(), ElfError> {
        let offset = phdr.p_offset as usize;
        let filesz = phdr.p_filesz as usize;
        let memsz = phdr.p_memsz as usize;
        let vaddr = base_addr.checked_add(phdr.p_vaddr).ok_or(ElfError::InvalidSegment)?;

        // Check boundaries
        offset.checked_add(filesz).filter(|&end| end <= data.len())
            .ok_or(ElfError::InvalidSegment)?;

        // SAFETY: INV-ELF-SEG — vaddr..vaddr+memsz is the destination span
        // the caller's address-space setup mapped for this PT_LOAD (the
        // exec path maps before loading); the file span was bounds-checked
        // above.
        unsafe {
            // Copy segment data to memory
            if filesz > 0 {
                let src = data.as_ptr().add(offset);
                let dst = vaddr as *mut u8;
                core::ptr::copy_nonoverlapping(src, dst, filesz);
            }

            // Zero BSS segment (p_memsz > p_filesz portion)
            if memsz > filesz {
                let bss_start = vaddr.checked_add(filesz as u64).ok_or(ElfError::InvalidSegment)?;
                let bss_size = memsz - filesz;
                core::ptr::write_bytes(bss_start as *mut u8, 0, bss_size);
            }
        }

        Ok(())
    }

    /// Get interpreter path (if exists)
    pub fn get_interpreter(data: &[u8]) -> Option<&[u8]> {
        let ehdr = Elf64Ehdr::from_bytes(data)?;
        let phdr_count = Self::get_program_headers(data).ok()?;

        for i in 0..phdr_count {
            let phdr = ehdr.get_program_header(data, i)?;
            if phdr.p_type == ElfPtType::PT_INTERP as u32 {
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
                        return Some(&data[offset..offset + len]);
                    }
                }
            }
        }

        None
    }
}
