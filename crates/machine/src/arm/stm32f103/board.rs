//! STM32F103 composition from `manifests/machine/arm/stm32f103/soc.toml`.
//!
//! The board owns *exactly* what a board owns (ADR 0025): the declaration
//! (parts, regions, the RCC clock-gate wires `rcc.gate-apb2-iopa` →
//! `gpioa.clock`, the USART1 → NVIC 37 and RCC CIR → NVIC 5 wires), the host
//! console endpoint for `soc.usart1`, and the PA5 pin probe this board's
//! diagnostics read. Every device model — its parameters, its clock binding,
//! its IRQ wiring — comes from the device catalog
//! (`mosaic_hw::catalog::device_registry()`), exactly as QEMU boards build
//! `TYPE_INFO`-registered devices without re-describing them. Adding a
//! sibling board (another STM32, a PL011 board, ...) copies *none* of this
//! file's wiring knowledge.

use std::sync::{Arc, Mutex};

use mosaic_assemble::{materialize, HostAttachments};
use mosaic_bus::uart::pair::{UartEndpoint, UartPair};
use mosaic_contract::model::{ContextError, Ctx};
use mosaic_contract::signal::SignalSink;
use mosaic_describe::MachineGraph;
use mosaic_exec::{EventId, NodeId, SignalId, VTime};
use mosaic_hw::catalog::device_registry;
use mosaic_runtime::{FlatView, Machine};

use super::manifest::stm32f103_soc;

pub const SRAM_BASE: u64 = 0x2000_0000;
pub const SRAM_SIZE: u64 = 0x0000_5000;
pub const GPIOA_BASE: u64 = 0x4001_0800;
pub const USART1_BASE: u64 = 0x4001_3800;
pub const RCC_BASE: u64 = 0x4002_1000;
/// PA5, the observed GPIO output. CRL nibble 5 is bits 20..23, not nibble 4.
pub const PROBE_PIN: u32 = 5;

/// The declared machine plus the host ends the board attached.
#[allow(dead_code)]
pub struct Stm32Board {
    pub view: FlatView,
    pub machine: Machine,
    pub console: UartEndpoint,
    pub pin_levels: Arc<Mutex<Vec<bool>>>,
    pub core: NodeId,
    pub flash: NodeId,
}

/// The PA5 probe: a board-unique diagnostic (which pin carries the LED is
/// this board's own knowledge, not the SoC's).
struct PinProbe {
    levels: Arc<Mutex<Vec<bool>>>,
}

impl SignalSink for PinProbe {
    fn set_level(&mut self, high: bool) {
        self.levels.lock().expect("pin probe lock").push(high);
    }
}

fn graph() -> Result<MachineGraph, String> {
    let soc = stm32f103_soc()?;
    MachineGraph::build(&soc, None).map_err(|error| error.to_string())
}

/// Build the F103 machine from the catalog: the declaration is the wiring.
pub fn build_stm32f103() -> Result<Stm32Board, String> {
    let graph = graph()?;
    // The board's one host-side fact: `soc.usart1` is the console. The peer
    // end stays here; the model takes the guest end through its factory's
    // host attachment.
    let pair = UartPair::new(128);
    let (uart_peer, console) = pair.endpoints();
    let mut hosts = HostAttachments::new();
    hosts.attach_uart("soc.usart1", uart_peer);

    // The whole device population comes from the catalog (QEMU's
    // MODULE_INIT aggregate): zero board-side factory closures.
    let registry = device_registry();
    let mut assembled =
        materialize(&graph, &registry, &hosts, 32).map_err(|error| error.to_string())?;

    // The board's own diagnostics: the PA5 probe on this board's LED pin.
    let gpioa = assembled
        .nodes
        .get("soc.gpioa")
        .copied()
        .ok_or("soc.gpioa was not allocated")?;
    let pin_levels = Arc::new(Mutex::new(Vec::new()));
    let probe = NodeId(assembled.next_node);
    assembled
        .machine
        .register_sink(
            probe,
            Box::new(PinProbe {
                levels: Arc::clone(&pin_levels),
            }),
        )
        .map_err(|error| format!("pin sink register: {error:?}"))?;
    assembled
        .machine
        .connect(
            gpioa,
            mosaic_hw::ip::gpio::gpio_f1::pin_signal(PROBE_PIN),
            probe,
        )
        .map_err(|error| format!("pin wiring: {error:?}"))?;
    assembled
        .machine
        .verify()
        .map_err(|error| format!("verify: {error:?}"))?;

    Ok(Stm32Board {
        view: assembled.view,
        machine: assembled.machine,
        console,
        pin_levels,
        core: *assembled
            .nodes
            .get("soc.core0")
            .ok_or("soc.core0 was not allocated")?,
        flash: *assembled
            .nodes
            .get("memory.flash")
            .ok_or("memory.flash was not allocated")?,
    })
}

/// Minimal context for harness-side stream reads.
pub struct CaptureCtx;

impl Ctx for CaptureCtx {
    fn now(&self) -> VTime {
        VTime::ZERO
    }
    fn schedule(&mut self, _at: VTime, _token: u64) -> Result<EventId, ContextError> {
        Err(ContextError::ResourceExhausted)
    }
    fn cancel(&mut self, _event: EventId) -> bool {
        false
    }
    fn set_signal(&mut self, _signal: SignalId, _high: bool) -> Result<(), ContextError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use mosaic_exec::{AccessKind, AccessSize, Addr, Attr, Endian, Security};

    use super::*;

    fn attr(master: NodeId) -> Attr {
        Attr {
            master,
            security: Security::Secure,
            privileged: true,
            kind: AccessKind::Data,
            exclusive: false,
            byte_enable: 0,
            endian: Endian::Little,
        }
    }

    #[test]
    fn the_declaration_is_the_rm0008_map_not_a_synthetic_fixture() {
        let graph = graph().expect("soc.toml");
        assert_eq!(graph.soc_id, "soc:st/stm32f103");
        let usart = graph
            .regions
            .iter()
            .find(|region| region.name == "usart1-mmio")
            .expect("usart region");
        assert_eq!(usart.base, USART1_BASE);
        assert_eq!(usart.size, 0x1C);
        let flash = graph
            .regions
            .iter()
            .find(|region| region.name == "flash")
            .expect("flash region");
        assert_eq!(flash.base, 0x0800_0000);
        assert_eq!(flash.size, 0x0002_0000);
        let sram = graph
            .regions
            .iter()
            .find(|region| region.name == "sram")
            .expect("sram region");
        assert_eq!(sram.base, SRAM_BASE);
        assert_eq!(sram.size, SRAM_SIZE);
        // The clock-gate wires are declared edges, not board code.
        assert!(graph.edges.iter().any(|edge| {
            edge.kind == "clock"
                && edge.from_node == "soc.rcc"
                && edge.from_port == "gate-apb2-iopa"
                && edge.to_node == "soc.gpioa"
                && edge.to_port == "clock"
        }));
        // The IRQ wires: USART1 combined into NVIC 37, RCC CIR into NVIC 5.
        assert!(graph.edges.iter().any(|edge| {
            edge.kind == "irq"
                && edge.from_node == "soc.usart1"
                && edge.from_port == "combined"
                && edge.to_node == "soc.scs"
                && edge.to_port == "inputs:37"
        }));
        assert!(graph.edges.iter().any(|edge| {
            edge.kind == "irq"
                && edge.from_node == "soc.rcc"
                && edge.from_port == "rcc"
                && edge.to_node == "soc.scs"
                && edge.to_port == "inputs:5"
        }));
    }

    /// Engine-free replay of the firmware's MMIO sequence against the declared
    /// wiring: RCC gates, USART IRQ into NVIC input 37, and PA5.
    #[test]
    fn device_chain_gates_clocks_and_aggregates_the_usart_irq() {
        let Stm32Board {
            mut machine,
            pin_levels,
            core,
            ..
        } = build_stm32f103().expect("composition");
        let attr = attr(core);
        let wr = |machine: &mut Machine, addr: u64, value: u64| {
            machine
                .bus_write(Addr(addr), AccessSize::Word, value, &attr)
                .expect("mmio write");
        };
        let rd = |machine: &mut Machine, addr: u64| -> u64 {
            machine
                .bus_read(Addr(addr), AccessSize::Word, &attr)
                .expect("mmio read")
        };
        let drain = |machine: &mut Machine| {
            let now = machine.now();
            machine.run_until(now, 32).expect("event drain");
        };

        assert_eq!(rd(&mut machine, GPIOA_BASE + 0x04), 0);
        assert_eq!(rd(&mut machine, USART1_BASE), 0);
        wr(&mut machine, USART1_BASE + 0x0C, 0x2008);
        assert_eq!(rd(&mut machine, USART1_BASE + 0x0C), 0);

        // The RCC gates are declared wires now: the register write reaches
        // GPIOA and USART1 through the graph's clock edges.
        wr(&mut machine, RCC_BASE + 0x18, 0x4004);
        drain(&mut machine);
        assert_eq!(rd(&mut machine, RCC_BASE + 0x18), 0x4004);
        assert_eq!(rd(&mut machine, GPIOA_BASE + 0x04), 0x4444_4444);
        assert_eq!(rd(&mut machine, USART1_BASE), 0x00C0);

        // Nibble 5 (bits 20..23) = 0x3: PA5 general-purpose output.
        wr(&mut machine, GPIOA_BASE, 0x4434_4444);
        wr(&mut machine, GPIOA_BASE + 0x10, 1 << PROBE_PIN);
        drain(&mut machine);
        assert_eq!(rd(&mut machine, GPIOA_BASE + 0x0C), 1 << PROBE_PIN);
        // Enabling the output drives the reset ODR level, then BSRR raises it.
        assert_eq!(*pin_levels.lock().unwrap(), vec![false, true]);

        wr(&mut machine, USART1_BASE + 0x0C, 0x2008);
        wr(&mut machine, 0xE000E104, 1 << 5);
        wr(&mut machine, USART1_BASE + 0x0C, 0x2008 | 0x0080);
        drain(&mut machine);
        assert_eq!(rd(&mut machine, 0xE000E204), 1 << 5);
        // ICSR: ISRPENDING (bit 22) and VECTPENDING = 37 + 16 = 53.
        assert_eq!(rd(&mut machine, 0xE000ED04), 0x0043_5000);

        wr(&mut machine, USART1_BASE + 0x0C, 0x2008);
        drain(&mut machine);
        assert_eq!(rd(&mut machine, 0xE000E204), 0);
        assert_eq!(*pin_levels.lock().unwrap(), vec![false, true]);
    }
}
