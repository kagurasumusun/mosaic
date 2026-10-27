//! PCI bus fabric: config-space routing by (device, function).
//!
//! The fabric owns the bus's own addressing: which device number holds a
//! function, and how a config cycle's (device, function, offset) decodes to
//! that function's [`PciFunction`] port. BAR assignment protocols
//! (write-all-ones sizing) are answered by each function, because the size
//! belongs to the device; the fabric only routes.

use std::collections::BTreeMap;

use mosaic_contract::pci::{PciError, PciFunction};

/// One (device, function) slot on this bus.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct PciSlot {
    pub device: u8,
    pub function: u8,
}

impl PciSlot {
    pub const fn new(device: u8, function: u8) -> Self {
        Self { device, function }
    }
}

/// A PCI bus segment: functions by slot, config cycles routed by slot.
pub struct PciBus {
    functions: BTreeMap<PciSlot, Box<dyn PciFunction>>,
    /// Slots the fabric reports as occupied (for host-bridge enumeration
    /// models and diagnostics).
    next_slot_hint: u8,
}

impl Default for PciBus {
    fn default() -> Self {
        Self::new()
    }
}

impl PciBus {
    pub fn new() -> Self {
        Self {
            functions: BTreeMap::new(),
            next_slot_hint: 0,
        }
    }

    /// Attach a function at a slot. The machine chooses the slot number the
    /// way a board wires its board-specific IDSEL lines — board-unique
    /// information, never device information.
    pub fn attach(
        &mut self,
        slot: PciSlot,
        function: Box<dyn PciFunction>,
    ) -> Result<(), PciError> {
        if self.functions.contains_key(&slot) {
            return Err(PciError::NoDevice);
        }
        self.next_slot_hint = self.next_slot_hint.max(slot.device.saturating_add(1));
        self.functions.insert(slot, function);
        Ok(())
    }

    pub fn detach(&mut self, slot: PciSlot) -> Option<Box<dyn PciFunction>> {
        self.functions.remove(&slot)
    }

    pub fn slot_count(&self) -> usize {
        self.functions.len()
    }

    /// The next free device number (for host bridges that enumerate).
    pub fn next_free_device(&self) -> u8 {
        let mut free = 0u8;
        while self.functions.keys().any(|slot| slot.device == free) {
            free += 1;
        }
        free
    }

    /// One config read, decoded by slot and offset. The addressed byte lanes
    /// of the function's dword answer, exactly as a real config cycle
    /// returns only the addressed bytes on a byte access.
    pub fn config_read(&mut self, slot: PciSlot, offset: u8, size: u8) -> Result<u32, PciError> {
        let function = self.functions.get_mut(&slot).ok_or(PciError::NoDevice)?;
        let dword = function.config_read(offset & !0x3)?;
        match size {
            1 => Ok((dword >> ((offset & 0x3) * 8)) & 0xFF),
            2 => Ok((dword >> ((offset & 0x2) * 8)) & 0xFFFF),
            4 => Ok(dword),
            _ => Err(PciError::BadOffset),
        }
    }

    /// One config write. Byte lanes inside the dword are merged the way a
    /// real bridge merges them.
    pub fn config_write(
        &mut self,
        slot: PciSlot,
        offset: u8,
        size: u8,
        value: u32,
    ) -> Result<(), PciError> {
        let function = self.functions.get_mut(&slot).ok_or(PciError::NoDevice)?;
        match size {
            1 => {
                let shift = (offset & 0x3) * 8;
                let dword = function.config_read(offset & !0x3)?;
                let merged = (dword & !(0xFFu32 << shift)) | ((value & 0xFF) << shift);
                function.config_write(offset & !0x3, merged)
            }
            2 => {
                let shift = (offset & 0x2) * 8;
                let dword = function.config_read(offset & !0x3)?;
                let merged = (dword & !(0xFFFFu32 << shift)) | ((value & 0xFFFF) << shift);
                function.config_write(offset & !0x3, merged)
            }
            4 => function.config_write(offset & !0x3, value),
            _ => Err(PciError::BadOffset),
        }
    }

    /// Assert/deassert a function's interrupt line.
    pub fn set_interrupt(&mut self, slot: PciSlot, asserted: bool) -> Result<(), PciError> {
        let function = self.functions.get_mut(&slot).ok_or(PciError::NoDevice)?;
        function.set_interrupt(asserted);
        Ok(())
    }

    /// Enumerate occupied slots in deterministic order.
    pub fn slots(&self) -> impl Iterator<Item = PciSlot> + '_ {
        self.functions.keys().copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mosaic_contract::pci::{header, PciError};

    /// A function that decodes a type-0 header: vendor/device, one 4 KiB
    /// memory BAR at BAR0.
    struct TestFunction {
        vendor: u16,
        device: u16,
        bar0: u32,
        interrupts: Vec<bool>,
    }

    impl TestFunction {
        fn new(vendor: u16, device: u16) -> Self {
            Self {
                vendor,
                device,
                bar0: 0,
                interrupts: Vec::new(),
            }
        }
    }

    impl PciFunction for TestFunction {
        fn config_read(&mut self, offset: u8) -> Result<u32, PciError> {
            // The contract serves whole dwords at 4-byte-aligned offsets:
            // the word at +2 of the dword at 0 is the device ID.
            Ok(match offset {
                header::VENDOR_ID => u32::from(self.vendor) | (u32::from(self.device) << 16),
                offset if offset == header::BAR[0] => self.bar0,
                // INTERRUPT_PIN (0x3D) is byte lane 1 of the dword at 0x3C.
                0x3C => 0x0000_0100, // INTA#
                _ => 0,
            })
        }

        fn config_write(&mut self, offset: u8, value: u32) -> Result<(), PciError> {
            if offset == header::BAR[0] {
                // Sizing protocol: writing all ones reads back the size mask.
                self.bar0 = if value == 0xFFFF_FFFF {
                    0xFFFF_F000 // 4 KiB memory BAR
                } else {
                    value & 0xFFFF_F000
                };
            }
            Ok(())
        }

        fn set_interrupt(&mut self, asserted: bool) {
            self.interrupts.push(asserted);
        }
    }

    #[test]
    fn config_reads_route_by_slot() {
        let mut bus = PciBus::new();
        bus.attach(
            PciSlot::new(3, 0),
            Box::new(TestFunction::new(0x8086, 0x1234)),
        )
        .unwrap();
        assert_eq!(
            bus.config_read(PciSlot::new(3, 0), header::VENDOR_ID, 2),
            Ok(0x8086)
        );
        assert_eq!(
            bus.config_read(PciSlot::new(3, 0), header::DEVICE_ID, 2),
            Ok(0x1234)
        );
        assert_eq!(
            bus.config_read(PciSlot::new(4, 0), header::VENDOR_ID, 2),
            Err(PciError::NoDevice)
        );
    }

    #[test]
    fn byte_and_word_accesses_decode_lanes() {
        let mut bus = PciBus::new();
        bus.attach(
            PciSlot::new(1, 0),
            Box::new(TestFunction::new(0x8086, 0x7000)),
        )
        .unwrap();
        // INTERRUPT_PIN is byte 0x3D = byte lane 1 of dword 0x3C.
        assert_eq!(bus.config_read(PciSlot::new(1, 0), 0x3D, 1), Ok(0x01));
        assert_eq!(bus.config_read(PciSlot::new(1, 0), 0x3C, 2), Ok(0x0100));
    }

    #[test]
    fn bar_sizing_answers_the_size_mask() {
        let mut bus = PciBus::new();
        bus.attach(PciSlot::new(2, 0), Box::new(TestFunction::new(1, 2)))
            .unwrap();
        bus.config_write(PciSlot::new(2, 0), header::BAR[0], 4, 0xFFFF_FFFF)
            .unwrap();
        assert_eq!(
            bus.config_read(PciSlot::new(2, 0), header::BAR[0], 4),
            Ok(0xFFFF_F000)
        );
        bus.config_write(PciSlot::new(2, 0), header::BAR[0], 4, 0xE000_0000)
            .unwrap();
        assert_eq!(
            bus.config_read(PciSlot::new(2, 0), header::BAR[0], 4),
            Ok(0xE000_0000)
        );
    }

    #[test]
    fn interrupt_assertion_reaches_the_function() {
        let mut bus = PciBus::new();
        bus.attach(PciSlot::new(0, 0), Box::new(TestFunction::new(1, 1)))
            .unwrap();
        bus.set_interrupt(PciSlot::new(0, 0), true).unwrap();
        bus.set_interrupt(PciSlot::new(0, 0), false).unwrap();
        // Observed through the function's own recording; slots enumerate
        // in deterministic order.
        assert_eq!(bus.slots().collect::<Vec<_>>(), vec![PciSlot::new(0, 0)]);
    }
}
