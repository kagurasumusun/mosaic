//! The VGA video memory: 256 KiB, four planes, the `A0000h..BFFFFh`
//! apertures and the text-mode screen renderer.
//!
//! The aperture addressing follows the register file's mode state: text
//! modes address the even/odd planes as `B8000h`-based cells, planar
//! graphics modes address `A0000h`-based planes (sequential or chain4 by the
//! sequencer's memory mode), and the odd/even mode splits planes by address
//! bit 0. Writes honor the sequencer's map mask; reads honor the graphics
//! controller's read map select — the addressing rules every VGA driver
//! programs, implemented once for every machine that has a VGA.

use crate::ip::display::vga::regs::VgaRegisters;
use mosaic_contract::mmio::MmioTarget;
use mosaic_contract::model::Ctx;
use mosaic_exec::{AccessSize, Attr, BusFault};

/// 256 KiB of video memory: four 64 KiB planes.
pub const VRAM_SIZE: usize = 256 * 1024;
/// The `A0000h..BFFFFh` aperture.
pub const APERTURE_BASE: u64 = 0x000A_0000;
pub const APERTURE_SIZE: u64 = 0x0002_0000;
/// The `B8000h` text window (mode 3's two planes).
pub const TEXT_WINDOW_BASE: u64 = 0x000B_8000;
/// The default linear framebuffer base a board maps the LFB at (VBE modes).
pub const LFB_DEFAULT_BASE: u64 = 0xE000_0000;
/// The default LFB size (16 MiB, the most a VBE mode asks of a PC).
pub const LFB_DEFAULT_SIZE: u64 = 0x0100_0000;

/// Which addressing mode the aperture is in.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VgaMode {
    /// Text mode: `B8000h`-relative, even/odd planes (mode 3's 80x25).
    Text,
    /// Planar graphics: `A0000h`-relative, plane selected by read map /
    /// written by map mask.
    Planar,
    /// Chained (chain4/odd-even) graphics: sequential addressing into all
    /// four planes.
    Chained,
}

/// The video memory and its aperture decode.
#[derive(Clone, Debug)]
pub struct VgaMemory {
    /// Four 64 KiB planes.
    planes: [Vec<u8>; 4],
}

impl Default for VgaMemory {
    fn default() -> Self {
        Self::new()
    }
}

impl VgaMemory {
    pub fn new() -> Self {
        Self {
            planes: [
                vec![0; VRAM_SIZE / 4],
                vec![0; VRAM_SIZE / 4],
                vec![0; VRAM_SIZE / 4],
                vec![0; VRAM_SIZE / 4],
            ],
        }
    }

    /// One plane's bytes (0..4), the diagnostic and LFB path.
    pub fn plane(&self, plane: usize) -> &[u8] {
        &self.planes[plane & 0x3]
    }

    /// The addressing mode the register file's state selects.
    pub fn mode(registers: &VgaRegisters) -> VgaMode {
        if registers.is_text_mode() {
            VgaMode::Text
        } else if registers.chain4() {
            VgaMode::Chained
        } else {
            VgaMode::Planar
        }
    }

    /// Read one aperture byte at `address` (`A0000h..C0000h`) under the
    /// register file's mode.
    pub fn read_aperture(&self, registers: &VgaRegisters, address: u64) -> u8 {
        if !(APERTURE_BASE..APERTURE_BASE + APERTURE_SIZE).contains(&address) {
            return 0xFF;
        }
        let offset = (address - APERTURE_BASE) as usize;
        match Self::mode(registers) {
            VgaMode::Text => {
                let plane = plane_for_text(offset);
                // Text cells: even offsets the character plane, odd offsets
                // the attribute plane, each plane indexed by cell number.
                let within = ((offset & 0x7FFF) >> 1) % (VRAM_SIZE / 4);
                self.planes[plane][within]
            }
            VgaMode::Planar => {
                let plane = usize::from(registers.read_map_select());
                self.planes[plane][offset % (VRAM_SIZE / 4)]
            }
            VgaMode::Chained => {
                let plane = offset & 0x3;
                let within = (offset >> 2) % (VRAM_SIZE / 4);
                self.planes[plane][within]
            }
        }
    }

    /// Write one aperture byte honoring the sequencer's map mask.
    pub fn write_aperture(&mut self, registers: &VgaRegisters, address: u64, value: u8) {
        if !(APERTURE_BASE..APERTURE_BASE + APERTURE_SIZE).contains(&address) {
            return;
        }
        let offset = (address - APERTURE_BASE) as usize;
        match Self::mode(registers) {
            VgaMode::Text => {
                let plane = plane_for_text(offset);
                let mask = map_mask_for(registers, plane);
                if mask {
                    let within = ((offset & 0x7FFF) >> 1) % (VRAM_SIZE / 4);
                    self.planes[plane][within] = value;
                }
            }
            VgaMode::Planar => {
                for plane in 0..4u8 {
                    if registers.map_mask() & (1 << plane) != 0 {
                        let within = offset % (VRAM_SIZE / 4);
                        self.planes[plane as usize][within] = value;
                    }
                }
            }
            VgaMode::Chained => {
                let plane = offset & 0x3;
                if registers.map_mask() & (1 << plane) != 0 {
                    let within = (offset >> 2) % (VRAM_SIZE / 4);
                    self.planes[plane][within] = value;
                }
            }
        }
    }

    /// Linear framebuffer access for VBE modes: 4 bytes per pixel stride,
    /// sequential across all planes.
    pub fn read_lfb(&self, offset: u64) -> u8 {
        let offset = offset as usize;
        if offset >= VRAM_SIZE {
            return 0;
        }
        self.planes[0][offset % (VRAM_SIZE / 4)]
            | self.planes[offset / (VRAM_SIZE / 4) % 4][offset % (VRAM_SIZE / 4)]
    }

    pub fn write_lfb(&mut self, offset: u64, value: u8) {
        let offset = offset as usize;
        if offset >= VRAM_SIZE {
            return;
        }
        let within = offset % (VRAM_SIZE / 4);
        self.planes[0][within] = value;
        self.planes[offset / (VRAM_SIZE / 4) % 4][within] = value;
    }

    /// The text-mode screen as printable rows (80x25 in mode 3), read from
    /// the text window the CRTC points at. The attribute byte becomes
    /// nothing here — a text dump is characters, the way `--screen-dump`
    /// reports and a serial-less boot shows.
    pub fn screen_text(&self, registers: &VgaRegisters) -> Vec<String> {
        let (columns, rows) = registers.text_geometry();
        let start = registers.start_address() as usize;
        let mut lines = Vec::new();
        for row in 0..rows {
            let mut line = String::new();
            for column in 0..columns {
                let cell = start + (row * columns + column) as usize;
                // Text cells alternate even plane (character) and odd plane
                // (attribute) in the B8000h window.
                let character = self.planes[0][cell % (VRAM_SIZE / 4)];
                let byte = if character.is_ascii_graphic() || character == b' ' {
                    character
                } else {
                    b' '
                };
                line.push(byte as char);
            }
            lines.push(line.trim_end().to_string());
        }
        while lines.last().is_some_and(|line| line.is_empty()) {
            lines.pop();
        }
        lines
    }
}

/// The plane a text-mode aperture offset addresses: even offsets the
/// character plane (0), odd offsets the attribute plane (1).
fn plane_for_text(offset: usize) -> usize {
    offset & 1
}

/// The map mask for one text plane (the sequencer bit the plane maps to).
fn map_mask_for(registers: &VgaRegisters, plane: usize) -> bool {
    registers.map_mask() & (1 << plane) != 0 || registers.map_mask() == 0
}

/// The aperture as an [`MmioTarget`]: a machine maps `A0000h..C0000h` here
/// and hands the register file alongside (the mode state the decode reads).
pub struct VgaAperture<'a> {
    pub memory: &'a mut VgaMemory,
    pub registers: &'a VgaRegisters,
}

impl MmioTarget for VgaAperture<'_> {
    fn read(
        &mut self,
        _ctx: &mut dyn Ctx,
        offset: u64,
        size: AccessSize,
        _attr: &Attr,
    ) -> Result<u64, BusFault> {
        if size != AccessSize::Byte {
            // VGA aperture accesses are byte-wise through the sequencer
            // modes; a wider read is a driver bug, not a bus fault.
            let mut value = 0u64;
            for index in 0..size.bytes() {
                value |= u64::from(
                    self.memory
                        .read_aperture(self.registers, APERTURE_BASE + offset + u64::from(index)),
                ) << (index * 8);
            }
            return Ok(value);
        }
        Ok(u64::from(
            self.memory
                .read_aperture(self.registers, APERTURE_BASE + offset),
        ))
    }

    fn write(
        &mut self,
        _ctx: &mut dyn Ctx,
        offset: u64,
        size: AccessSize,
        value: u64,
        _attr: &Attr,
    ) -> Result<(), BusFault> {
        for index in 0..u64::from(size.bytes()) {
            let byte = (value >> (index * 8)) as u8;
            self.memory
                .write_aperture(self.registers, APERTURE_BASE + offset + index, byte);
        }
        Ok(())
    }
}

#[cfg(test)]
impl VgaMemory {
    fn plane_mut_for_test(&mut self, plane: usize) -> &mut [u8] {
        &mut self.planes[plane & 0x3]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registers() -> VgaRegisters {
        VgaRegisters::default()
    }

    /// The default registers with the attribute controller's graphics bit
    /// set (the mode a planar graphics driver programs).
    fn graphics_registers() -> VgaRegisters {
        let mut registers = VgaRegisters::default();
        registers.attribute[crate::ip::display::vga::regs::attr::MODE_CONTROL as usize] = 0x01;
        registers
    }

    #[test]
    fn text_cells_land_in_the_even_and_odd_planes() {
        let registers = registers();
        let mut memory = VgaMemory::new();
        // The mode-3 window is B8000h: cell 0 (character 'A') then attribute.
        memory.write_aperture(&registers, TEXT_WINDOW_BASE, b'A');
        memory.write_aperture(&registers, TEXT_WINDOW_BASE + 1, 0x07);
        assert_eq!(memory.plane(0)[0], b'A');
        assert_eq!(memory.plane(1)[0], 0x07);
    }

    #[test]
    fn the_screen_renders_the_crtc_window() {
        let mut registers = registers();
        let mut memory = VgaMemory::new();
        for (index, character) in b"MOSAIC VGA".iter().enumerate() {
            memory.write_aperture(&registers, TEXT_WINDOW_BASE + index as u64 * 2, *character);
        }
        let screen = memory.screen_text(&registers);
        assert_eq!(screen.first().map(String::as_str), Some("MOSAIC VGA"));
        // Moving the CRTC start address moves the window (word offset).
        registers.crtc[crate::ip::display::vga::regs::crtc::START_ADDRESS_LOW as usize] = 0x01;
        let screen = memory.screen_text(&registers);
        assert_eq!(screen.first().map(String::as_str), Some("OSAIC VGA"));
    }

    #[test]
    fn planar_reads_follow_the_read_map_select() {
        let mut registers = graphics_registers();
        let mut memory = VgaMemory::new();
        // Planar graphics: chain4 off, odd/even off, A000h aperture.
        registers.sequencer[crate::ip::display::vga::regs::seq::MEMORY_MODE as usize] = 0x06;
        let plane = memory.plane_mut_for_test(2);
        plane[0x10] = 0xAB;
        registers.graphics[crate::ip::display::vga::regs::gc::READ_MAP_SELECT as usize] = 0x02;
        let value = memory.read_aperture(&registers, APERTURE_BASE + 0x10);
        assert_eq!(value, 0xAB);
    }

    #[test]
    fn the_map_mask_gates_planar_writes() {
        let mut registers = graphics_registers();
        registers.sequencer[crate::ip::display::vga::regs::seq::MEMORY_MODE as usize] = 0x06;
        let mut memory = VgaMemory::new();
        // Only plane 1 in the mask.
        registers.sequencer[crate::ip::display::vga::regs::seq::MAP_MASK as usize] = 0x02;
        memory.write_aperture(&registers, APERTURE_BASE, 0x5A);
        assert_eq!(memory.plane(1)[0], 0x5A);
        assert_eq!(memory.plane(0)[0], 0x00);
        assert_eq!(memory.plane(2)[0], 0x00);
    }
}
