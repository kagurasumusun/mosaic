//! STM32F103 composition from `catalog/soc/st/stm32f103/soc.toml`.
//!
//! The graph names the blocks and the USART1 → NVIC route. RCC gate outputs
//! are the RCC model's published signal IDs (ADR 0008); the host UART pair and
//! the PA5 probe are not part of the SoC declaration.

use std::sync::{Arc, Mutex};

use mosaic_assemble::{materialize, DeviceRegistry, InstantiateCtx, MaterializeError};
use mosaic_contract_model::{ContextError, Ctx};
use mosaic_contract_signal::SignalSink;
use mosaic_core_runtime::{FlatView, Machine};
use mosaic_core_types::{EventId, NodeId, SignalId, VTime};
use mosaic_describe::MachineGraph;
use mosaic_interconnect_uart_pair::{UartEndpoint, UartPair};
use mosaic_ip_arm_scs::{Scs, ScsParams};
use mosaic_ip_st_gpio_f1::{pin_signal, GpioF1, GpioF1Params};
use mosaic_ip_st_rcc_f1::{RccF1, RccF1Params, GATE_APB2_IOPA, GATE_APB2_USART1};
use mosaic_ip_st_usart_f1::{StUsartF1, StUsartF1Params};

use crate::manifest::stm32f103_soc;

#[cfg_attr(not(test), allow(dead_code))]
pub const FLASH_BASE: u64 = 0x0800_0000;
#[cfg_attr(not(test), allow(dead_code))]
pub const FLASH_SIZE: u64 = 0x0002_0000;
pub const SRAM_BASE: u64 = 0x2000_0000;
pub const SRAM_SIZE: u64 = 0x0000_5000;
pub const GPIOA_BASE: u64 = 0x4001_0800;
pub const USART1_BASE: u64 = 0x4001_3800;
pub const RCC_BASE: u64 = 0x4002_1000;
#[cfg_attr(not(test), allow(dead_code))]
pub const SCS_BASE: u64 = 0xE000_E000;
/// PA5, the observed GPIO output. CRL nibble 5 is bits 20..23, not nibble 4.
pub const PROBE_PIN: u32 = 5;

/// The declared machine plus the host ends the SoC graph does not own.
///
/// `view`, `console` and `flash` are consumed by the feature-gated engine
/// harness. They stay on the struct so that harness does not rebuild the map.
#[allow(dead_code)]
pub struct Stm32Board {
    pub view: FlatView,
    pub machine: Machine,
    pub console: UartEndpoint,
    pub pin_levels: Arc<Mutex<Vec<bool>>>,
    pub core: NodeId,
    pub flash: NodeId,
}

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

/// Build the F103 machine from the catalog declaration.
pub fn build_stm32f103() -> Result<Stm32Board, String> {
    let graph = graph()?;
    let pair = UartPair::new(128);
    let (uart_peer, console) = pair.endpoints();
    let uart_peer = Mutex::new(Some(uart_peer));

    let mut registry = DeviceRegistry::new();
    registry
        .register(
            "ip:st/gpio-f1",
            |ctx: &InstantiateCtx, machine: &mut Machine| {
                let rcc = ctx.node("soc.rcc")?;
                let mut gpio = GpioF1::new(GpioF1Params);
                gpio.bind_clock(rcc, GATE_APB2_IOPA);
                machine.register_device(ctx.node, Box::new(gpio))?;
                Ok(())
            },
        )
        .map_err(|error| error.to_string())?;
    registry
        .register(
            "ip:st/usart-f1",
            move |ctx: &InstantiateCtx, machine: &mut Machine| {
                let peer = uart_peer
                    .lock()
                    .map_err(|_| MaterializeError::Bind {
                        node: ctx.graph_id.to_owned(),
                        detail: "uart peer lock poisoned".into(),
                    })?
                    .take()
                    .ok_or_else(|| MaterializeError::Bind {
                        node: ctx.graph_id.to_owned(),
                        detail: "USART stream endpoint already taken".into(),
                    })?;
                let rcc = ctx.node("soc.rcc")?;
                let mut usart = StUsartF1::new(
                    StUsartF1Params::new(ctx.output("combined")?),
                    Some(Box::new(peer)),
                );
                usart.bind_clock(rcc, GATE_APB2_USART1);
                machine.register_device(ctx.node, Box::new(usart))?;
                Ok(())
            },
        )
        .map_err(|error| error.to_string())?;
    registry
        .register(
            "ip:st/rcc-f1",
            |ctx: &InstantiateCtx, machine: &mut Machine| {
                machine.register_device(
                    ctx.node,
                    Box::new(RccF1::new(RccF1Params::new(SignalId(1)))),
                )?;
                Ok(())
            },
        )
        .map_err(|error| error.to_string())?;
    registry
        .register(
            "ip:arm/scs",
            |ctx: &InstantiateCtx, machine: &mut Machine| {
                let mut scs = Scs::new(ScsParams::default());
                for wire in ctx.incoming {
                    let Some(index) = wire.index() else {
                        continue;
                    };
                    scs.bind_irq(index, wire.source, wire.signal)
                        .map_err(|detail| MaterializeError::Bind {
                            node: ctx.graph_id.to_owned(),
                            detail,
                        })?;
                }
                machine.register_device(ctx.node, Box::new(scs))?;
                Ok(())
            },
        )
        .map_err(|error| error.to_string())?;

    let mut assembled = materialize(&graph, &registry, 32).map_err(|error| error.to_string())?;
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
        .connect(gpioa, pin_signal(PROBE_PIN), probe)
        .map_err(|error| format!("pin wiring: {error:?}"))?;
    // RCC gate IDs are the model’s published outputs, not graph edges.
    // connect() needs both devices registered, so it cannot run inside a factory.
    let rcc = assembled
        .nodes
        .get("soc.rcc")
        .copied()
        .ok_or("soc.rcc was not allocated")?;
    let usart = assembled
        .nodes
        .get("soc.usart1")
        .copied()
        .ok_or("soc.usart1 was not allocated")?;
    assembled
        .machine
        .connect(rcc, GATE_APB2_IOPA, gpioa)
        .map_err(|error| format!("gpioa gate wiring: {error:?}"))?;
    assembled
        .machine
        .connect(rcc, GATE_APB2_USART1, usart)
        .map_err(|error| format!("usart1 gate wiring: {error:?}"))?;
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
    use mosaic_core_types::{AccessKind, AccessSize, Addr, Attr, Endian, Security};

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
        assert_eq!(flash.base, FLASH_BASE);
        assert_eq!(flash.size, FLASH_SIZE);
        let sram = graph
            .regions
            .iter()
            .find(|region| region.name == "sram")
            .expect("sram region");
        assert_eq!(sram.base, SRAM_BASE);
        assert_eq!(sram.size, SRAM_SIZE);
        assert!(graph.edges.iter().any(|edge| {
            edge.kind == "irq"
                && edge.from_node == "soc.usart1"
                && edge.from_port == "combined"
                && edge.to_node == "soc.scs"
                && edge.to_port == "inputs:37"
        }));
        let flash = graph
            .regions
            .iter()
            .filter(|region| region.target_node == "memory.flash")
            .count();
        assert_eq!(flash, 2);
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

        wr(&mut machine, RCC_BASE + 0x18, 0x4004);
        drain(&mut machine);
        assert_eq!(rd(&mut machine, RCC_BASE + 0x18), 0x4004);
        assert_eq!(rd(&mut machine, GPIOA_BASE + 0x04), 0x4444_4444);
        assert_eq!(rd(&mut machine, USART1_BASE), 0x00C0);

        // Nibble 5 (bits 20..23) = 0x3: PA5 general-purpose output. The older
        // literal 0x4443_4444 configured pin 4 and then probed pin 5.
        wr(&mut machine, GPIOA_BASE, 0x4434_4444);
        wr(&mut machine, GPIOA_BASE + 0x10, 1 << PROBE_PIN);
        drain(&mut machine);
        assert_eq!(rd(&mut machine, GPIOA_BASE + 0x0C), 1 << PROBE_PIN);
        // Enabling the output drives the reset ODR level, then BSRR raises it.
        assert_eq!(*pin_levels.lock().unwrap(), vec![false, true]);

        wr(&mut machine, USART1_BASE + 0x0C, 0x2008);
        wr(&mut machine, SCS_BASE + 0x104, 1 << 5);
        wr(&mut machine, USART1_BASE + 0x0C, 0x2008 | 0x0080);
        drain(&mut machine);
        assert_eq!(rd(&mut machine, SCS_BASE + 0x204), 1 << 5);
        // ICSR: ISRPENDING (bit 22) and VECTPENDING = 37 + 16 = 53. The SCS
        // model’s own test is the authority; 0x4000_D000 was a bad expectation.
        assert_eq!(rd(&mut machine, SCS_BASE + 0xD04), 0x0043_5000);

        wr(&mut machine, USART1_BASE + 0x0C, 0x2008);
        drain(&mut machine);
        assert_eq!(rd(&mut machine, SCS_BASE + 0x204), 0);
        assert_eq!(*pin_levels.lock().unwrap(), vec![false, true]);
    }
}
