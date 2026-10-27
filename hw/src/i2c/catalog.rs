//! I2C controllers: catalog entries (ADR 0025). Bus membership — which
//! target sits at which address — is board-unique wiring through the
//! `mosaic-bus::i2c` fabric; the controller picks up the whole composed
//! fabric through its host attachment.

use std::sync::{Arc, Mutex};

use mosaic_assemble::{InstantiateCtx, MaterializeError};
use mosaic_bus::i2c::core::I2cBus;
use mosaic_contract::i2c::{I2cAddr, I2cError, I2cInitiator, I2cSpeed};
use mosaic_contract::model::Ctx;
use mosaic_runtime::Machine;

use crate::catalog::ModelEntry;
use crate::i2c::dw_apb_i2c::{DwApbI2c, DwApbI2cParams};

/// The controller-side port over the shared bus fabric: every controller
/// model that speaks I2C wraps its host attachment in this adapter.
pub struct SharedI2c {
    bus: Arc<Mutex<I2cBus>>,
}

impl SharedI2c {
    pub fn new(bus: Arc<Mutex<I2cBus>>) -> Self {
        Self { bus }
    }
}

impl I2cInitiator for SharedI2c {
    fn set_speed(&mut self, speed: I2cSpeed) {
        if let Ok(mut bus) = self.bus.lock() {
            bus.set_speed(speed);
        }
    }

    fn write(&mut self, ctx: &mut dyn Ctx, addr: I2cAddr, bytes: &[u8]) -> Result<(), I2cError> {
        match self.bus.lock() {
            Ok(mut bus) => bus.write(ctx, addr, bytes),
            Err(_) => Err(I2cError::BusBusy),
        }
    }

    fn read(&mut self, ctx: &mut dyn Ctx, addr: I2cAddr, buf: &mut [u8]) -> Result<(), I2cError> {
        match self.bus.lock() {
            Ok(mut bus) => bus.read(ctx, addr, buf),
            Err(_) => Err(I2cError::BusBusy),
        }
    }

    fn write_read(
        &mut self,
        ctx: &mut dyn Ctx,
        addr: I2cAddr,
        out: &[u8],
        input: &mut [u8],
    ) -> Result<(), I2cError> {
        match self.bus.lock() {
            Ok(mut bus) => bus.write_read(ctx, addr, out, input),
            Err(_) => Err(I2cError::BusBusy),
        }
    }
}

/// DesignWare APB I2C: `combined` IRQ output; the I2C fabric (with its
/// targets) arrives as the host attachment `host_i2c()`.
fn spawn_dw_apb_i2c(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("combined")?;
    let initiator = match ctx.host_i2c() {
        Some(bus) => Box::new(SharedI2c::new(bus)) as Box<dyn I2cInitiator>,
        None => {
            return Err(MaterializeError::Bind {
                node: ctx.graph_id.to_owned(),
                detail: "dw-apb-i2c needs an I2C fabric host attachment (host_i2c)".into(),
            })
        }
    };
    let controller = DwApbI2c::new(DwApbI2cParams::new(irq), initiator);
    machine.register_device(ctx.node, Box::new(controller))?;
    Ok(())
}

pub const MODELS: &[ModelEntry] = &[ModelEntry {
    id: "ip:snps/dw-apb-i2c",
    summary: "DesignWare APB I2C controller (DW_apb_i2c register model)",
    outputs: &[],
    spawn: spawn_dw_apb_i2c,
}];
