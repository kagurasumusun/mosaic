//! `pata` bus entity (ADR 0025): PATA/IDE parallel bus (ATA/ATAPI command block).

use mosaic_contract::model::Ctx;
use mosaic_contract::pata::{PataError, PataInitiator, PataTarget};

pub struct PataBus {
    devices: [Option<Box<dyn PataTarget>>; 2],
    selected: u8,
}

impl Default for PataBus {
    fn default() -> Self {
        Self {
            devices: [None, None],
            selected: 0,
        }
    }
}

impl PataBus {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn attach(&mut self, dev: u8, t: Box<dyn PataTarget>) {
        if (dev as usize) < 2 {
            self.devices[dev as usize] = Some(t);
        }
    }
    fn cur(&mut self) -> Result<&mut Box<dyn PataTarget>, PataError> {
        self.devices[self.selected as usize]
            .as_mut()
            .ok_or(PataError::NoDevice)
    }
    pub fn select(&mut self, dev: u8) -> Result<(), PataError> {
        if (dev as usize) >= 2 || self.devices[dev as usize].is_none() {
            return Err(PataError::NoDevice);
        }
        self.selected = dev;
        Ok(())
    }
}

impl PataInitiator for PataBus {
    fn write_reg(&mut self, ctx: &mut dyn Ctx, reg: u8, value: u8) -> Result<(), PataError> {
        self.cur()?.write_reg(ctx, reg, value)
    }
    fn read_reg(&mut self, ctx: &mut dyn Ctx, reg: u8) -> Result<u8, PataError> {
        self.cur()?.read_reg(ctx, reg)
    }
    fn pio_data(
        &mut self,
        ctx: &mut dyn Ctx,
        _dev: u8,
        write: bool,
        buf: &mut [u16],
    ) -> Result<usize, PataError> {
        self.cur()?.pio_data(ctx, write, buf)
    }
}
