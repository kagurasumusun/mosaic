//! USB host controller IP blocks (QEMU `hw/usb/`); USB devices
//! (keyboards, mice, storage) live in `crate::part::*`.

use crate::catalog::ModelEntry;
use crate::ip::usb::am335x_usb::Am335XUsb;
use crate::ip::usb::dwc2::Dwc2;
use crate::ip::usb::dwc3::Dwc3;
use crate::ip::usb::ehci::Ehci;
use crate::ip::usb::lpc11xx_usb::Lpc11XxUsb;
use crate::ip::usb::lpc17xx_usb::Lpc17XxUsb;
use crate::ip::usb::lpc43xx_usb::Lpc43XxUsb;
use crate::ip::usb::mtk_xhci::MtkXhci;
use crate::ip::usb::ohci::Ohci;
use crate::ip::usb::pic32_usb::Pic32Usb;
use crate::ip::usb::rk_dwc2::RkDwc2;
use crate::ip::usb::rp2040_usb::Rp2040Usb;
use crate::ip::usb::sunxi_musb::SunxiMusb;
use crate::ip::usb::uhci::Uhci;
use crate::ip::usb::usb_f1::UsbF1;
use crate::ip::usb::usb_f4::UsbF4;
use crate::ip::usb::usb_h7::UsbH7;
use crate::ip::usb::xhci::Xhci;
use mosaic_assemble::{InstantiateCtx, MaterializeError};
use mosaic_runtime::Machine;

/// Intel UHCI: `irq` output; the USB fabric (with its devices) arrives as
/// the host attachment `host_usb()`.
fn spawn_uhci(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let mut controller = Uhci::new(irq);
    if let Some(bus) = ctx.host_usb() {
        controller.attach_bus(bus);
    }
    machine.register_device(ctx.node, Box::new(controller))?;
    Ok(())
}
/// Default spawn for ohci: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_ohci(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Ohci::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for ehci: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_ehci(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Ehci::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for xhci: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_xhci(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Xhci::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for dwc2: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_dwc2(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Dwc2::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for dwc3: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_dwc3(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Dwc3::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for usb-f1: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_usb_f1(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = UsbF1::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for usb-f4: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_usb_f4(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = UsbF4::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for usb-h7: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_usb_h7(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = UsbH7::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for lpc11xx-usb: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_lpc11xx_usb(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Lpc11XxUsb::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for lpc17xx-usb: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_lpc17xx_usb(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Lpc17XxUsb::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for lpc43xx-usb: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_lpc43xx_usb(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Lpc43XxUsb::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for rp2040-usb: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_rp2040_usb(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Rp2040Usb::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for sunxi-musb: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_sunxi_musb(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = SunxiMusb::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for rk-dwc2: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_rk_dwc2(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = RkDwc2::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for mtk-xhci: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_mtk_xhci(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = MtkXhci::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for am335x-usb: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_am335x_usb(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Am335XUsb::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for pic32-usb: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_pic32_usb(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Pic32Usb::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
pub const MODELS: &[ModelEntry] = &[
    ModelEntry {
        id: "ip:intel/uhci",
        summary: "Intel UHCI USB 1.1 host controller (frame list, TD walk, PCI function)",
        outputs: &[],
        spawn: spawn_uhci,
    },
    ModelEntry {
        id: "ip:intel/ohci",
        summary: "Intel OHCI USB 1.1 host controller (PCI)",
        outputs: &[],
        spawn: spawn_ohci,
    },
    ModelEntry {
        id: "ip:intel/ehci",
        summary: "Intel EHCI USB 2.0 host controller (PCI)",
        outputs: &[],
        spawn: spawn_ehci,
    },
    ModelEntry {
        id: "ip:intel/xhci",
        summary: "Intel xHCI USB 3.0 host controller (PCI)",
        outputs: &[],
        spawn: spawn_xhci,
    },
    ModelEntry {
        id: "ip:synopsys/dwc2",
        summary: "Synopsys DesignWare USB 2.0 OTG controller",
        outputs: &[],
        spawn: spawn_dwc2,
    },
    ModelEntry {
        id: "ip:synopsys/dwc3",
        summary: "Synopsys DesignWare USB 3.0 controller",
        outputs: &[],
        spawn: spawn_dwc3,
    },
    ModelEntry {
        id: "ip:st/usb-f1",
        summary: "STM32F1 USB 2.0 FS device controller",
        outputs: &[],
        spawn: spawn_usb_f1,
    },
    ModelEntry {
        id: "ip:st/usb-f4",
        summary: "STM32F4 USB 2.0 HS OTG controller",
        outputs: &[],
        spawn: spawn_usb_f4,
    },
    ModelEntry {
        id: "ip:st/usb-h7",
        summary: "STM32H7 USB 2.0 HS OTG controller",
        outputs: &[],
        spawn: spawn_usb_h7,
    },
    ModelEntry {
        id: "ip:nxp/lpc11xx-usb",
        summary: "NXP LPC11xx USB 2.0 device controller",
        outputs: &[],
        spawn: spawn_lpc11xx_usb,
    },
    ModelEntry {
        id: "ip:nxp/lpc17xx-usb",
        summary: "NXP LPC17xx USB 2.0 OTG controller",
        outputs: &[],
        spawn: spawn_lpc17xx_usb,
    },
    ModelEntry {
        id: "ip:nxp/lpc43xx-usb",
        summary: "NXP LPC43xx USB 2.0 controller",
        outputs: &[],
        spawn: spawn_lpc43xx_usb,
    },
    ModelEntry {
        id: "ip:raspberrypi/rp2040-usb",
        summary: "Raspberry Pi RP2040 USB 2.0 device controller",
        outputs: &[],
        spawn: spawn_rp2040_usb,
    },
    ModelEntry {
        id: "ip:allwinner/sunxi-musb",
        summary: "Allwinner Sunxi Mentor Graphics USB OTG",
        outputs: &[],
        spawn: spawn_sunxi_musb,
    },
    ModelEntry {
        id: "ip:rockchip/rk-dwc2",
        summary: "Rockchip DWC2 USB 2.0 OTG controller",
        outputs: &[],
        spawn: spawn_rk_dwc2,
    },
    ModelEntry {
        id: "ip:mediatek/mtk-xhci",
        summary: "MediaTek xHCI USB 3.0 controller",
        outputs: &[],
        spawn: spawn_mtk_xhci,
    },
    ModelEntry {
        id: "ip:ti/am335x-usb",
        summary: "TI AM335x USB 2.0 OTG controller",
        outputs: &[],
        spawn: spawn_am335x_usb,
    },
    ModelEntry {
        id: "ip:microchip/pic32-usb",
        summary: "Microchip PIC32 USB 2.0 controller",
        outputs: &[],
        spawn: spawn_pic32_usb,
    },
];
