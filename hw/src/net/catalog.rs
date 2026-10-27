//! Network controllers: catalog entries (ADR 0025).

use std::sync::{Arc, Mutex};

use mosaic_assemble::{InstantiateCtx, MaterializeError};
use mosaic_runtime::Machine;

use crate::catalog::ModelEntry;
use crate::net::lan9118::Lan9118;
use crate::net::lan9220::Lan9220;
use crate::net::lan9250::Lan9250;
use crate::net::rtl8139::Rtl8139;
use crate::net::rtl8169::Rtl8169;
use crate::net::e1000::E1000;
use crate::net::e1000e::E1000E;
use crate::net::i82540::I82540;
use crate::net::i82551::I82551;
use crate::net::i82557::I82557;
use crate::net::i82559::I82559;
use crate::net::i82574::I82574;
use crate::net::pcnet32::Pcnet32;
use crate::net::am79c970::Am79C970;
use crate::net::am79c973::Am79C973;
use crate::net::dp83932::Dp83932;
use crate::net::dm9000::Dm9000;
use crate::net::gmac::Gmac;
use crate::net::dwmac::Dwmac;
use crate::net::fec::Fec;
use crate::net::cpsw::Cpsw;
use crate::net::davinci_mdio::DavinciMdio;
use crate::net::lan91c111::Lan91C111;
use crate::net::bcmgenet::Bcmgenet;
use crate::net::mv643xx::Mv643Xx;

/// SMSC LAN9118: `irq` output; the Ethernet link (with its ports) arrives
/// as the host attachment `host_net()`.
fn spawn_lan9118(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let mac = match ctx.parameter_str("mac") {
        Some(text) => parse_mac(text),
        None => [0x52, 0x54, 0x00, 0x12, 0x34, 0x56],
    };
    let shared = Lan9118::new(irq, mac).shared();
    if let Some(link) = ctx.host_net() {
        let port = Lan9118::port(&shared);
        let index = link
            .lock()
            .map(|mut link| link.add_port(Box::new(port)))
            .unwrap_or(0);
        if let Ok(mut nic) = shared.lock() {
            nic.attach_link(link, index);
        }
    }
    machine.register_device(ctx.node, Box::new(SharedLan9118 { inner: shared }))?;
    Ok(())
}

/// Parse a `52:54:00:12:34:56` MAC string.
fn parse_mac(text: &str) -> [u8; 6] {
    let mut mac = [0u8; 6];
    for (index, byte) in text.split(':').enumerate() {
        if index >= 6 {
            break;
        }
        if let Ok(value) = u8::from_str_radix(byte, 16) {
            mac[index] = value;
        }
    }
    mac
}

/// The registered device: the NIC behind the handle the port half shares.
struct SharedLan9118 {
    inner: Arc<Mutex<Lan9118>>,
}

impl mosaic_contract::mmio::MmioTarget for SharedLan9118 {
    fn read(
        &mut self,
        ctx: &mut dyn mosaic_contract::model::Ctx,
        offset: u64,
        size: mosaic_exec::AccessSize,
        attr: &mosaic_exec::Attr,
    ) -> Result<u64, mosaic_exec::BusFault> {
        match self.inner.lock() {
            Ok(mut nic) => {
                mosaic_contract::mmio::MmioTarget::read(&mut *nic, ctx, offset, size, attr)
            }
            Err(_) => Err(mosaic_exec::BusFault::Device),
        }
    }

    fn write(
        &mut self,
        ctx: &mut dyn mosaic_contract::model::Ctx,
        offset: u64,
        size: mosaic_exec::AccessSize,
        value: u64,
        attr: &mosaic_exec::Attr,
    ) -> Result<(), mosaic_exec::BusFault> {
        match self.inner.lock() {
            Ok(mut nic) => {
                mosaic_contract::mmio::MmioTarget::write(&mut *nic, ctx, offset, size, value, attr)
            }
            Err(_) => Err(mosaic_exec::BusFault::Device),
        }
    }
}

impl mosaic_contract::model::EventTarget for SharedLan9118 {
    fn on_timer(
        &mut self,
        _ctx: &mut dyn mosaic_contract::model::Ctx,
        _token: u64,
    ) -> Result<(), mosaic_contract::model::ContextError> {
        Ok(())
    }
}

pub const MODELS: &[ModelEntry] = &[ModelEntry {
    id: "ip:smsc/lan9118",
    summary: "SMSC LAN9118 Ethernet controller (TX/RX FIFOs, MAC, internal PHY)",
    outputs: &[],
    spawn: spawn_lan9118,
}];

/// Default spawn for lan9220: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_lan9220(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Lan9220::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for lan9250: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_lan9250(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Lan9250::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for rtl8139: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_rtl8139(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Rtl8139::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for rtl8169: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_rtl8169(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Rtl8169::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for e1000: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_e1000(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = E1000::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for e1000e: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_e1000e(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = E1000E::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for i82540: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_i82540(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = I82540::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for i82551: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_i82551(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = I82551::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for i82557: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_i82557(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = I82557::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for i82559: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_i82559(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = I82559::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for i82574: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_i82574(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = I82574::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for pcnet32: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_pcnet32(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Pcnet32::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for am79c970: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_am79c970(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Am79C970::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for am79c973: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_am79c973(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Am79C973::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for dp83932: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_dp83932(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Dp83932::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for dm9000: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_dm9000(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Dm9000::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for gmac: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_gmac(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Gmac::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for dwmac: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_dwmac(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Dwmac::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for fec: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_fec(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Fec::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for cpsw: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_cpsw(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Cpsw::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for davinci-mdio: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_davinci_mdio(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = DavinciMdio::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for lan91c111: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_lan91c111(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Lan91C111::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for bcmgenet: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_bcmgenet(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Bcmgenet::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for mv643xx: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_mv643xx(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Mv643Xx::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
