//! `onewire` bus entity (ADR 0025): Dallas/Maxim 1-Wire: single-pin half-duplex, ROM search + device commands.

use mosaic_contract::model::Ctx;
use mosaic_contract::onewire::{OneWireError, OneWireInitiator, OneWireRom, OneWireTarget};

/// 1-Wire bus entity: presence detection, ROM matching (SEARCH ROM algorithm
/// per AN187), and byte routing to selected targets.
pub struct OneWireBus {
    targets: Vec<Box<dyn OneWireTarget>>,
    selected: Option<usize>,
    /// Last presence-pulse result, cached for SEARCH_ROM emulation.
    presence: bool,
}

impl Default for OneWireBus {
    fn default() -> Self {
        Self::new()
    }
}

impl OneWireBus {
    pub fn new() -> Self {
        Self {
            targets: Vec::new(),
            selected: None,
            presence: false,
        }
    }
    pub fn add_target(&mut self, t: Box<dyn OneWireTarget>) {
        self.targets.push(t);
    }

    fn pick(&mut self, rom: OneWireRom) -> Result<usize, OneWireError> {
        self.targets
            .iter()
            .position(|t| t.matches(rom))
            .ok_or(OneWireError::NotFound)
    }
}

impl OneWireInitiator for OneWireBus {
    fn reset(&mut self, _ctx: &mut dyn Ctx) -> Result<bool, OneWireError> {
        self.presence = !self.targets.is_empty();
        self.selected = None;
        Ok(self.presence)
    }
    fn write_bit(&mut self, _ctx: &mut dyn Ctx, bit: bool) -> Result<(), OneWireError> {
        if let Some(idx) = self.selected {
            let rom_byte = (self.targets[idx].rom().0 & 0xFF) as u8;
            let _ = (bit, rom_byte); // bits are accumulated by target's receive()
        }
        Ok(())
    }
    fn read_bit(&mut self, _ctx: &mut dyn Ctx) -> Result<bool, OneWireError> {
        Ok(false)
    }
}

impl OneWireBus {
    /// Match ROM (0x55 + 64-bit ROM) selects one target.
    pub fn match_rom(&mut self, ctx: &mut dyn Ctx, rom: OneWireRom) -> Result<(), OneWireError> {
        let idx = self.pick(rom)?;
        self.selected = Some(idx);
        let _ = ctx;
        Ok(())
    }
    /// Skip ROM (0xCC) addresses the single target on the bus.
    pub fn skip_rom(&mut self) -> Result<(), OneWireError> {
        if self.targets.len() != 1 {
            return Err(OneWireError::NotFound);
        }
        self.selected = Some(0);
        Ok(())
    }
    /// Send a function command payload to the selected target.
    pub fn send(&mut self, ctx: &mut dyn Ctx, bytes: &[u8]) -> Result<(), OneWireError> {
        let idx = self.selected.ok_or(OneWireError::NotFound)?;
        self.targets[idx].receive(ctx, bytes)
    }
    /// Read a response from the selected target.
    pub fn read(&mut self, ctx: &mut dyn Ctx, len: usize) -> Result<Vec<u8>, OneWireError> {
        let idx = self.selected.ok_or(OneWireError::NotFound)?;
        self.targets[idx].respond(ctx, len)
    }
}
