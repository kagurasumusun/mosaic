#![forbid(unsafe_code)]
//! SPI bus entity: chip-select management and full-duplex byte exchange.
//!
//! One [`SpiBus`] maps each chip-select line to exactly one
//! [`mosaic_contract_spi::SpiTarget`]. Clock mode travels with every transfer
//! so targets with different CPOL/CPHA requirements can share the controller.

use std::collections::BTreeMap;

use mosaic_contract_model::Ctx;
use mosaic_contract_spi::{SpiCs, SpiError, SpiInitiator, SpiMode, SpiTarget};

/// Addressed SPI bus. CS lines are assigned when targets are registered.
pub struct SpiBus {
    targets: BTreeMap<u8, Box<dyn SpiTarget>>,
    selected: Option<u8>,
}

impl Default for SpiBus {
    fn default() -> Self {
        Self::new()
    }
}

impl SpiBus {
    pub fn new() -> Self {
        Self {
            targets: BTreeMap::new(),
            selected: None,
        }
    }

    pub fn add_target(&mut self, cs: SpiCs, target: Box<dyn SpiTarget>) {
        self.targets.insert(cs.0, target);
    }

    pub fn is_selected(&self, cs: SpiCs) -> bool {
        self.selected == Some(cs.0)
    }

    fn target(&mut self, cs: SpiCs) -> Result<&mut Box<dyn SpiTarget>, SpiError> {
        self.targets.get_mut(&cs.0).ok_or(SpiError::NoTarget)
    }
}

impl SpiInitiator for SpiBus {
    fn select(&mut self, ctx: &mut dyn Ctx, cs: SpiCs, assert: bool) -> Result<(), SpiError> {
        if assert {
            let target = self.target(cs)?;
            target.selected(ctx, true);
            self.selected = Some(cs.0);
        } else if self.selected == Some(cs.0) {
            let target = self.target(cs)?;
            target.selected(ctx, false);
            self.selected = None;
        }
        Ok(())
    }

    fn exchange(
        &mut self,
        ctx: &mut dyn Ctx,
        cs: SpiCs,
        mode: SpiMode,
        mosi: &[u8],
        miso: &mut [u8],
    ) -> Result<(), SpiError> {
        if mosi.len() != miso.len() {
            return Err(SpiError::Overflow);
        }
        let managed_select = self.selected != Some(cs.0);
        if managed_select {
            self.select(ctx, cs, true)?;
        }
        let result = self.exchange_frame(ctx, cs, mode, mosi, miso);
        if managed_select {
            self.select(ctx, cs, false)?;
        }
        result
    }
}

impl SpiBus {
    fn exchange_frame(
        &mut self,
        ctx: &mut dyn Ctx,
        cs: SpiCs,
        mode: SpiMode,
        mosi: &[u8],
        miso: &mut [u8],
    ) -> Result<(), SpiError> {
        let target = self.target(cs)?;
        for (out_byte, in_byte) in mosi.iter().zip(miso.iter_mut()) {
            *in_byte = target.exchange_byte(ctx, mode, *out_byte)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mosaic_contract_model::ContextError;
    use std::sync::{Arc, Mutex};

    /// Recorded select/frame traffic, shared so the test can inspect the
    /// target while the bus owns it behind a trait object.
    #[derive(Default)]
    struct EchoProbe {
        selects: Vec<bool>,
        frames: Vec<Vec<u8>>,
        current: Vec<u8>,
    }

    /// Echo target: MISO is MOSI XOR 0x55, deselect closes the current frame.
    struct EchoTarget {
        probe: Arc<Mutex<EchoProbe>>,
    }

    impl SpiTarget for EchoTarget {
        fn selected(&mut self, _ctx: &mut dyn Ctx, assert: bool) {
            let mut probe = self.probe.lock().unwrap_or_else(|e| e.into_inner());
            probe.selects.push(assert);
            if !assert && !probe.current.is_empty() {
                probe.frames.push(core::mem::take(&mut probe.current));
            }
        }

        fn exchange_byte(
            &mut self,
            _ctx: &mut dyn Ctx,
            _mode: SpiMode,
            mosi: u8,
        ) -> Result<u8, SpiError> {
            self.probe
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .current
                .push(mosi);
            Ok(mosi ^ 0x55)
        }
    }

    struct DummyCtx;
    impl Ctx for DummyCtx {
        fn now(&self) -> mosaic_core_types::VTime {
            mosaic_core_types::VTime::ZERO
        }
        fn schedule(
            &mut self,
            _at: mosaic_core_types::VTime,
            _token: u64,
        ) -> Result<mosaic_core_types::EventId, ContextError> {
            Err(ContextError::ResourceExhausted)
        }
        fn cancel(&mut self, _event: mosaic_core_types::EventId) -> bool {
            false
        }
        fn set_signal(
            &mut self,
            _signal: mosaic_core_types::SignalId,
            _high: bool,
        ) -> Result<(), ContextError> {
            Ok(())
        }
    }

    #[test]
    fn exchange_routes_to_the_target_behind_the_chip_select() {
        let mut bus = SpiBus::new();
        bus.add_target(
            SpiCs(0),
            Box::new(EchoTarget {
                probe: Arc::new(Mutex::new(EchoProbe::default())),
            }),
        );
        let mut miso = [0u8; 3];
        bus.exchange(
            &mut DummyCtx,
            SpiCs(0),
            SpiMode::from_number(0).unwrap(),
            &[0x01, 0x02, 0x03],
            &mut miso,
        )
        .unwrap();
        assert_eq!(miso, [0x54, 0x57, 0x56]);
        assert_eq!(
            bus.exchange(
                &mut DummyCtx,
                SpiCs(2),
                SpiMode::from_number(0).unwrap(),
                &[0],
                &mut [0],
            ),
            Err(SpiError::NoTarget)
        );
    }

    #[test]
    fn explicit_select_keeps_one_frame_across_exchanges() {
        let probe = Arc::new(Mutex::new(EchoProbe::default()));
        let mut bus = SpiBus::new();
        bus.add_target(
            SpiCs(1),
            Box::new(EchoTarget {
                probe: Arc::clone(&probe),
            }),
        );
        let ctx = &mut DummyCtx;
        bus.select(ctx, SpiCs(1), true).unwrap();
        bus.exchange(ctx, SpiCs(1), SpiMode::new(true, true), &[0xAA], &mut [0])
            .unwrap();
        bus.exchange(ctx, SpiCs(1), SpiMode::new(true, true), &[0xBB], &mut [0])
            .unwrap();
        bus.select(ctx, SpiCs(1), false).unwrap();
        let recorded = probe.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(recorded.selects, vec![true, false]);
        assert_eq!(recorded.frames.len(), 1);
        assert_eq!(recorded.frames[0], vec![0xAA, 0xBB]);
    }

    #[test]
    fn mismatched_buffer_lengths_are_rejected() {
        let mut bus = SpiBus::new();
        bus.add_target(
            SpiCs(0),
            Box::new(EchoTarget {
                probe: Arc::new(Mutex::new(EchoProbe::default())),
            }),
        );
        let result = bus.exchange(
            &mut DummyCtx,
            SpiCs(0),
            SpiMode::new(false, false),
            &[1, 2],
            &mut [0],
        );
        assert_eq!(result, Err(SpiError::Overflow));
    }
}
