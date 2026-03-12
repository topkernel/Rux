//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! PCI configuration space access layer
//!
//! Implements PCI configuration space access and device enumeration

/// PCI configuration space register offsets
pub mod offset {
    pub const VENDOR_ID: u8 = 0x00;
    pub const DEVICE_ID: u8 = 0x02;
    pub const COMMAND: u8 = 0x04;
    pub const STATUS: u8 = 0x06;
    pub const REVISION: u8 = 0x08;
    pub const PROG_IF: u8 = 0x09;
    pub const SUBCLASS: u8 = 0x0A;
    pub const CLASS: u8 = 0x0B;
    pub const CACHE_LINE_SIZE: u8 = 0x0C;
    pub const LATENCY_TIMER: u8 = 0x0D;
    pub const HEADER_TYPE: u8 = 0x0E;
    pub const BIST: u8 = 0x0F;
    pub const BAR0: u8 = 0x10;
    pub const BAR1: u8 = 0x14;
    pub const BAR2: u8 = 0x18;
    pub const BAR3: u8 = 0x1C;
    pub const BAR4: u8 = 0x20;
    pub const BAR5: u8 = 0x24;
    pub const CARDBUS_CIS_PTR: u8 = 0x28;
    pub const SUBSYSTEM_VENDOR_ID: u8 = 0x2C;
    pub const SUBSYSTEM_ID: u8 = 0x2E;
    pub const EXP_ROM_BASE_ADDR: u8 = 0x30;
    pub const CAPABILITIES_PTR: u8 = 0x34;
    pub const RESERVED0: u8 = 0x35;
    pub const RESERVED1: u8 = 0x38;
    pub const INT_LINE: u8 = 0x3C;
    pub const INT_PIN: u8 = 0x3D;
    pub const MIN_GNT: u8 = 0x3E;
    pub const MAX_LAT: u8 = 0x3F;
}

/// PCI command register bits
pub mod command {
    pub const IO_SPACE: u16 = 0x0001;
    pub const MEMORY_SPACE: u16 = 0x0002;
    pub const BUS_MASTER: u16 = 0x0004;
    pub const SPECIAL_CYCLES: u16 = 0x0008;
    pub const MEM_WR_INV: u16 = 0x0010;
    pub const VGA_PALETTE: u16 = 0x0020;
    pub const PARITY_ERR_RESP: u16 = 0x0040;
    pub const SERR_ENABLE: u16 = 0x0080;
    pub const FAST_BACK_TO_BACK: u16 = 0x0100;
    pub const INT_DISABLE: u16 = 0x0400;
}

/// PCI status register bits
pub mod status {
    pub const CAPABILITIES_LIST: u16 = 0x0010;
    pub const IRQ_STATUS: u16 = 0x0008;
    pub const CAPABILITIES_LIST_66MHZ: u16 = 0x0200;
    pub const FAST_BACK_TO_BACK: u16 = 0x0080;
    pub const DEVSEL_TIMING: u16 = 0x0600;
}

/// PCI BAR type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BARType {
    MemoryMapped,
    IOMapped,
    None,
}

/// PCI Base Address Register (BAR)
#[derive(Debug, Clone, Copy)]
pub struct PCIBAR {
    pub base_addr: u64,
    pub size: u64,
    pub bar_type: BARType,
    pub prefetchable: bool,
    pub is_64bit: bool,
}

impl PCIBAR {
    pub const fn empty() -> Self {
        Self {
            base_addr: 0,
            size: 0,
            bar_type: BARType::None,
            prefetchable: false,
            is_64bit: false,
        }
    }

    /// Check if BAR is valid (non-empty)
    pub fn is_valid(&self) -> bool {
        self.bar_type != BARType::None && self.size > 0
    }
}

/// PCI configuration space access structure
#[derive(Debug, Clone, Copy)]
pub struct PCIConfig {
    pub base_addr: u64,
}

impl PCIConfig {
    /// Create new PCI configuration space access
    pub const fn new(base_addr: u64) -> Self {
        Self { base_addr }
    }

    /// Read 32-bit configuration space register
    pub fn read_config_dword(&self, offset: u8) -> u32 {
        unsafe {
            let ptr = (self.base_addr + offset as u64) as *const u32;
            core::ptr::read_volatile(ptr)
        }
    }

    /// Write 32-bit configuration space register
    pub fn write_config_dword(&self, offset: u8, value: u32) {
        unsafe {
            let ptr = (self.base_addr + offset as u64) as *mut u32;
            core::ptr::write_volatile(ptr, value);
        }
    }

    /// Read 16-bit configuration space register
    pub fn read_config_word(&self, offset: u8) -> u16 {
        unsafe {
            let ptr = (self.base_addr + offset as u64) as *const u16;
            core::ptr::read_volatile(ptr)
        }
    }

    /// Read 8-bit configuration space register
    pub fn read_config_byte(&self, offset: u8) -> u8 {
        unsafe {
            let ptr = (self.base_addr + offset as u64) as *const u8;
            core::ptr::read_volatile(ptr)
        }
    }

    /// Read BAR
    pub fn read_bar(&self, bar_index: u8) -> PCIBAR {
        if bar_index > 5 {
            return PCIBAR::empty();
        }

        let bar_offset = offset::BAR0 + (bar_index * 4);
        let bar_low = self.read_config_dword(bar_offset);

        // Determine BAR type
        if bar_low & 0x00000001 == 0x00000001 {
            // I/O mapped BAR
            PCIBAR {
                base_addr: (bar_low & 0xFFFFFFFC) as u64,
                size: 0,
                bar_type: BARType::IOMapped,
                prefetchable: false,
                is_64bit: false,
            }
        } else {
            // Memory mapped BAR
            // Check if 64-bit BAR (bit 2-1 = 10)
            let is_64bit = (bar_low & 0x00000006) == 0x00000004;

            let base_addr = if is_64bit {
                // 64-bit BAR: read next BAR register as high 32 bits
                if bar_index + 1 <= 5 {
                    let bar_high = self.read_config_dword(bar_offset + 4);
                    ((bar_high as u64) << 32) | ((bar_low & 0xFFFFFFF0) as u64)
                } else {
                    // Should not happen: 64-bit BAR must have paired high 32-bit register
                    (bar_low & 0xFFFFFFF0) as u64
                }
            } else {
                // 32-bit BAR
                (bar_low & 0xFFFFFFF0) as u64
            };

            PCIBAR {
                base_addr,
                size: 0,
                bar_type: BARType::MemoryMapped,
                prefetchable: (bar_low & 0x00000008) != 0,
                is_64bit,
            }
        }
    }

    /// Probe BAR size
    ///
    /// Determine BAR size by writing all 1s then reading back
    pub fn probe_bar_size(&self, bar_index: u8) -> u64 {
        if bar_index > 5 {
            return 0;
        }

        let bar_offset = offset::BAR0 + (bar_index * 4);
        let original_low = self.read_config_dword(bar_offset);

        // Determine BAR type
        let is_io = (original_low & 0x01) != 0;
        let is_64bit = !is_io && ((original_low & 0x06) == 0x04);

        // Save original value
        let original_high = if is_64bit && bar_index + 1 <= 5 {
            self.read_config_dword(bar_offset + 4)
        } else {
            0
        };

        // Write all 1s to probe size
        self.write_config_dword(bar_offset, 0xFFFFFFFF);

        let size_low = self.read_config_dword(bar_offset);

        // Restore original value
        self.write_config_dword(bar_offset, original_low);

        let size = if is_io {
            // I/O BAR: size is inverted & ~0x03
            ((!(size_low & 0xFFFFFFFC)) + 1) as u64
        } else if is_64bit {
            // 64-bit Memory BAR
            // Write all 1s to high bits
            if bar_index + 1 <= 5 {
                self.write_config_dword(bar_offset + 4, 0xFFFFFFFF);
                let size_high = self.read_config_dword(bar_offset + 4);
                self.write_config_dword(bar_offset + 4, original_high);

                let low = (!(size_low & 0xFFFFFFF0)) as u64;
                let high = (!size_high) as u64;
                ((high << 32) | low) + 1
            } else {
                0
            }
        } else {
            // 32-bit Memory BAR
            ((!(size_low & 0xFFFFFFF0)) + 1) as u64
        };

        // If size is 0 or result overflowed, return 0
        if size == 0 || size > 0x8000000000 {
            0
        } else {
            size
        }
    }

    /// Allocate and set BAR address
    ///
    /// # Parameters
    /// - `bar_index`: BAR index (0-5)
    /// - `base_addr`: Base address to set
    ///
    /// # Returns
    /// Successfully set BAR info on success, error on failure
    pub fn assign_bar(&self, bar_index: u8, base_addr: u64) -> Result<PCIBAR, &'static str> {
        if bar_index > 5 {
            return Err("Invalid BAR index");
        }

        let bar_offset = offset::BAR0 + (bar_index * 4);

        // Read original value to determine BAR type
        let original_low = self.read_config_dword(bar_offset);
        let is_io = (original_low & 0x01) != 0;
        let is_64bit = !is_io && ((original_low & 0x06) == 0x04);

        // Alignment check: address must satisfy BAR alignment requirement
        let size = self.probe_bar_size(bar_index);
        if size == 0 {
            return Err("Invalid BAR size");
        }

        if base_addr % size != 0 {
            return Err("BAR address not properly aligned");
        }

        if is_64bit {
            // 64-bit BAR: write low 32 bits and high 32 bits
            let low = (base_addr & 0xFFFFFFFF) as u32;
            let high = (base_addr >> 32) as u32;

            // Preserve type bits in low bits
            let low_with_flags = low | (original_low & 0x0F);

            self.write_config_dword(bar_offset, low_with_flags);
            if bar_index + 1 <= 5 {
                self.write_config_dword(bar_offset + 4, high);
            }

            Ok(PCIBAR {
                base_addr,
                size,
                bar_type: BARType::MemoryMapped,
                prefetchable: (original_low & 0x08) != 0,
                is_64bit: true,
            })
        } else if is_io {
            // I/O BAR
            let addr = (base_addr & 0xFFFFFFFC) as u32;
            self.write_config_dword(bar_offset, addr | 0x01);

            Ok(PCIBAR {
                base_addr: addr as u64,
                size,
                bar_type: BARType::IOMapped,
                prefetchable: false,
                is_64bit: false,
            })
        } else {
            // 32-bit Memory BAR
            let addr = (base_addr & 0xFFFFFFF0) as u32;
            let prefetchable = (original_low & 0x08) != 0;
            self.write_config_dword(bar_offset, addr | (if prefetchable { 0x08 } else { 0x00 }) | 0x00);

            Ok(PCIBAR {
                base_addr: addr as u64,
                size,
                bar_type: BARType::MemoryMapped,
                prefetchable,
                is_64bit: false,
            })
        }
    }

    /// Get vendor ID
    pub fn vendor_id(&self) -> u16 {
        self.read_config_word(offset::VENDOR_ID)
    }

    /// Get device ID
    pub fn device_id(&self) -> u16 {
        self.read_config_word(offset::DEVICE_ID)
    }

    /// Get class code
    pub fn class_code(&self) -> u8 {
        self.read_config_byte(offset::CLASS)
    }

    /// Get subclass code
    pub fn subclass(&self) -> u8 {
        self.read_config_byte(offset::SUBCLASS)
    }

    /// Get programming interface
    pub fn prog_if(&self) -> u8 {
        self.read_config_byte(offset::PROG_IF)
    }

    /// Get revision ID
    pub fn revision_id(&self) -> u8 {
        self.read_config_byte(offset::REVISION)
    }

    /// Get interrupt pin
    pub fn interrupt_pin(&self) -> u8 {
        self.read_config_byte(offset::INT_PIN)
    }

    /// Get interrupt line
    pub fn interrupt_line(&self) -> u8 {
        self.read_config_byte(offset::INT_LINE)
    }

    /// Set command register
    pub fn set_command(&self, cmd: u16) {
        self.write_config_dword(offset::COMMAND, cmd as u32);
    }

    /// Get command register
    pub fn command(&self) -> u16 {
        self.read_config_word(offset::COMMAND)
    }

    /// Enable bus mastering
    pub fn enable_bus_master(&self) {
        let current = self.command();
        self.set_command(current | command::BUS_MASTER | command::MEMORY_SPACE);
    }
}

/// Known vendor IDs
pub mod vendor {
    pub const RED_HAT: u16 = 0x1AF4;
}

/// VirtIO device IDs (PCI)
pub mod virtio_device {
    pub const VIRTIO_NET: u16 = 0x1000;
    pub const VIRTIO_BLK: u16 = 0x1001;
    pub const VIRTIO_BLK_MODERN: u16 = 0x1042;
    pub const VIRTIO_GPU: u16 = 0x1050;
}

/// RISC-V PCIe ECAM base address
#[cfg(feature = "riscv64")]
pub const RISCV_PCIE_ECAM_BASE: u64 = 0x30000000;

/// PCIe ECAM configuration space size
pub const PCIE_ECAM_SIZE: u64 = 0x1000;

/// Enumerate VirtIO devices on PCI bus
///
/// # Returns
/// Number of VirtIO devices found
pub fn enumerate_virtio_devices() -> usize {
    #[cfg(feature = "riscv64")]
    {
        let mut device_count = 0;

        // RISC-V: Scan PCIe ECAM space
        const MAX_DEVICES: u8 = 32;

        for device in 0..MAX_DEVICES {
            let ecam_addr = RISCV_PCIE_ECAM_BASE + (device as u64 * PCIE_ECAM_SIZE);
            let config = PCIConfig::new(ecam_addr);

            let vendor_id = config.vendor_id();
            let device_id = config.device_id();

            // Check if device exists
            if vendor_id == 0xFFFF {
                continue;
            }

            // Check if VirtIO device (Red Hat)
            if vendor_id == vendor::RED_HAT {
                // Identify VirtIO device type
                match device_id {
                    virtio_device::VIRTIO_BLK | virtio_device::VIRTIO_NET => {
                        device_count += 1;
                    }
                    _ => {}
                }
            }
        }

        device_count
    }

    #[cfg(not(feature = "riscv64"))]
    {
        0
    }
}
