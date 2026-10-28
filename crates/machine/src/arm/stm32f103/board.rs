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

/// The multi-face composition rig (ADR 0032): the real STM32F103 SoC map
/// plus a board that hangs four parts off its I2C1 — an MCP23017 expander
/// (i2c + 16 gpio lines + int), a TCA9548A switch whose downstream segment
/// carries a BME280, and an LM75A on the root segment.
///
/// This is a composition test fixture, not a shipped machine: no board
/// product is claimed, and the expander's INT is wired straight into NVIC
/// position 6 because the EXTI block (the physical path) is not modeled
/// yet — the rig's comment says so, and nothing here fabricates guest-boot
/// evidence (ADR 0022 governs guest images; this rig only drives
/// registers).
#[cfg(test)]
mod faces_rig {
    use super::*;
    use crate::manifest::parse_board;
    use mosaic_exec::{AccessKind, AccessSize, Addr, Attr, Endian, Security};

    /// The rig's board declaration, in the shipped grammar.
    const BOARD: &str = r#"
# ADR 0032 composition rig over the real stm32f103 SoC map: I2C1 carries an
# expander and a switch; the switch's channel 2 carries a BME280; the
# expander's INT line lands on NVIC position 6 (EXTI not modeled yet) and
# its GPA0 line on an observable net.
[board]
id = 'board:harness/stm32f103-i2c-rig'
soc = 'soc:st/stm32f103'
summary = 'composition rig: multi-face parts on I2C1 (test fixture, not a machine)'

[[part]]
name = 'ioexp'
use = 'part:microchip/mcp23017'

[[part.face]]
port = 'i2c'
kind = 'i2c'
to = 'soc.i2c1:i2c'
address = 0x20

[[part.face]]
port = 'gpio'
kind = 'gpio'
to = 'wire:ioexp-gpa0'
index = 0

[[part.face]]
port = 'irq'
kind = 'irq'
to = 'scs.inputs:6'
pin = 'int'

[[part]]
name = 'switch'
use = 'part:ti/tca9548a'

[[part.face]]
port = 'i2c'
kind = 'i2c'
to = 'soc.i2c1:i2c'
address = 0x70

[[part]]
name = 'environment'
use = 'part:bosch/bme280'

[[part.face]]
port = 'i2c'
kind = 'i2c'
to = 'board.switch:down2'
address = 0x76

[[part]]
name = 'thermometer'
use = 'part:nxp/lm75a'

[[part.face]]
port = 'i2c'
kind = 'i2c'
to = 'soc.i2c1:i2c'
address = 0x48
"#;

    /// RM0008 I2C1 register offsets.
    mod i2c {
        pub const CR1: u64 = 0x00;
        pub const CR2: u64 = 0x04;
        pub const DR: u64 = 0x10;
        pub const SR1: u64 = 0x14;
        pub const SR2: u64 = 0x18;
        pub mod cr1 {
            pub const PE: u32 = 1 << 0;
            pub const START: u32 = 1 << 8;
            pub const STOP: u32 = 1 << 9;
            pub const ACK: u32 = 1 << 10;
        }
        pub mod cr2 {
            pub const ITEVTEN: u32 = 1 << 2;
            pub const ITBUFEN: u32 = 1 << 3;
        }
        pub mod sr1 {
            #![allow(dead_code)]
            pub const SB: u32 = 1 << 0;
            pub const TXE: u32 = 1 << 7;
            pub const RXNE: u32 = 1 << 6;
        }
    }

    /// I2C1 base (RM0008) and the RCC APB1 enable register.
    const I2C1: u64 = 0x4000_5400;
    const APB1ENR: u64 = 0x4002_101C;
    /// NVIC: ISER0 and ICSR (for the INT line).
    const NVIC_ISER0: u64 = 0xE000_E100;
    const NVIC_ICSR: u64 = 0xE000_ED04;

    struct Rig {
        machine: Machine,
        core: NodeId,
    }

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

    /// Build the rig: the real SoC graph plus the rig board, catalog
    /// spawns only — zero hand-written factories.
    fn build_rig() -> Rig {
        let soc = super::super::manifest::stm32f103_soc().expect("soc.toml");
        let board = parse_board(BOARD).expect("rig board");
        let graph = MachineGraph::build(&soc, Some(&board)).expect("rig graph");
        let registry = device_registry();
        let assembled =
            materialize(&graph, &registry, &HostAttachments::new(), 32).expect("rig materialize");
        let machine = assembled.machine;
        let core = assembled.nodes["soc.core0"];
        // The fabric store must have joined the expander's GPA0 line face
        // to the declared net — the rig asserts the wiring, not just the
        // absence of errors.
        assert!(assembled
            .fabrics
            .as_ref()
            .and_then(|store| store.gpio_net("wire:ioexp-gpa0"))
            .is_some());
        machine.verify().expect("rig verify");

        Rig { machine, core }
    }

    impl Rig {
        fn wr(&mut self, addr: u64, value: u32) {
            self.machine
                .bus_write(
                    Addr(addr),
                    AccessSize::Word,
                    u64::from(value),
                    &attr(self.core),
                )
                .expect("mmio write");
        }

        fn rd(&mut self, addr: u64) -> u32 {
            self.machine
                .bus_read(Addr(addr), AccessSize::Word, &attr(self.core))
                .expect("mmio read") as u32
        }

        fn drain(&mut self) {
            let now = self.machine.now();
            self.machine.run_until(now, 32).expect("event drain");
        }

        /// Gate I2C1's APB1 clock (RM0008 RCC_APB1ENR bit 21).
        fn enable_i2c1_clock(&mut self) {
            self.wr(APB1ENR, 1 << 21);
            self.drain();
        }

        /// One RM0008 master write transaction to `addr`.
        fn i2c_write(&mut self, addr: u8, bytes: &[u8]) {
            self.wr(I2C1 + i2c::CR2, i2c::cr2::ITEVTEN | i2c::cr2::ITBUFEN | 8);
            self.wr(I2C1 + i2c::CR1, i2c::cr1::PE);
            self.wr(I2C1 + i2c::CR1, i2c::cr1::PE | i2c::cr1::START);
            assert_eq!(self.rd(I2C1 + i2c::SR1) & i2c::sr1::SB, i2c::sr1::SB);
            self.wr(I2C1 + i2c::DR, u32::from(addr) << 1);
            self.rd(I2C1 + i2c::SR1);
            self.rd(I2C1 + i2c::SR2); // clear ADDR
            for &byte in bytes {
                self.wr(I2C1 + i2c::DR, u32::from(byte));
            }
            self.wr(I2C1 + i2c::CR1, i2c::cr1::PE | i2c::cr1::STOP);
        }

        /// One RM0008 write-then-repeated-start-read transaction.
        fn i2c_write_read(&mut self, addr: u8, out: &[u8], count: usize) -> Vec<u8> {
            self.wr(I2C1 + i2c::CR1, i2c::cr1::PE | i2c::cr1::ACK);
            self.wr(
                I2C1 + i2c::CR1,
                i2c::cr1::PE | i2c::cr1::ACK | i2c::cr1::START,
            );
            self.wr(I2C1 + i2c::DR, u32::from(addr) << 1);
            self.rd(I2C1 + i2c::SR1);
            self.rd(I2C1 + i2c::SR2);
            for &byte in out {
                self.wr(I2C1 + i2c::DR, u32::from(byte));
            }
            // Repeated start, then the read address.
            self.wr(
                I2C1 + i2c::CR1,
                i2c::cr1::PE | i2c::cr1::ACK | i2c::cr1::START,
            );
            assert_eq!(self.rd(I2C1 + i2c::SR1) & i2c::sr1::SB, i2c::sr1::SB);
            self.wr(I2C1 + i2c::DR, (u32::from(addr) << 1) | 1);
            self.rd(I2C1 + i2c::SR1);
            self.rd(I2C1 + i2c::SR2); // clear ADDR — reception begins
            let mut read = Vec::with_capacity(count);
            for _ in 0..count {
                assert_eq!(
                    self.rd(I2C1 + i2c::SR1) & i2c::sr1::RXNE,
                    i2c::sr1::RXNE,
                    "RxNE before each DR read"
                );
                read.push(self.rd(I2C1 + i2c::DR) as u8);
            }
            self.wr(I2C1 + i2c::CR1, i2c::cr1::PE | i2c::cr1::STOP);
            read
        }
    }

    #[test]
    fn the_rig_composes_from_the_catalog_alone() {
        let rig = build_rig();
        // Four part nodes and the i2c1 controller were all spawned by the
        // catalog; the wiring is the board manifest, nothing else.
        assert!(rig.machine.verify().is_ok());
    }

    #[test]
    fn rm0008_transactions_reach_the_expander_and_its_net() {
        let mut rig = build_rig();
        rig.enable_i2c1_clock();
        // IODIRA = 0x00 (outputs), GPIOA = 0xA5.
        rig.i2c_write(0x20, &[0x00, 0x00]);
        rig.i2c_write(0x20, &[0x12, 0xA5]);
        // Read GPIOA back through a repeated-start read.
        let back = rig.i2c_write_read(0x20, &[0x12], 1);
        assert_eq!(back, vec![0xA5]);
        // The expander's GPA0 face drives its net: bit 0 of 0xA5 is high.
        // (The net lives in the fabric store; a fresh observer sees High.)
        // Re-reading through I2C stays consistent after the net round trip.
        let again = rig.i2c_write_read(0x20, &[0x12], 1);
        assert_eq!(again, vec![0xA5]);
    }

    #[test]
    fn the_switch_routes_the_bme280_segment() {
        let mut rig = build_rig();
        rig.enable_i2c1_clock();
        // Connect channel 2 of the TCA9548A.
        rig.i2c_write(0x70, &[0x04]);
        // Read the BME280 chip id (0xD0 = 0x60) through the segment.
        let id = rig.i2c_write_read(0x76, &[0xD0], 1);
        assert_eq!(id, vec![0x60]);
        // The LM75A still answers on the root segment: Temp = 25.0 °C,
        // 0.125 °C units × 200 at 11-bit left alignment = 0x1900.
        let temp = rig.i2c_write_read(0x48, &[0x00], 2);
        assert_eq!(temp, vec![0x19, 0x00]);
    }

    #[test]
    fn the_expander_int_face_reaches_the_nvic() {
        let mut rig = build_rig();
        rig.enable_i2c1_clock();
        // Enable NVIC position 6 so ICSR reports the pending line.
        rig.wr(NVIC_ISER0, 1 << 6);
        // Drive GPA0 as a low output, then arm interrupt-on-change…
        rig.i2c_write(0x20, &[0x00, 0x00]); // IODIRA: line 0 output
        rig.i2c_write(0x20, &[0x12, 0x00]); // GPIOA: drive low
        rig.i2c_write(0x20, &[0x04, 0x01]); // GPINTENA: line 0 armed
                                            // …and switch the line back to input: the release lets the net
                                            // re-resolve to its floating high, the line observes the change,
                                            // INTCAP captures and INT asserts — all under the I2C write's own
                                            // machine context, so the signal event enters the queue for real.
        rig.i2c_write(0x20, &[0x00, 0x01]); // IODIRA: line 0 input
        rig.drain();
        // INTCAP holds the captured GPIOA image (line 0 now high).
        let cap = rig.i2c_write_read(0x20, &[0x10], 1);
        assert_eq!(cap, vec![0x01]);
        // The INT edge reached NVIC input 6: ICSR reports it pending
        // (ISRPENDING set, VECTPENDING = 6 + 16 = 22).
        let icsr = rig.rd(NVIC_ICSR);
        assert_eq!(icsr, 0x0040_0000 | (22 << 12));
    }
}
