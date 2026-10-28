//! SPI bus controller IP blocks (QEMU `hw/ssi/`): the register models
//! that master an SPI bus (the directory is named after the contract
//! vocabulary `spi`, not QEMU's legacy `ssi`). The bus fabric lives in
//! `mosaic-bus::spi`.

use crate::catalog::ModelEntry;
use crate::ip::spi::am335x_spi::Am335XSpi;
use crate::ip::spi::esp32_spi::Esp32Spi;
use crate::ip::spi::lpc11xx_spi::Lpc11XxSpi;
use crate::ip::spi::lpc17xx_spi::Lpc17XxSpi;
use crate::ip::spi::lpc43xx_spi::Lpc43XxSpi;
use crate::ip::spi::mtk_spi::MtkSpi;
use crate::ip::spi::pic32_spi::Pic32Spi;
use crate::ip::spi::pl022::{Pl022, Pl022Params};
use crate::ip::spi::rk_spi::RkSpi;
use crate::ip::spi::rp2040_spi::Rp2040Spi;
use crate::ip::spi::spi_f1::SpiF1;
use crate::ip::spi::spi_f4::SpiF4;
use crate::ip::spi::spi_h7::SpiH7;
use crate::ip::spi::spi_l4::SpiL4;
use crate::ip::spi::sunxi_spi::SunxiSpi;
use mosaic_assemble::{InstantiateCtx, MaterializeError};
use mosaic_bus::spi::core::SpiBus;
use mosaic_contract::model::Ctx;
use mosaic_contract::spi::{SpiCs, SpiError, SpiInitiator, SpiMode};
use mosaic_runtime::Machine;
use std::sync::{Arc, Mutex};

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
pub const MODELS: &[ModelEntry] = &[
    ModelEntry {
        id: "ip:arm/pl022",
        summary: "ARM PrimeCell SSP (PL022) SPI controller register model",
        outputs: &[],
        spawn: spawn_pl022,
    },
    ModelEntry {
        id: "ip:st/spi-f1",
        summary: "STM32F1 SPI",
        outputs: &[],
        spawn: spawn_spi_f1,
    },
    ModelEntry {
        id: "ip:st/spi-f4",
        summary: "STM32F4 SPI",
        outputs: &[],
        spawn: spawn_spi_f4,
    },
    ModelEntry {
        id: "ip:st/spi-h7",
        summary: "STM32H7 SPI",
        outputs: &[],
        spawn: spawn_spi_h7,
    },
    ModelEntry {
        id: "ip:st/spi-l4",
        summary: "STM32L4 SPI",
        outputs: &[],
        spawn: spawn_spi_l4,
    },
    ModelEntry {
        id: "ip:nxp/lpc11xx-spi",
        summary: "NXP LPC11xx SPI",
        outputs: &[],
        spawn: spawn_lpc11xx_spi,
    },
    ModelEntry {
        id: "ip:nxp/lpc17xx-spi",
        summary: "NXP LPC17xx SSP/SPI",
        outputs: &[],
        spawn: spawn_lpc17xx_spi,
    },
    ModelEntry {
        id: "ip:nxp/lpc43xx-spi",
        summary: "NXP LPC43xx SPI",
        outputs: &[],
        spawn: spawn_lpc43xx_spi,
    },
    ModelEntry {
        id: "ip:raspberrypi/rp2040-spi",
        summary: "Raspberry Pi RP2040 SPI",
        outputs: &[],
        spawn: spawn_rp2040_spi,
    },
    ModelEntry {
        id: "ip:allwinner/sunxi-spi",
        summary: "Allwinner Sunxi SPI",
        outputs: &[],
        spawn: spawn_sunxi_spi,
    },
    ModelEntry {
        id: "ip:rockchip/rk-spi",
        summary: "Rockchip SPI",
        outputs: &[],
        spawn: spawn_rk_spi,
    },
    ModelEntry {
        id: "ip:mediatek/mtk-spi",
        summary: "MediaTek SPI",
        outputs: &[],
        spawn: spawn_mtk_spi,
    },
    ModelEntry {
        id: "ip:ti/am335x-spi",
        summary: "TI AM335x MCSPI",
        outputs: &[],
        spawn: spawn_am335x_spi,
    },
    ModelEntry {
        id: "ip:microchip/pic32-spi",
        summary: "Microchip PIC32 SPI",
        outputs: &[],
        spawn: spawn_pic32_spi,
    },
    ModelEntry {
        id: "ip:espressif/esp32-spi",
        summary: "Espressif ESP32 SPI",
        outputs: &[],
        spawn: spawn_esp32_spi,
    },
];
