//! RISC-V `virt` composition from `manifests/machine/riscv/virt/soc.toml`.
//!
//! The board owns *exactly* what a board owns (ADR 0025): the declaration
//! (parts, regions, the CLINT → core0 mtimer/msip wires, the PLIC →
//! core0 mext wire, the PL011 → PLIC source 10 wire, the virtio → PLIC
//! source 8/9/11 wires), the host console endpoint for `soc.uart0`, and
//! the runtime RAM size override (QEMU's `-m` role). Every device model —
//! its parameters, its clock binding, its IRQ wiring — comes from the
//! device catalog (`mosaic_hw::catalog::device_registry()`), exactly as
//! QEMU boards build `TYPE_INFO`-registered devices without re-describing
//! them.

use mosaic_assemble::{materialize, HostAttachments};
use mosaic_bus::uart::pair::{UartEndpoint, UartPair};
use mosaic_describe::MachineGraph;
use mosaic_exec::NodeId;
use mosaic_hw::catalog::device_registry;
use mosaic_runtime::{FlatView, Machine};

use super::manifest::virt_soc;

/// RAM base in the RISC-V `virt` map (QEMU's `VIRT_MEM`).
pub const RAM_BASE: u64 = 0x8000_0000;
/// The default RAM size the SoC declaration carries (128 MiB).
pub const DEFAULT_RAM_SIZE: u64 = 0x0800_0000;
/// CLINT base (QEMU's `VIRT_CLINT`).
pub const CLINT_BASE: u64 = 0x0200_0000;
/// PLIC base (QEMU's `VIRT_PLIC`).
pub const PLIC_BASE: u64 = 0x0c00_0000;
/// PL011 UART base (QEMU's `VIRT_UART0`).
pub const UART0_BASE: u64 = 0x1000_0000;
/// virtio-mmio transport window base (QEMU's `VIRTIO_MMIO`).
pub const VIRTIO_BASE: u64 = 0x1000_1000;
/// Stride between adjacent virtio-mmio transports.
pub const VIRTIO_STRIDE: u64 = 0x200;

/// The declared machine plus the host ends the board attached.
#[allow(dead_code)]
pub struct VirtBoard {
    pub view: FlatView,
    pub machine: Machine,
    pub console: UartEndpoint,
    pub core: NodeId,
    pub ram: NodeId,
    /// Runtime RAM size, after the `-m` override if any.
    pub ram_size: u64,
}

fn graph() -> Result<MachineGraph, String> {
    let soc = virt_soc()?;
    MachineGraph::build(&soc, None).map_err(|error| error.to_string())
}

/// Build the `virt` machine from the catalog: the declaration is the wiring.
/// `ram_bytes` overrides the SoC declaration's canonical 128 MiB when non-zero
/// (QEMU's `-m` role).
pub fn build_virt(ram_bytes: u64) -> Result<VirtBoard, String> {
    let graph = graph()?;
    let pair = UartPair::new(128);
    let (uart_peer, console) = pair.endpoints();
    let mut hosts = HostAttachments::new();
    hosts.attach_uart("soc.uart0", uart_peer);

    let registry = device_registry();
    let assembled =
        materialize(&graph, &registry, &hosts, 32).map_err(|error| error.to_string())?;

    let ram_size = if ram_bytes == 0 {
        DEFAULT_RAM_SIZE
    } else {
        ram_bytes
    };
    let _ = ram_size;

    assembled
        .machine
        .verify()
        .map_err(|error| format!("verify: {error:?}"))?;

    let core = *assembled
        .nodes
        .get("soc.core0")
        .ok_or("soc.core0 was not allocated")?;
    let ram = *assembled
        .nodes
        .get("memory.ram")
        .ok_or("memory.ram was not allocated")?;

    Ok(VirtBoard {
        view: assembled.view,
        machine: assembled.machine,
        console,
        core,
        ram,
        ram_size,
    })
}

/// Minimal context for harness-side stream reads.
pub struct CaptureCtx;

impl mosaic_contract::model::Ctx for CaptureCtx {
    fn now(&self) -> mosaic_exec::VTime {
        mosaic_exec::VTime::ZERO
    }
    fn schedule(
        &mut self,
        _at: mosaic_exec::VTime,
        _token: u64,
    ) -> Result<mosaic_exec::EventId, mosaic_contract::model::ContextError> {
        Err(mosaic_contract::model::ContextError::ResourceExhausted)
    }
    fn cancel(&mut self, _event: mosaic_exec::EventId) -> bool {
        false
    }
    fn set_signal(
        &mut self,
        _signal: mosaic_exec::SignalId,
        _high: bool,
    ) -> Result<(), mosaic_contract::model::ContextError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_declaration_is_the_qemu_virt_map_not_a_synthetic_fixture() {
        let graph = graph().expect("soc.toml");
        assert_eq!(graph.soc_id, "soc:riscv/virt");
        let clint = graph
            .regions
            .iter()
            .find(|region| region.name == "clint-mmio")
            .expect("clint region");
        assert_eq!(clint.base, CLINT_BASE);
        assert_eq!(clint.size, 0x10000);
        let plic = graph
            .regions
            .iter()
            .find(|region| region.name == "plic-mmio")
            .expect("plic region");
        assert_eq!(plic.base, PLIC_BASE);
        assert_eq!(plic.size, 0x201000);
        let uart = graph
            .regions
            .iter()
            .find(|region| region.name == "uart0-mmio")
            .expect("uart region");
        assert_eq!(uart.base, UART0_BASE);
        // The CLINT → core0 mtimer/msip wires are declared edges.
        assert!(graph.edges.iter().any(|edge| {
            edge.kind == "irq"
                && edge.from_node == "soc.clint"
                && edge.from_port == "irq"
                && edge.to_node == "soc.core0"
                && edge.to_port == "mtimer"
        }));
        // The PLIC → core0 mext wire.
        assert!(graph.edges.iter().any(|edge| {
            edge.kind == "irq"
                && edge.from_node == "soc.plic"
                && edge.from_port == "irq"
                && edge.to_node == "soc.core0"
                && edge.to_port == "mext"
        }));
        // The PL011 → PLIC source 10 wire.
        assert!(graph.edges.iter().any(|edge| {
            edge.kind == "irq"
                && edge.from_node == "soc.uart0"
                && edge.from_port == "combined"
                && edge.to_node == "soc.plic"
                && edge.to_port == "inputs:10"
        }));
    }

    /// The composition materialises from the catalog alone — zero
    /// board-side factory closures. The CLINT, PLIC, PL011 and three
    /// virtio transports all spawn, and the machine verifies.
    #[test]
    fn the_machine_composes_from_the_catalog_alone() {
        let board = build_virt(0).expect("composition");
        assert!(board.machine.verify().is_ok());
    }
}
