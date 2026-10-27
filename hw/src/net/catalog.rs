//! Network controllers: catalog entries (ADR 0025).

use std::sync::{Arc, Mutex};

use mosaic_assemble::{InstantiateCtx, MaterializeError};
use mosaic_runtime::Machine;

use crate::catalog::ModelEntry;
use crate::net::lan9118::Lan9118;

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
