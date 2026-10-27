//! PCI vocabulary: config-space ports and the standard header layout.
//!
//! A PCI function model implements [`PciFunction`]; the PCI fabric
//! (`mosaic-bus::pci`) routes (bus, device, function, offset) to it. The
//! layout constants below are the offsets in the *type 0* header every
//! function decodes — they live in the contract so a model and its fabric
//! can never disagree about where `VENDOR_ID` sits.

/// Config-space offsets every type-0 header decodes (PCI 3.0 §6.2).
pub mod header {
    /// 0x00: vendor ID (0xFFFF reads as "no device").
    pub const VENDOR_ID: u8 = 0x00;
    /// 0x02: device ID.
    pub const DEVICE_ID: u8 = 0x02;
    /// 0x04: command register.
    pub const COMMAND: u8 = 0x04;
    /// 0x06: status register.
    pub const STATUS: u8 = 0x06;
    /// 0x08: revision ID.
    pub const REVISION_ID: u8 = 0x08;
    /// 0x09: class code, high byte.
    pub const CLASS_CODE_HIGH: u8 = 0x09;
    /// 0x0B: class code, base class byte.
    pub const CLASS_CODE_BASE: u8 = 0x0B;
    /// 0x0C: cache line size.
    pub const CACHE_LINE_SIZE: u8 = 0x0C;
    /// 0x0D: latency timer.
    pub const LATENCY_TIMER: u8 = 0x0D;
    /// 0x0E: header type (bit 7 = multi-function).
    pub const HEADER_TYPE: u8 = 0x0E;
    /// 0x0F: built-in self test.
    pub const BIST: u8 = 0x0F;
    /// 0x10..0x28: base address registers 0..5 (BAR 1, 3, 5 may be 64-bit).
    pub const BAR: [u8; 6] = [0x10, 0x14, 0x18, 0x1C, 0x20, 0x24];
    /// 0x2C: subsystem vendor ID.
    pub const SUBSYSTEM_VENDOR_ID: u8 = 0x2C;
    /// 0x2E: subsystem ID.
    pub const SUBSYSTEM_ID: u8 = 0x2E;
    /// 0x30: expansion ROM base address.
    pub const EXPANSION_ROM: u8 = 0x30;
    /// 0x3C: interrupt line.
    pub const INTERRUPT_LINE: u8 = 0x3C;
    /// 0x3D: interrupt pin (0 = none, 1 = INTA#).
    pub const INTERRUPT_PIN: u8 = 0x3D;
    /// 0x3E: min grant.
    pub const MIN_GNT: u8 = 0x3E;
    /// 0x3F: max latency.
    pub const MAX_LAT: u8 = 0x3F;
}

/// `command` register bits a function must honour.
pub mod command {
    pub const IO_SPACE: u16 = 1 << 0;
    pub const MEMORY_SPACE: u16 = 1 << 1;
    pub const BUS_MASTER: u16 = 1 << 2;
    pub const INTERRUPT_DISABLE: u16 = 1 << 10;
}

/// Base class codes (PCI 3.0 Appendix D, the common ones).
pub mod class_code {
    pub const MASS_STORAGE_IDE: u8 = 0x01;
    pub const NETWORK_ETHERNET: u8 = 0x02;
    pub const DISPLAY_VGA: u8 = 0x03;
    pub const MULTIMEDIA: u8 = 0x04;
    pub const MEMORY: u8 = 0x05;
    pub const BRIDGE_HOST: u8 = 0x06;
    pub const SERIAL_USB: u8 = 0x0C;
    pub const WIRELESS: u8 = 0x0D;
}

/// Why a config access failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum PciError {
    /// No function at this (bus, device, function).
    NoDevice,
    /// The offset is outside the 256-byte config space.
    BadOffset,
}

/// One PCI function on a bus: its config space, decoded by the function
/// itself. Host bridge models route config cycles here through the fabric.
pub trait PciFunction: Send {
    /// A config-space read. The function returns the whole dword; the
    /// fabric extracts the addressed bytes.
    fn config_read(&mut self, offset: u8) -> Result<u32, PciError>;

    /// A config-space write (dword; unaddressed bytes within the dword are
    /// byte lanes). BAR writes during sizing (write-all-ones) are answered
    /// by the function with the BAR's size mask.
    fn config_write(&mut self, offset: u8, value: u32) -> Result<(), PciError>;

    /// Assert/deassert the function's interrupt line (INTA#..). The fabric
    /// routes this to the interrupt controller input the machine declared.
    fn set_interrupt(&mut self, asserted: bool);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bar_layout_is_the_type0_layout() {
        assert_eq!(header::BAR[0], 0x10);
        assert_eq!(header::BAR[5], 0x24);
        assert_eq!(header::INTERRUPT_PIN, 0x3D);
    }

    #[test]
    fn usb_is_class_0c() {
        assert_eq!(class_code::SERIAL_USB, 0x0C);
        assert_eq!(class_code::NETWORK_ETHERNET, 0x02);
    }
}
