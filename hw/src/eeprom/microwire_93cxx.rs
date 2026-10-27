//! 93Cxx Microwire EEPROM functional core (93C46..93C86, AT93C/M93C/CAT93C).
//!
//! The fabric decodes the start bit + opcode into [`MicrowireOp`] and the
//! address field into `addr`; this core keeps the array. The 8-bit
//! organization is fixed (the manifests declare 8-bit address widths).
//! Write-class cycles can only clear bits (NOR-style AND semantics),
//! require the write-enable latch (`EWEN`) and drop it when the cycle
//! completes, exactly like the datasheet's self-timed burn. The erased
//! state is `0xFF`.

use mosaic_contract::microwire::{MicrowireError, MicrowireOp, MicrowireTarget};
use mosaic_contract::model::Ctx;

/// One 93Cxx-family Microwire EEPROM (8-bit organization).
pub struct MicrowireEeprom {
    size: usize,
    addr_bits: u8,
    wren: bool,
    data: Vec<u8>,
}

impl MicrowireEeprom {
    /// Capacity in bytes and the address-field width in bits.
    pub fn new(size: usize, addr_bits: u8) -> Self {
        Self {
            size: size.max(1),
            addr_bits,
            wren: false,
            data: vec![0xFF; size.max(1)],
        }
    }

    /// Load an image (a programmed EEPROM contents blob).
    pub fn load(&mut self, image: &[u8]) {
        let n = image.len().min(self.size);
        self.data[..n].copy_from_slice(&image[..n]);
    }

    /// Raw contents (save path for boards and tests).
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// The write-enable latch state.
    pub fn write_enabled(&self) -> bool {
        self.wren
    }
}

impl MicrowireTarget for MicrowireEeprom {
    fn instruction_bits(&self) -> u8 {
        // Start bit + opcode, per the 93Cxx command table.
        3
    }

    fn address_bits(&self) -> u8 {
        self.addr_bits
    }

    fn execute(
        &mut self,
        _ctx: &mut dyn Ctx,
        op: MicrowireOp,
        addr: u16,
        data: &[u8],
    ) -> Result<Vec<u8>, MicrowireError> {
        let mask = if self.addr_bits >= 16 {
            u16::MAX
        } else {
            (1u16 << self.addr_bits) - 1
        };
        let cell = (addr & mask) as usize % self.size;
        let mut out = Vec::new();
        match op {
            MicrowireOp::Ewen => self.wren = true,
            MicrowireOp::Ewds => self.wren = false,
            MicrowireOp::Read => {
                // Serial stream from the addressed cell to the array end.
                for i in cell..self.size {
                    out.push(self.data[i]);
                }
            }
            MicrowireOp::Erase => {
                if self.wren {
                    self.data[cell] = 0xFF;
                    self.wren = false;
                }
            }
            MicrowireOp::Write => {
                if self.wren {
                    // NOR semantics: a write can only clear bits.
                    if let Some(&value) = data.first() {
                        self.data[cell] &= value;
                    }
                    self.wren = false;
                }
            }
            MicrowireOp::Eral => {
                if self.wren {
                    self.data.fill(0xFF);
                    self.wren = false;
                }
            }
            MicrowireOp::Wral => {
                if self.wren {
                    if let Some(&value) = data.first() {
                        for cell in self.data.iter_mut() {
                            *cell &= value;
                        }
                    }
                    self.wren = false;
                }
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TestCtx;

    #[test]
    fn writes_need_ewen_and_only_clear_bits() {
        let mut part = MicrowireEeprom::new(128, 7);
        let mut ctx = TestCtx::default();
        // Without EWEN the write is ignored.
        part.execute(&mut ctx, MicrowireOp::Write, 0, &[0x00])
            .unwrap();
        assert_eq!(part.data()[0], 0xFF);
        // With EWEN the AND semantics apply.
        part.execute(&mut ctx, MicrowireOp::Ewen, 0, &[]).unwrap();
        part.execute(&mut ctx, MicrowireOp::Write, 0, &[0xF0])
            .unwrap();
        assert_eq!(part.data()[0], 0xF0);
        // The latch dropped with the completed cycle.
        part.execute(&mut ctx, MicrowireOp::Write, 1, &[0x00])
            .unwrap();
        assert_eq!(part.data()[1], 0xFF);
    }

    #[test]
    fn read_streams_to_the_end_of_the_array() {
        let mut part = MicrowireEeprom::new(128, 7);
        part.load(&[0x11, 0x22, 0x33]);
        let mut ctx = TestCtx::default();
        let out = part.execute(&mut ctx, MicrowireOp::Read, 1, &[]).unwrap();
        assert_eq!(&out[..2], &[0x22, 0x33]);
        assert_eq!(out.len(), 127);
    }

    #[test]
    fn eral_and_wral_cover_the_whole_array() {
        let mut part = MicrowireEeprom::new(64, 6);
        let mut ctx = TestCtx::default();
        part.execute(&mut ctx, MicrowireOp::Ewen, 0, &[]).unwrap();
        part.execute(&mut ctx, MicrowireOp::Wral, 0, &[0x0F]).unwrap();
        assert!(part.data().iter().all(|&b| b == 0x0F));
        part.execute(&mut ctx, MicrowireOp::Ewen, 0, &[]).unwrap();
        part.execute(&mut ctx, MicrowireOp::Eral, 0, &[]).unwrap();
        assert!(part.data().iter().all(|&b| b == 0xFF));
    }

    #[test]
    fn address_wraps_into_the_array() {
        let mut part = MicrowireEeprom::new(128, 7);
        let mut ctx = TestCtx::default();
        part.execute(&mut ctx, MicrowireOp::Ewen, 0, &[]).unwrap();
        // 7-bit field: 0x80 masks to 0x00.
        part.execute(&mut ctx, MicrowireOp::Write, 0x80, &[0x5A])
            .unwrap();
        assert_eq!(part.data()[0], 0x5A);
    }
}
