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
use crate::ssi::am335x_spi::Am335XSpi;
use crate::ssi::efm32_usart_spi::Efm32UsartSpi;
use crate::ssi::esp32_spi::Esp32Spi;
use crate::ssi::lpc11xx_spi::Lpc11XxSpi;
use crate::ssi::lpc17xx_spi::Lpc17XxSpi;
use crate::ssi::lpc43xx_spi::Lpc43XxSpi;
use crate::ssi::mtk_spi::MtkSpi;
use crate::ssi::pic32_spi::Pic32Spi;
use crate::ssi::pl022::{Pl022, Pl022Params};
use crate::ssi::rk_spi::RkSpi;
use crate::ssi::rp2040_spi::Rp2040Spi;
use crate::ssi::spi_f1::SpiF1;
use crate::ssi::spi_f4::SpiF4;
use crate::ssi::spi_h7::SpiH7;
use crate::ssi::spi_l4::SpiL4;
use crate::ssi::sunxi_spi::SunxiSpi;

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

/// Default spawn for spi-f1: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_spi_f1(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = SpiF1::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for spi-f4: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_spi_f4(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = SpiF4::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for spi-h7: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_spi_h7(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = SpiH7::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for spi-l4: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_spi_l4(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = SpiL4::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for lpc11xx-spi: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_lpc11xx_spi(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Lpc11XxSpi::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for lpc17xx-spi: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_lpc17xx_spi(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Lpc17XxSpi::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for lpc43xx-spi: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_lpc43xx_spi(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Lpc43XxSpi::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for rp2040-spi: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_rp2040_spi(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Rp2040Spi::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for sunxi-spi: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_sunxi_spi(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = SunxiSpi::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for rk-spi: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_rk_spi(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = RkSpi::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for mtk-spi: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_mtk_spi(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = MtkSpi::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for am335x-spi: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_am335x_spi(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Am335XSpi::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for pic32-spi: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_pic32_spi(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Pic32Spi::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for esp32-spi: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_esp32_spi(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Esp32Spi::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for efm32-usart-spi: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_efm32_usart_spi(
    ctx: &InstantiateCtx,
    machine: &mut Machine,
) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Efm32UsartSpi::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
