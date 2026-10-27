//! The VESA Bochs DISPI extension (QEMU's `hw/display/vbe.c` role): the
//! linear-framebuffer mode-set interface at ports `1CEh` (index) and
//! `1CFh` (data).
//!
//! Clean-room from the VBE Bochs extension register list: ID, XRES, YRES,
//! BPP, ENABLE, BANK, VIRT_WIDTH, VIRT_HEIGHT, XOFF, YOFF and the video
//! memory layout a mode-set describes. A mode becomes visible when ENABLE's
//! bit 0 (and the linear bit 6) are set; until then the aperture keeps the
//! VGA's own addressing.

/// DISPI register indices.
#[allow(dead_code)] // documented register indices
pub mod index {
    pub const ID: u16 = 0x00;
    pub const XRES: u16 = 0x01;
    pub const YRES: u16 = 0x02;
    pub const BPP: u16 = 0x03;
    pub const ENABLE: u16 = 0x04;
    pub const BANK: u16 = 0x05;
    pub const VIRT_WIDTH: u16 = 0x06;
    pub const VIRT_HEIGHT: u16 = 0x07;
    pub const X_OFFSET: u16 = 0x08;
    pub const Y_OFFSET: u16 = 0x09;
    pub const VIDEO_MEMORY_64K: u16 = 0x0A;
}

/// The Bochs VBE interface ID this model answers with.
pub const VBE_ID: u16 = 0xB0C5;
/// ENABLE bits: bit 0 enable, bit 1 linear, bit 6? (the DISPI's LFB bit is 6
/// in the extension; Bochs documents 0x40 as linear).
pub const ENABLE_ON: u16 = 1 << 0;
pub const ENABLE_LINEAR: u16 = 1 << 6;

/// The ports the DISPI decodes.
pub const VBE_INDEX_PORT: u16 = 0x1CE;
pub const VBE_DATA_PORT: u16 = 0x1CF;

/// The Bochs DISPI register set.
#[derive(Clone, Debug)]
pub struct VgaVbe {
    index: u16,
    id: u16,
    pub xres: u16,
    pub yres: u16,
    pub bpp: u16,
    pub enable: u16,
    pub bank: u16,
    pub virt_width: u16,
    pub virt_height: u16,
    pub x_offset: u16,
    pub y_offset: u16,
    /// The 64 KiB unit count the VIDEO_MEMORY register reports.
    video_memory_units: u16,
}

impl Default for VgaVbe {
    fn default() -> Self {
        Self::new()
    }
}

impl VgaVbe {
    pub fn new() -> Self {
        Self {
            index: 0,
            id: VBE_ID,
            xres: 0,
            yres: 0,
            bpp: 0,
            enable: 0,
            bank: 0,
            virt_width: 0,
            virt_height: 0,
            x_offset: 0,
            y_offset: 0,
            video_memory_units: 256, // 16 MiB of video memory
        }
    }

    /// Whether a linear-framebuffer mode is active (the machine maps the
    /// LFB window at its own chosen base when this is true).
    pub fn linear_mode_active(&self) -> bool {
        self.enable & ENABLE_ON != 0 && self.enable & ENABLE_LINEAR != 0
    }

    /// The byte size of one LFB frame at the current mode.
    pub fn frame_size(&self) -> u64 {
        if !self.linear_mode_active() {
            return 0;
        }
        let bytes_per_pixel = match self.bpp {
            4 => 1, // 4 bits packed
            8 => 1,
            15 | 16 => 2,
            24 => 3,
            32 => 4,
            _ => 1,
        };
        u64::from(self.xres) * u64::from(self.yres) * bytes_per_pixel
    }

    /// The virtual desktop's byte size (the surface the offsets pan over).
    pub fn virtual_frame_size(&self) -> u64 {
        if self.virt_width == 0 || self.virt_height == 0 {
            return self.frame_size();
        }
        let bytes_per_pixel = match self.bpp {
            15 | 16 => 2,
            24 => 3,
            32 => 4,
            _ => 1,
        };
        u64::from(self.virt_width) * u64::from(self.virt_height) * bytes_per_pixel
    }

    pub fn read_port(&mut self, port: u16) -> u32 {
        u32::from(match port {
            VBE_INDEX_PORT => self.index,
            VBE_DATA_PORT => match self.index {
                index::ID => self.id,
                index::XRES => self.xres,
                index::YRES => self.yres,
                index::BPP => self.bpp,
                index::ENABLE => self.enable,
                index::BANK => self.bank,
                index::VIRT_WIDTH => self.virt_width,
                index::VIRT_HEIGHT => self.virt_height,
                index::X_OFFSET => self.x_offset,
                index::Y_OFFSET => self.y_offset,
                index::VIDEO_MEMORY_64K => self.video_memory_units,
                _ => 0,
            },
            _ => 0,
        })
    }

    pub fn write_port(&mut self, port: u16, value: u16) {
        match port {
            VBE_INDEX_PORT => self.index = value & 0x0F,
            VBE_DATA_PORT => match self.index {
                index::ID => {
                    // Writing a supported ID acknowledges the extension.
                    if value == VBE_ID || value == 0xB0C0 || value == 0xB0C4 {
                        self.id = value;
                    }
                }
                index::XRES => {
                    if self.enable & ENABLE_ON == 0 {
                        self.xres = value.min(8192);
                        if self.virt_width < self.xres {
                            self.virt_width = self.xres;
                        }
                    }
                }
                index::YRES => {
                    if self.enable & ENABLE_ON == 0 {
                        self.yres = value.min(8192);
                        if self.virt_height < self.yres {
                            self.virt_height = self.yres;
                        }
                    }
                }
                index::BPP => {
                    if self.enable & ENABLE_ON == 0 {
                        self.bpp = value;
                    }
                }
                index::ENABLE => {
                    // Bit 0 set: apply the pending mode. The driver's set
                    // sequence (XRES, YRES, BPP, then ENABLE) is enforced by
                    // the locks above.
                    self.enable = value;
                }
                index::BANK => self.bank = value,
                index::VIRT_WIDTH => self.virt_width = value.max(self.xres),
                index::VIRT_HEIGHT => self.virt_height = value.max(self.yres),
                index::X_OFFSET => self.x_offset = value,
                index::Y_OFFSET => self.y_offset = value,
                _ => {}
            },
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_id_register_answers_the_bochs_id() {
        let mut vbe = VgaVbe::new();
        vbe.write_port(VBE_INDEX_PORT, index::ID);
        assert_eq!(vbe.read_port(VBE_DATA_PORT), u32::from(VBE_ID));
    }

    #[test]
    fn a_mode_set_sequence_activates_the_linear_framebuffer() {
        let mut vbe = VgaVbe::new();
        vbe.write_port(VBE_INDEX_PORT, index::XRES);
        vbe.write_port(VBE_DATA_PORT, 640);
        vbe.write_port(VBE_INDEX_PORT, index::YRES);
        vbe.write_port(VBE_DATA_PORT, 480);
        vbe.write_port(VBE_INDEX_PORT, index::BPP);
        vbe.write_port(VBE_DATA_PORT, 32);
        assert!(!vbe.linear_mode_active());
        vbe.write_port(VBE_INDEX_PORT, index::ENABLE);
        vbe.write_port(VBE_DATA_PORT, ENABLE_ON | ENABLE_LINEAR);
        assert!(vbe.linear_mode_active());
        assert_eq!(vbe.frame_size(), 640 * 480 * 4);
    }

    #[test]
    fn geometry_locks_while_enabled() {
        let mut vbe = VgaVbe::new();
        vbe.write_port(VBE_INDEX_PORT, index::XRES);
        vbe.write_port(VBE_DATA_PORT, 800);
        vbe.write_port(VBE_INDEX_PORT, index::ENABLE);
        vbe.write_port(VBE_DATA_PORT, ENABLE_ON);
        vbe.write_port(VBE_INDEX_PORT, index::XRES);
        vbe.write_port(VBE_DATA_PORT, 1024); // ignored while enabled
        vbe.write_port(VBE_INDEX_PORT, index::XRES);
        assert_eq!(vbe.read_port(VBE_DATA_PORT), 800);
    }
}
