//! The VGA register file (CRTC, sequencer, graphics controller, attribute
//! controller, miscellaneous output) behind ports `3B0h..3DFh`.
//!
//! Clean-room from IBM's VGA/XGA technical reference and the register
//! summaries every VGA documentation set carries: the CRTC's 25 registers
//! (cursor location, start address, vertical total and the mode-3 defaults a
//! text console reads), the sequencer's 5, the graphics controller's 9
//! (including the read map select and chain4 the apertures decode with), the
//! attribute controller's 21 (including the palette RAM an attribute write
//! fills), the miscellaneous output register, and the two input-status
//! registers (the retrace bit of status 1 alternates so a poller can never
//! hang; the attribute address flip-flop toggles the way the shared
//! `3C0h` port demands).

/// CRTC register indices (the standard subset every driver programs).
#[allow(dead_code)] // documented CRTC indices
pub mod crtc {
    pub const HORIZONTAL_TOTAL: u8 = 0x00;
    pub const HORIZONTAL_DISPLAY_END: u8 = 0x01;
    pub const START_HORIZONTAL_BLANK: u8 = 0x02;
    pub const END_HORIZONTAL_BLANK: u8 = 0x03;
    pub const START_HORIZONTAL_RETRACE: u8 = 0x04;
    pub const END_HORIZONTAL_RETRACE: u8 = 0x05;
    pub const VERTICAL_TOTAL: u8 = 0x06;
    pub const OVERFLOW: u8 = 0x07;
    pub const PRESET_ROW_SCAN: u8 = 0x08;
    pub const MAXIMUM_SCAN_LINE: u8 = 0x09;
    pub const CURSOR_START: u8 = 0x0A;
    pub const CURSOR_END: u8 = 0x0B;
    pub const START_ADDRESS_HIGH: u8 = 0x0C;
    pub const START_ADDRESS_LOW: u8 = 0x0D;
    pub const CURSOR_LOCATION_HIGH: u8 = 0x0E;
    pub const CURSOR_LOCATION_LOW: u8 = 0x0F;
    pub const VERTICAL_RETRACE_START: u8 = 0x10;
    pub const VERTICAL_RETRACE_END: u8 = 0x11;
    pub const VERTICAL_DISPLAY_END: u8 = 0x12;
    pub const OFFSET: u8 = 0x13;
    pub const UNDERLINE_LOCATION: u8 = 0x14;
    pub const START_VERTICAL_BLANK: u8 = 0x15;
    pub const END_VERTICAL_BLANK: u8 = 0x16;
    pub const MODE_CONTROL: u8 = 0x17;
    pub const LINE_COMPARE: u8 = 0x18;
}

/// Sequencer register indices.
#[allow(dead_code)] // documented sequencer indices
pub mod seq {
    pub const RESET: u8 = 0x00;
    pub const CLOCKING_MODE: u8 = 0x01;
    pub const MAP_MASK: u8 = 0x02;
    pub const CHARACTER_MAP_SELECT: u8 = 0x03;
    pub const MEMORY_MODE: u8 = 0x04;
}

/// Graphics controller register indices.
#[allow(dead_code)] // documented graphics controller indices
pub mod gc {
    pub const SET_RESET: u8 = 0x00;
    pub const ENABLE_SET_RESET: u8 = 0x01;
    pub const COLOR_COMPARE: u8 = 0x02;
    pub const DATA_ROTATE: u8 = 0x03;
    pub const READ_MAP_SELECT: u8 = 0x04;
    pub const MODE: u8 = 0x05;
    pub const MISCELLANEOUS: u8 = 0x06;
    pub const COLOR_DONT_CARE: u8 = 0x07;
    pub const BIT_MASK: u8 = 0x08;
}

/// Attribute controller register indices (0..15 palette, then the control
/// registers).
#[allow(dead_code)] // documented attribute controller indices
pub mod attr {
    pub const PALETTE: u8 = 0x00; // ..= 0x0F
    pub const MODE_CONTROL: u8 = 0x10;
    pub const OVERSCAN_COLOR: u8 = 0x11;
    pub const COLOR_PLANE_ENABLE: u8 = 0x12;
    pub const HORIZONTAL_PEL_PANNING: u8 = 0x13;
    pub const COLOR_SELECT: u8 = 0x14;
}

/// Ports the register file decodes (VGA color addressing; mono boards alias
/// at `3B4h` — both decode here).
#[allow(dead_code)] // documented port numbers
pub mod port {
    pub const CRTC_INDEX: u16 = 0x3D4;
    pub const CRTC_INDEX_MONO: u16 = 0x3B4;
    pub const CRTC_DATA: u16 = 0x3D5;
    pub const CRTC_DATA_MONO: u16 = 0x3B5;
    pub const SEQUENCER_INDEX: u16 = 0x3C4;
    pub const SEQUENCER_DATA: u16 = 0x3C5;
    pub const DAC_WRITE_INDEX: u16 = 0x3C8;
    pub const DAC_DATA: u16 = 0x3C9;
    pub const FEATURE_CONTROL: u16 = 0x3CA;
    pub const MISC_OUTPUT_WRITE: u16 = 0x3C2;
    pub const MISC_OUTPUT_READ: u16 = 0x3CC;
    pub const GRAPHICS_INDEX: u16 = 0x3CE;
    pub const GRAPHICS_DATA: u16 = 0x3CF;
    pub const ATTRIBUTE_INDEX: u16 = 0x3C0;
    pub const ATTRIBUTE_DATA: u16 = 0x3C1;
    pub const INPUT_STATUS_0: u16 = 0x3C2; // read alias of MISC write
    pub const INPUT_STATUS_1_MONO: u16 = 0x3BA;
    pub const INPUT_STATUS_1: u16 = 0x3DA;
}

/// The whole VGA register file. `read_port`/`write_port` are the machine's
/// port-routing surface.
#[derive(Clone, Debug)]
pub struct VgaRegisters {
    crtc_index: u8,
    pub crtc: [u8; 25],
    sequencer_index: u8,
    pub sequencer: [u8; 5],
    graphics_index: u8,
    pub graphics: [u8; 9],
    /// The attribute controller's address flip-flop: writes at `3C0h`
    /// alternate index/data when bit 5 of the value is clear.
    attribute_flip: bool,
    attribute_index: u8,
    pub attribute: [u8; 21],
    /// The palette RAM (16 entries x 6 bits).
    pub palette: [u8; 16],
    misc_output: u8,
    feature_control: u8,
    /// The retrace toggle every `INPUT_STATUS_1` read flips.
    retrace: bool,
}

impl Default for VgaRegisters {
    fn default() -> Self {
        let mut registers = Self {
            crtc_index: 0,
            crtc: [0; 25],
            sequencer_index: 0,
            sequencer: [0x03, 0x00, 0x03, 0x00, 0x02],
            graphics_index: 0,
            graphics: [0; 9],
            attribute_flip: false,
            attribute_index: 0,
            attribute: [0; 21],
            palette: [0; 16],
            misc_output: 0x67, // color I/O addressing, 25.175 MHz clock
            feature_control: 0,
            retrace: false,
        };
        // Mode 3 (80x25 text) CRTC state as a video BIOS leaves it: 80
        // columns, 400 scanlines, cursor visible, display from offset 0.
        registers.crtc[crtc::HORIZONTAL_TOTAL as usize] = 0x71;
        registers.crtc[crtc::HORIZONTAL_DISPLAY_END as usize] = 0x4F; // 80 cells - 1
        registers.crtc[crtc::CURSOR_START as usize] = 0x0E;
        registers.crtc[crtc::CURSOR_END as usize] = 0x0F;
        registers.crtc[crtc::START_ADDRESS_HIGH as usize] = 0x00;
        registers.crtc[crtc::START_ADDRESS_LOW as usize] = 0x00;
        registers.crtc[crtc::CURSOR_LOCATION_HIGH as usize] = 0x00;
        registers.crtc[crtc::CURSOR_LOCATION_LOW as usize] = 0x00;
        registers.crtc[crtc::VERTICAL_DISPLAY_END as usize] = 0x8F; // 399 low 8 bits
        registers.crtc[crtc::OVERFLOW as usize] = 0x20; // VDE bit 8 (400 lines)
        registers.crtc[crtc::OFFSET as usize] = 0x28; // 40 words per row
        registers.crtc[MAXIMUM_SCAN_LINE_INDEX] = 0x4F; // 16-line cells
        registers
    }
}

const MAXIMUM_SCAN_LINE_INDEX: usize = crtc::MAXIMUM_SCAN_LINE as usize;

impl VgaRegisters {
    /// Whether the attribute controller selected a text (alpha) mode: its
    /// mode control register bit 0 — the one bit every mode list keys on.
    pub fn is_text_mode(&self) -> bool {
        self.attribute[attr::MODE_CONTROL as usize] & 0x01 == 0
    }

    /// The text columns/rows the CRTC describes (mode 3: 80x25). The
    /// vertical display end's bit 8 lives in the overflow register.
    pub fn text_geometry(&self) -> (u32, u32) {
        let columns = u32::from(self.crtc[crtc::HORIZONTAL_DISPLAY_END as usize]) + 1;
        let mut scanlines = u32::from(self.crtc[crtc::VERTICAL_DISPLAY_END as usize]);
        if self.crtc[crtc::OVERFLOW as usize] & 0x20 != 0 {
            scanlines += 256;
        }
        scanlines += 1;
        let cell = u32::from(self.crtc[MAXIMUM_SCAN_LINE_INDEX] & 0x1F) + 1;
        (columns.min(100), (scanlines / cell.max(1)).min(128))
    }

    /// The character map select's active font plane bits (sequencer 3).
    pub fn character_map(&self) -> u8 {
        self.sequencer[seq::CHARACTER_MAP_SELECT as usize]
    }

    /// The sequencer's chain-4 bit (memory mode bit 3): chained modes
    /// address planes by A0/A1, planar modes by the full offset.
    pub fn chain4(&self) -> bool {
        self.sequencer[seq::MEMORY_MODE as usize] & 0x08 != 0
    }

    pub fn read_map_select(&self) -> u8 {
        self.graphics[gc::READ_MAP_SELECT as usize] & 0x03
    }

    pub fn map_mask(&self) -> u8 {
        self.sequencer[seq::MAP_MASK as usize] & 0x0F
    }

    /// The start address the CRTC displays from (word offset in video RAM).
    pub fn start_address(&self) -> u32 {
        (u32::from(self.crtc[crtc::START_ADDRESS_HIGH as usize]) << 8)
            | u32::from(self.crtc[crtc::START_ADDRESS_LOW as usize])
    }

    /// The cursor location (word offset in video RAM).
    pub fn cursor_location(&self) -> u32 {
        (u32::from(self.crtc[crtc::CURSOR_LOCATION_HIGH as usize]) << 8)
            | u32::from(self.crtc[crtc::CURSOR_LOCATION_LOW as usize])
    }

    /// One port read. Unclaimed ports read as `FFh`, as an idle ISA bus
    /// does — a probing driver must see "no device".
    pub fn read_port(&mut self, port: u16) -> u32 {
        u32::from(match port {
            port::CRTC_INDEX | port::CRTC_INDEX_MONO => self.crtc_index,
            port::CRTC_DATA | port::CRTC_DATA_MONO => {
                self.crtc[usize::from(self.crtc_index).min(24)]
            }
            port::SEQUENCER_INDEX => self.sequencer_index,
            port::SEQUENCER_DATA => self.sequencer[usize::from(self.sequencer_index) % 5],
            port::GRAPHICS_INDEX => self.graphics_index,
            port::GRAPHICS_DATA => self.graphics[usize::from(self.graphics_index) % 9],
            port::ATTRIBUTE_INDEX | port::ATTRIBUTE_DATA => {
                // With the flip-flop set, reads return the palette; the
                // index itself otherwise.
                if port == port::ATTRIBUTE_DATA && !self.attribute_flip {
                    self.palette[usize::from(self.attribute_index) & 0x0F]
                } else {
                    self.attribute_index & 0x1F
                }
            }
            port::MISC_OUTPUT_READ => self.misc_output,
            port::INPUT_STATUS_0 => 0x00,
            port::INPUT_STATUS_1 | port::INPUT_STATUS_1_MONO => {
                // The retrace bit alternates so neither retrace poll can
                // spin forever (bit 3 vertical retrace, bit 0 display
                // enable).
                self.retrace = !self.retrace;
                if self.retrace {
                    0x08
                } else {
                    0x00
                }
            }
            port::FEATURE_CONTROL => self.feature_control,
            port::DAC_WRITE_INDEX | port::DAC_DATA => 0x00,
            _ => 0xFF,
        })
    }

    /// One port write.
    pub fn write_port(&mut self, port: u16, value: u8) {
        match port {
            port::CRTC_INDEX | port::CRTC_INDEX_MONO => self.crtc_index = value & 0x1F,
            port::CRTC_DATA | port::CRTC_DATA_MONO => {
                self.crtc[usize::from(self.crtc_index).min(24)] = value;
            }
            port::SEQUENCER_INDEX => self.sequencer_index = value & 0x07,
            port::SEQUENCER_DATA => {
                self.sequencer[usize::from(self.sequencer_index) % 5] = value;
            }
            port::GRAPHICS_INDEX => self.graphics_index = value & 0x0F,
            port::GRAPHICS_DATA => {
                self.graphics[usize::from(self.graphics_index) % 9] = value;
            }
            port::ATTRIBUTE_INDEX | port::ATTRIBUTE_DATA => {
                // 3C0h is the shared index/data port: bit 5 clear after a
                // first write selects the index (and toggles the flip-flop
                // for the next write), bit 5 set passes a data value.
                self.attribute_flip = !self.attribute_flip;
                if self.attribute_flip {
                    self.attribute_index = value & 0x1F;
                } else if self.attribute_index <= attr::PALETTE + 0x0F {
                    self.palette[usize::from(self.attribute_index) & 0x0F] = value & 0x3F;
                } else {
                    self.attribute[usize::from(self.attribute_index).min(20)] = value;
                }
            }
            port::MISC_OUTPUT_WRITE => self.misc_output = value,
            port::FEATURE_CONTROL | port::DAC_WRITE_INDEX | port::DAC_DATA => {}
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode3_defaults_describe_80x25() {
        let registers = VgaRegisters::default();
        assert!(registers.is_text_mode());
        assert_eq!(registers.text_geometry(), (80, 25));
        assert_eq!(registers.start_address(), 0);
    }

    #[test]
    fn the_retrace_bit_alternates() {
        let mut registers = VgaRegisters::default();
        let first = registers.read_port(port::INPUT_STATUS_1) as u8;
        let second = registers.read_port(port::INPUT_STATUS_1) as u8;
        assert_ne!(first & 0x08, second & 0x08);
    }

    #[test]
    fn the_crtc_cursor_moves_by_index_and_data() {
        let mut registers = VgaRegisters::default();
        registers.write_port(port::CRTC_INDEX, crtc::CURSOR_LOCATION_HIGH);
        registers.write_port(port::CRTC_DATA, 0x00);
        registers.write_port(port::CRTC_INDEX, crtc::CURSOR_LOCATION_LOW);
        registers.write_port(port::CRTC_DATA, 0x0A);
        assert_eq!(registers.cursor_location(), 0x000A);
    }

    #[test]
    fn unclaimed_ports_read_idle() {
        let mut registers = VgaRegisters::default();
        assert_eq!(registers.read_port(0x3E0), 0xFF);
    }
}
