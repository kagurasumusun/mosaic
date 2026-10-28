//! Network controller IP blocks (QEMU `hw/net/`): on-SoC MACs, PCI and
//! ISA NICs. PHYs, wireless modules and modem cards live in
//! `crate::part::net`.

use crate::catalog::ModelEntry;
use crate::ip::net::am79c970::Am79C970;
use crate::ip::net::am79c973::Am79C973;
use crate::ip::net::bcmgenet::Bcmgenet;
use crate::ip::net::cpsw::Cpsw;
use crate::ip::net::davinci_mdio::DavinciMdio;
use crate::ip::net::dm9000::Dm9000;
use crate::ip::net::dp83932::Dp83932;
use crate::ip::net::dwmac::Dwmac;
use crate::ip::net::e1000::E1000;
use crate::ip::net::e1000e::E1000E;
use crate::ip::net::fec::Fec;
use crate::ip::net::gmac::Gmac;
use crate::ip::net::i82540::I82540;
use crate::ip::net::i82551::I82551;
use crate::ip::net::i82557::I82557;
use crate::ip::net::i82559::I82559;
use crate::ip::net::i82574::I82574;
use crate::ip::net::lan9118::Lan9118;
use crate::ip::net::lan91c111::Lan91C111;
use crate::ip::net::lan9220::Lan9220;
use crate::ip::net::lan9250::Lan9250;
use crate::ip::net::mv643xx::Mv643Xx;
use crate::ip::net::pcnet32::Pcnet32;
use crate::ip::net::rtl8139::Rtl8139;
use crate::ip::net::rtl8169::Rtl8169;
use mosaic_assemble::{InstantiateCtx, MaterializeError};
use mosaic_runtime::Machine;
use std::sync::{Arc, Mutex};

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
pub const MODELS: &[ModelEntry] = &[
    ModelEntry {
        id: "ip:smsc/lan9118",
        summary: "SMSC LAN9118 Ethernet controller (TX/RX FIFOs, MAC, internal PHY)",
        outputs: &[],
        spawn: spawn_lan9118,
    },
    ModelEntry {
        id: "ip:smsc/lan9220",
        summary: "SMSC LAN9220 10/100 Ethernet controller",
        outputs: &[],
        spawn: spawn_lan9220,
    },
    ModelEntry {
        id: "ip:smsc/lan9250",
        summary: "SMSC LAN9250 10/100 Ethernet controller",
        outputs: &[],
        spawn: spawn_lan9250,
    },
    ModelEntry {
        id: "ip:realtek/rtl8139",
        summary: "Realtek RTL8139 10/100 Ethernet controller (PCI)",
        outputs: &[],
        spawn: spawn_rtl8139,
    },
    ModelEntry {
        id: "ip:realtek/rtl8169",
        summary: "Realtek RTL8169 10/100/1000 Ethernet controller (PCI)",
        outputs: &[],
        spawn: spawn_rtl8169,
    },
    ModelEntry {
        id: "ip:intel/e1000",
        summary: "Intel E1000 1G Ethernet controller (PCI)",
        outputs: &[],
        spawn: spawn_e1000,
    },
    ModelEntry {
        id: "ip:intel/e1000e",
        summary: "Intel E1000e 1G Ethernet controller (PCIe)",
        outputs: &[],
        spawn: spawn_e1000e,
    },
    ModelEntry {
        id: "ip:intel/i82540",
        summary: "Intel I82540EM 1G Ethernet controller (PCI)",
        outputs: &[],
        spawn: spawn_i82540,
    },
    ModelEntry {
        id: "ip:intel/i82551",
        summary: "Intel I82551QM 10/100 Ethernet controller (PCI)",
        outputs: &[],
        spawn: spawn_i82551,
    },
    ModelEntry {
        id: "ip:intel/i82557",
        summary: "Intel I82557 10/100 Ethernet controller (PCI)",
        outputs: &[],
        spawn: spawn_i82557,
    },
    ModelEntry {
        id: "ip:intel/i82559",
        summary: "Intel I82559ER 10/100 Ethernet controller (PCI)",
        outputs: &[],
        spawn: spawn_i82559,
    },
    ModelEntry {
        id: "ip:intel/i82574",
        summary: "Intel I82574 1G Ethernet controller (PCIe)",
        outputs: &[],
        spawn: spawn_i82574,
    },
    ModelEntry {
        id: "ip:amd/pcnet32",
        summary: "AMD PCnet32 10/100 Ethernet controller (PCI)",
        outputs: &[],
        spawn: spawn_pcnet32,
    },
    ModelEntry {
        id: "ip:amd/am79c970",
        summary: "AMD Am79C970 10/100 Ethernet controller (PCI)",
        outputs: &[],
        spawn: spawn_am79c970,
    },
    ModelEntry {
        id: "ip:amd/am79c973",
        summary: "AMD Am79C973 10/100 Ethernet controller (PCI)",
        outputs: &[],
        spawn: spawn_am79c973,
    },
    ModelEntry {
        id: "ip:national/dp83932",
        summary: "National DP83932 (SONIC) Ethernet controller",
        outputs: &[],
        spawn: spawn_dp83932,
    },
    ModelEntry {
        id: "ip:davicom/dm9000",
        summary: "DAVICOM DM9000 10/100 Ethernet controller (mem-mapped)",
        outputs: &[],
        spawn: spawn_dm9000,
    },
    ModelEntry {
        id: "ip:cadence/gmac",
        summary: "Cadence Gigabit MAC (used by Atmel SAMA5)",
        outputs: &[],
        spawn: spawn_gmac,
    },
    ModelEntry {
        id: "ip:synopsys/dwmac",
        summary: "Synopsys DesignWare MAC (used by STM32/Allwinner/Rockchip)",
        outputs: &[],
        spawn: spawn_dwmac,
    },
    ModelEntry {
        id: "ip:freescale/fec",
        summary: "Freescale FEC Ethernet controller",
        outputs: &[],
        spawn: spawn_fec,
    },
    ModelEntry {
        id: "ip:ti/cpsw",
        summary: "TI Common Platform Ethernet Switch (CPSW)",
        outputs: &[],
        spawn: spawn_cpsw,
    },
    ModelEntry {
        id: "ip:smsc/lan91c111",
        summary: "SMSC LAN91C111 10/100 Ethernet controller",
        outputs: &[],
        spawn: spawn_lan91c111,
    },
    ModelEntry {
        id: "ip:broadcom/bcmgenet",
        summary: "Broadcom GENET Ethernet controller",
        outputs: &[],
        spawn: spawn_bcmgenet,
    },
    ModelEntry {
        id: "ip:marvell/mv643xx",
        summary: "Marvell MV643xx Ethernet controller",
        outputs: &[],
        spawn: spawn_mv643xx,
    },
    ModelEntry {
        id: "ip:ti/davinci-mdio",
        summary: "TI DaVinci MDIO bus controller",
        outputs: &[],
        spawn: spawn_davinci_mdio,
    },
];
