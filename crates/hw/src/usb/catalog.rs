//! USB host controllers: catalog entries (ADR 0025).

use mosaic_assemble::{InstantiateCtx, MaterializeError};
use mosaic_runtime::Machine;

use crate::catalog::ModelEntry;
use crate::usb::am335x_usb::Am335XUsb;
use crate::usb::dwc2::Dwc2;
use crate::usb::dwc3::Dwc3;
use crate::usb::ehci::Ehci;
use crate::usb::lpc11xx_usb::Lpc11XxUsb;
use crate::usb::lpc17xx_usb::Lpc17XxUsb;
use crate::usb::lpc43xx_usb::Lpc43XxUsb;
use crate::usb::mtk_xhci::MtkXhci;
use crate::usb::ohci::Ohci;
use crate::usb::pic32_usb::Pic32Usb;
use crate::usb::rk_dwc2::RkDwc2;
use crate::usb::rp2040_usb::Rp2040Usb;
use crate::usb::sunxi_musb::SunxiMusb;
use crate::usb::uhci::Uhci;
use crate::usb::usb_f1::UsbF1;
use crate::usb::usb_f4::UsbF4;
use crate::usb::usb_h7::UsbH7;
use crate::usb::xhci::Xhci;

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

pub const MODELS: &[ModelEntry] = &[ModelEntry {
    id: "ip:intel/uhci",
    summary: "Intel UHCI USB 1.1 host controller (frame list, TD walk, PCI function)",
    outputs: &[],
    spawn: spawn_uhci,
}];

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
