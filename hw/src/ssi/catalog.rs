//! SPI controllers: catalog entries (ADR 0025). SPI bus membership — which
//! target hangs off which chip select — is board-unique wiring through the
//! `mosaic-bus::spi` fabric, exactly as QEMU boards wire `ssi_bus` in C.

use std::sync::{Arc, Mutex};

use mosaic_assemble::{InstantiateCtx, MaterializeError};
use mosaic_bus::spi::core::SpiBus;
use mosaic_contract::model::Ctx;
use mosaic_contract::spi::{SpiCs, SpiError, SpiInitiator, SpiMode};
use mosaic_runtime::Machine;

use crate::catalog::ModelEntry;
use crate::ssi::pl022::{Pl022, Pl022Params};

/// The controller-side port over the shared SPI fabric.
pub struct SharedSpi {
    bus: Arc<Mutex<SpiBus>>,
}

impl SharedSpi {
    pub fn new(bus: Arc<Mutex<SpiBus>>) -> Self {
        Self { bus }
    }
}

impl SpiInitiator for SharedSpi {
    fn select(&mut self, ctx: &mut dyn Ctx, cs: SpiCs, assert: bool) -> Result<(), SpiError> {
        match self.bus.lock() {
            Ok(mut bus) => bus.select(ctx, cs, assert),
            Err(_) => Err(SpiError::NoTarget),
        }
    }

    fn exchange(
        &mut self,
        ctx: &mut dyn Ctx,
        cs: SpiCs,
        mode: SpiMode,
        mosi: &[u8],
        miso: &mut [u8],
    ) -> Result<(), SpiError> {
        match self.bus.lock() {
            Ok(mut bus) => bus.exchange(ctx, cs, mode, mosi, miso),
            Err(_) => Err(SpiError::NoTarget),
        }
    }
}

/// ARM PL022 SSP: `combined` IRQ output; the SPI fabric arrives as the host
/// attachment `host_spi()`.
fn spawn_pl022(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("combined")?;
    let initiator = match ctx.host_spi() {
        Some(bus) => Box::new(SharedSpi::new(bus)) as Box<dyn SpiInitiator>,
        None => {
            return Err(MaterializeError::Bind {
                node: ctx.graph_id.to_owned(),
                detail: "pl022 needs an SPI fabric host attachment (host_spi)".into(),
            })
        }
    };
    let controller = Pl022::new(Pl022Params::new(irq), Some(initiator));
    machine.register_device(ctx.node, Box::new(controller))?;
    Ok(())
}

pub const MODELS: &[ModelEntry] = &[ModelEntry {
    id: "ip:arm/pl022",
    summary: "ARM PrimeCell SSP (PL022) SPI controller register model",
    outputs: &[],
    spawn: spawn_pl022,
}];
