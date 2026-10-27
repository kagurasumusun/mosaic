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
use crate::i2c::lpc11xx_i2c::Lpc11XxI2C;
use crate::i2c::lpc17xx_i2c::Lpc17XxI2C;
use crate::i2c::lpc43xx_i2c::Lpc43XxI2C;
use crate::i2c::i2c_f1::I2CF1;
use crate::i2c::i2c_f4::I2CF4;
use crate::i2c::i2c_f7::I2CF7;
use crate::i2c::i2c_h7::I2CH7;
use crate::i2c::i2c_l4::I2CL4;
use crate::i2c::rp2040_i2c::Rp2040I2C;
use crate::i2c::sunxi_i2c::SunxiI2C;
use crate::i2c::rk_i2c::RkI2C;
use crate::i2c::mtk_i2c::MtkI2C;
use crate::i2c::omap_i2c::OmapI2C;
use crate::i2c::am335x_i2c::Am335XI2C;
use crate::i2c::pic32_i2c::Pic32I2C;
use crate::i2c::esp32_i2c::Esp32I2C;
use crate::i2c::efm32_i2c::Efm32I2C;

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

/// Default spawn for lpc11xx-i2c: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_lpc11xx_i2c(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Lpc11XxI2C::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for lpc17xx-i2c: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_lpc17xx_i2c(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Lpc17XxI2C::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for lpc43xx-i2c: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_lpc43xx_i2c(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Lpc43XxI2C::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for i2c-f1: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_i2c_f1(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = I2CF1::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for i2c-f4: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_i2c_f4(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = I2CF4::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for i2c-f7: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_i2c_f7(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = I2CF7::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for i2c-h7: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_i2c_h7(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = I2CH7::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for i2c-l4: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_i2c_l4(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = I2CL4::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for rp2040-i2c: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_rp2040_i2c(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Rp2040I2C::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for sunxi-i2c: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_sunxi_i2c(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = SunxiI2C::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for rk-i2c: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_rk_i2c(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = RkI2C::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for mtk-i2c: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_mtk_i2c(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = MtkI2C::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for omap-i2c: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_omap_i2c(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = OmapI2C::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for am335x-i2c: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_am335x_i2c(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Am335XI2C::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for pic32-i2c: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_pic32_i2c(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Pic32I2C::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for esp32-i2c: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_esp32_i2c(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Esp32I2C::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for efm32-i2c: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_efm32_i2c(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Efm32I2C::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
