//! STM32F103 (medium density, Cortex-M3) slice: the first M-profile board —
//! flash alias, SRAM, and 0x400-byte peripheral blocks at non-page offsets,
//! composed through the page-coalescing binder (構成設計書 §12.3, ADR 0012).
//!
//! Topology (STM32F103CB-class memory map, RM0008):
//! - Flash 128 KiB at `0x0800_0000`, aliased read-execute at `0x0000_0000`
//!   (BOOT0=0 remap), one backing block under both leaves
//! - SRAM 20 KiB at `0x2000_0000`
//! - RCC at `0x4002_1000` — its APB2 gate outputs clock the GPIO port and
//!   the UART (wired, ADR 0008 style)
//! - GPIOA at `0x4001_0800` — pin 5 drives a harness probe through the
//!   `model/signal` port
//! - USART1 at `0x4001_3800` — TX bytes cross the `stream/byte` pair to the
//!   console end; its combined IRQ output is a wired input of the NVIC
//! - SCS (SysTick + NVIC + SCB) at `0xE000_E000`
//!
//! The CPU is the M-class engine (ADR 0012): T32 from reset, PC writes
//! halfword-aligned. Exception vectoring needs an injection-capable backend
//! (ADR 0009), so the guest observes the NVIC at register level: the firmware
//! enables IRQ 37 (USART1) in ISER1, raises the combined UART IRQ through
//! TXEIE, and reads the pending state back through ISPR1 and ICSR
//! (VECTPENDING = 53) before clearing the source again.
//!
//! The firmware is independent T32 machine code (every halfword disassembled
//! and verified with capstone): it brings up the clocks, configures PA9/PA10
//! for the UART and PA5 as a push-pull output, prints register observations
//! as hex, and runs the IRQ raise/observe/clear cycle described above.

use core::num::NonZeroU64;
use std::sync::{Arc, Mutex};

use mosaic_assemble::{bind_flat_view_paged, MachineDriver};
use mosaic_contract_cpu_engine::CpuEngine;
use mosaic_contract_model::{ContextError, Ctx};
use mosaic_contract_signal::SignalSink;
use mosaic_core_runtime::{FlatView, Machine, Region};
use mosaic_core_types::{Addr, EventId, Mapping, MemoryKind, NodeId, Permissions, SignalId, VTime};
use mosaic_engine_unicorn::UnicornCortexM3;
use mosaic_interconnect_uart_pair::UartPair;
use mosaic_ip_arm_scs::{Scs, ScsParams};
use mosaic_ip_st_gpio_f1::{pin_signal, GpioF1, GpioF1Params};
use mosaic_ip_st_rcc_f1::{RccF1, RccF1Params, GATE_APB2_IOPA, GATE_APB2_USART1};
use mosaic_ip_st_usart_f1::{StUsartF1, StUsartF1Params};
use mosaic_isa_arm::Register;

/// Sysbus addresses of the STM32F103 map (RM0008 memory map).
pub const FLASH_BASE: u64 = 0x0800_0000;
pub const FLASH_SIZE: u64 = 0x0002_0000; // 128 KiB (medium density upper bound)
pub const ALIAS_BASE: u64 = 0x0000_0000; // flash alias, BOOT0 = 0
pub const SRAM_BASE: u64 = 0x2000_0000;
pub const SRAM_SIZE: u64 = 0x0000_5000; // 20 KiB
pub const GPIOA_BASE: u64 = 0x4001_0800;
pub const USART1_BASE: u64 = 0x4001_3800;
pub const RCC_BASE: u64 = 0x4002_1000;
pub const SCS_BASE: u64 = 0xE000_E000;
/// Initial stack pointer: top of SRAM (vector-table word 0).
pub const INITIAL_SP: u64 = SRAM_BASE + SRAM_SIZE;
/// Reset handler entry (vector-table word 1 points here with bit 0 set).
pub const RESET_HANDLER: u64 = 0x0000_0008;

/// Firmware steps: each step is one instruction quantum plus the event drain,
/// so guest polling loops observe signal deliveries between quanta.
const STEPS: usize = 64;
const QUANTUM: u64 = 32;

const CPU: NodeId = NodeId(0);
const FLASH: NodeId = NodeId(1);
const SRAM: NodeId = NodeId(2);
const GPIOA: NodeId = NodeId(3);
const USART1: NodeId = NodeId(4);
const RCC: NodeId = NodeId(5);
const SCS: NodeId = NodeId(6);
const PIN_SINK: NodeId = NodeId(7);

/// Combined USART1 interrupt output (USART1 node signal space).
const USART1_IRQ: SignalId = SignalId(1);
/// NVIC position of USART1 on the STM32F103 medium-density vector table.
const USART1_IRQ_POSITION: usize = 37;
/// PA5, the observed GPIO output.
const PROBE_PIN: u32 = 5;

/// Observed result of one full firmware run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stm32F103Report {
    /// Bytes captured on the console end of the UART pair.
    pub output: Vec<u8>,
    /// Retired instruction hooks reported by the engine.
    pub hooks: u64,
    /// Runtime events delivered during the run.
    pub events: usize,
    /// PA5 levels observed by the harness sink, in delivery order.
    pub pin_edges: Vec<bool>,
}

/// Minimal context for harness-side stream reads: no events, no signals.
struct CaptureCtx;

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

fn leaf(base: u64, size: u64, node: NodeId, kind: MemoryKind, perms: Permissions) -> Region {
    Region::leaf(
        Mapping {
            base: Addr(base),
            size,
            target: node,
            offset: 0,
            kind,
            permissions: perms,
        },
        0,
    )
}

/// Harness probe for the GPIO pin signal port.
struct PinProbe {
    levels: Arc<Mutex<Vec<bool>>>,
}

impl SignalSink for PinProbe {
    fn set_level(&mut self, high: bool) {
        self.levels.lock().expect("pin probe lock").push(high);
    }
}

/// Independent T32 machine code, hand-assembled with a verified encoder and
/// disassembled word by word with capstone (Thumb mode). Layout:
/// - `0x000` vector table (initial SP, reset handler `0x8 | 1`)
/// - `0x008` reset: clocks (RCC APB2ENR = IOPAEN | USART1EN), GPIOA CRH/CRL
///   (PA9 AF push-pull 50 MHz, PA10 input, PA5 push-pull output), USART1
///   BRR = 0x271 and CR1 = UE | TE, then the report prints.
///
/// Register conventions (harness-preloaded like the P3 r8-r10 bases):
/// r4 = USART1, r5 = RCC, r6 = GPIOA. Prints: `mosaic e=<APB2ENR>
/// s=<USART SR> o=<GPIOA ODR> p=<ISPR1> i=<ICSR> q=<ISPR1>` where the
/// p/i pair is observed after enabling ISER1.USART1 and CR1.TXEIE (the
/// combined IRQ output rises through the wired NVIC input) and q after
/// clearing TXEIE again (the line falls, pending drops). hex print shows
/// the exact values the golden output pins.
static FIRMWARE_WORDS: [u32; 85] = [
    0x50002000, 0x00090000, 0x210469A8, 0x21404308, 0x43080209, 0x484861A8, 0x48486070, 0x20206030,
    0x48476130, 0x200860A0, 0x02092120, 0x60E04308, 0xF000216D, 0x216FF86E, 0xF86BF000, 0xF0002173,
    0x2161F868, 0xF865F000, 0xF0002169, 0x2163F862, 0xF85FF000, 0xF0002120, 0x2165F85C, 0xF859F000,
    0xF000213D, 0x69A8F856, 0xF858F000, 0xF0002120, 0x2173F850, 0xF84DF000, 0xF000213D, 0x6820F84A,
    0xF84CF000, 0xF0002120, 0x216FF844, 0xF841F000, 0xF000213D, 0x68F0F83E, 0xF840F000, 0x2120482A,
    0x68E06001, 0x43082180, 0x482760E0, 0x28206800, 0x2120D1FB, 0xF82DF000, 0xF0002170, 0x213DF82A,
    0xF827F000, 0xF82AF000, 0xF0002120, 0x2169F822, 0xF81FF000, 0xF000213D, 0x481CF81C, 0xF0006800,
    0x68E0F81D, 0x40012180, 0x60E04048, 0x68004817, 0xD1FB2800, 0xF0002120, 0x2171F80C, 0xF809F000,
    0xF000213D, 0xF000F806, 0x210AF809, 0xF801F000, 0x6823E7FE, 0xD5FC061B, 0x47707121, 0x2208B500,
    0x09000701, 0x290A0F09, 0x3937D301, 0x3130E000, 0xFFEFF7FF, 0xD1F33A01, 0x0000BD00, 0x444444B4,
    0x44434444, 0x00000271, 0xE000E104, 0xE000E204, 0xE000ED04,
];

fn firmware() -> Vec<u8> {
    let mut bytes = Vec::with_capacity(FIRMWARE_WORDS.len() * 4);
    for word in FIRMWARE_WORDS {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    bytes
}

/// The composed F103 machine plus the harness-owned ends of its contracts.
struct F103 {
    view: FlatView,
    machine: Machine,
    console: mosaic_interconnect_uart_pair::UartEndpoint,
    pin_levels: Arc<Mutex<Vec<bool>>>,
}

/// Compose the F103 topology. Shared by the engine-driven run and the
/// engine-free device-chain test.
fn build_f103() -> Result<F103, String> {
    // One flash backing block under two leaves: the alias at 0x0 (RX) and the
    // physical block at 0x0800_0000. SRAM is a separate RW block.
    let view = FlatView::build(&[
        leaf(
            ALIAS_BASE,
            FLASH_SIZE,
            FLASH,
            MemoryKind::Rom,
            Permissions::RX,
        ),
        leaf(
            FLASH_BASE,
            FLASH_SIZE,
            FLASH,
            MemoryKind::Rom,
            Permissions::RX,
        ),
        leaf(SRAM_BASE, SRAM_SIZE, SRAM, MemoryKind::Ram, Permissions::RW),
        leaf(GPIOA_BASE, 0x1C, GPIOA, MemoryKind::Mmio, Permissions::RW),
        leaf(USART1_BASE, 0x1C, USART1, MemoryKind::Mmio, Permissions::RW),
        leaf(RCC_BASE, 0x28, RCC, MemoryKind::Mmio, Permissions::RW),
        leaf(SCS_BASE, 0x1000, SCS, MemoryKind::Mmio, Permissions::RW),
    ])
    .map_err(|error| format!("layout: {error:?}"))?;

    // ADR 0008 wiring: the UART owns one stream endpoint, the harness the
    // other; neither side names a concrete model.
    let pair = UartPair::new(128);
    let (uart_peer, console) = pair.endpoints();

    let mut machine = Machine::new(view.clone(), 32);
    machine
        .add_backing(FLASH, FLASH_SIZE)
        .map_err(|error| format!("flash backing: {error:?}"))?;
    machine
        .add_backing(SRAM, SRAM_SIZE)
        .map_err(|error| format!("sram backing: {error:?}"))?;

    // RCC gate inputs are wired inputs of each consumer (ADR 0008).
    let mut gpio = GpioF1::new(GpioF1Params);
    gpio.bind_clock(RCC, GATE_APB2_IOPA);
    machine
        .register_device(GPIOA, Box::new(gpio))
        .map_err(|error| format!("gpioa register: {error:?}"))?;
    let mut usart = StUsartF1::new(StUsartF1Params::new(USART1_IRQ), Some(Box::new(uart_peer)));
    usart.bind_clock(RCC, GATE_APB2_USART1);
    machine
        .register_device(USART1, Box::new(usart))
        .map_err(|error| format!("usart1 register: {error:?}"))?;
    machine
        .register_device(RCC, Box::new(RccF1::new(RccF1Params::new(SignalId(2)))))
        .map_err(|error| format!("rcc register: {error:?}"))?;
    // USART1 IRQ 37 is a wired input of the NVIC (RM0008 vector table).
    let mut scs = Scs::new(ScsParams::default());
    scs.bind_irq(USART1_IRQ_POSITION, USART1, USART1_IRQ)
        .map_err(|error| format!("scs usart1 line: {error}"))?;
    machine
        .register_device(SCS, Box::new(scs))
        .map_err(|error| format!("scs register: {error:?}"))?;

    let pin_levels = Arc::new(Mutex::new(Vec::new()));
    machine
        .register_sink(
            PIN_SINK,
            Box::new(PinProbe {
                levels: Arc::clone(&pin_levels),
            }),
        )
        .map_err(|error| format!("pin sink register: {error:?}"))?;

    // Gate outputs, the UART IRQ output, and the observed GPIO pin.
    machine
        .connect(RCC, GATE_APB2_IOPA, GPIOA)
        .map_err(|error| format!("gpioa gate wiring: {error:?}"))?;
    machine
        .connect(RCC, GATE_APB2_USART1, USART1)
        .map_err(|error| format!("usart1 gate wiring: {error:?}"))?;
    machine
        .connect(USART1, USART1_IRQ, SCS)
        .map_err(|error| format!("usart1 irq wiring: {error:?}"))?;
    machine
        .connect(GPIOA, pin_signal(PROBE_PIN), PIN_SINK)
        .map_err(|error| format!("pin wiring: {error:?}"))?;
    machine
        .verify()
        .map_err(|error| format!("verify: {error:?}"))?;

    Ok(F103 {
        view,
        machine,
        console,
        pin_levels,
    })
}

/// Build and run the F103 machine once. Two calls must produce identical
/// reports, which the determinism check in `main` relies on.
pub fn run_stm32f103() -> Result<Stm32F103Report, String> {
    let F103 {
        view,
        mut machine,
        mut console,
        pin_levels,
    } = build_f103()?;
    machine
        .loader_write(FLASH, 0, &firmware())
        .map_err(|error| format!("load: {error:?}"))?;

    // M-class reset: the engine asserts T32 through the PC write path
    // (ADR 0012); the harness performs the vector-table loads (SP from word
    // 0, PC from word 1 minus the T bit) because exception/vector autoload
    // needs an injection-capable backend (ADR 0009).
    let mut engine = UnicornCortexM3::new().map_err(|error| format!("engine: {error:?}"))?;
    let shared = bind_flat_view_paged(&mut engine, &view, Arc::new(Mutex::new(machine)), CPU)
        .map_err(|error| format!("bind: {error:?}"))?;
    engine
        .reg_write(Register::Sp.into(), INITIAL_SP)
        .map_err(|error| format!("sp: {error:?}"))?;
    engine
        .reg_write(Register::Pc.into(), RESET_HANDLER)
        .map_err(|error| format!("pc: {error:?}"))?;
    engine
        .reg_write(Register::R4.into(), USART1_BASE)
        .map_err(|error| format!("r4: {error:?}"))?;
    engine
        .reg_write(Register::R5.into(), RCC_BASE)
        .map_err(|error| format!("r5: {error:?}"))?;
    engine
        .reg_write(Register::R6.into(), GPIOA_BASE)
        .map_err(|error| format!("r6: {error:?}"))?;

    let mut driver = MachineDriver::new(engine, shared);
    let quantum = NonZeroU64::new(QUANTUM).ok_or("invalid quantum")?;
    let mut hooks = 0;
    let mut events = 0;
    for _ in 0..STEPS {
        let report = driver
            .step(quantum, 32)
            .map_err(|error| format!("step: {error:?}"))?;
        hooks += report.engine.instruction_hooks;
        events += report.events_delivered;
    }

    let mut buffer = vec![0u8; 128];
    let read = console
        .read_bytes(&mut CaptureCtx, &mut buffer)
        .map_err(|error| format!("console read: {error:?}"))?;
    buffer.truncate(read);
    Ok(Stm32F103Report {
        output: buffer,
        hooks,
        events,
        pin_edges: pin_levels
            .lock()
            .map(|levels| levels.clone())
            .map_err(|_| "pin probe poisoned")?,
    })
}

/// Expected golden console output for the F103 firmware:
/// - `e=00004004` — APB2ENR after IOPAEN | USART1EN
/// - `s=000000C0` — USART1 SR: TXE | TC (instant-drain model)
/// - `o=00000020` — GPIOA ODR: PA5 driven high through BSRR
/// - `p=00000020` — ISPR1: USART1 (IRQ 37) pending after TXEIE raised the
///   combined IRQ output into the wired NVIC input
/// - `i=4000D000` — ICSR: VECTPENDING = 37 + 16 = 53, ISRPENDING set
/// - `q=00000000` — ISPR1 after clearing TXEIE dropped the line and the
///   level-approximated pending bit
pub const GOLDEN_OUTPUT: &[u8] =
    b"mosaic e=00004004 s=000000C0 o=00000020 p=00000020 i=4000D000 q=00000000\n";

#[cfg(test)]
mod tests {
    use super::*;
    use mosaic_core_types::{AccessKind, AccessSize, Attr, Endian, Security};

    fn attr() -> Attr {
        Attr {
            master: CPU,
            security: Security::Secure,
            privileged: true,
            kind: AccessKind::Data,
            exclusive: false,
            byte_enable: 0,
            endian: Endian::Little,
        }
    }

    /// Engine-free replay of the firmware's MMIO sequence against the same
    /// wiring: the clock-gate chain (RCC APB2ENR -> GPIOA/USART1), the UART
    /// IRQ chain (TXEIE -> combined output -> wired NVIC input -> ISPR1 and
    /// ICSR), and the GPIO pin signal (BSRR -> PA5 probe). The T32 encoding
    /// is capstone-verified and the guest-level golden runs in CI; this test
    /// pins the register-level behaviour without a backend.
    #[test]
    fn device_chain_gates_clocks_and_aggregates_the_usart_irq() {
        let F103 {
            mut machine,
            pin_levels,
            ..
        } = build_f103().expect("composition");
        let attr = attr();

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
        // Drain the events queued at the current virtual time, like one
        // driver step boundary.
        let drain = |machine: &mut Machine| {
            let now = machine.now();
            machine.run_until(now, 32).expect("event drain");
        };

        // Before the APB2 gates open, both ports read as zero and ignore
        // writes (unclocked peripheral on the APB bridge).
        assert_eq!(rd(&mut machine, GPIOA_BASE + 0x04), 0);
        assert_eq!(rd(&mut machine, USART1_BASE + 0x00), 0);
        wr(&mut machine, USART1_BASE + 0x0C, 0x2008);
        assert_eq!(rd(&mut machine, USART1_BASE + 0x0C), 0);

        // Firmware words 4..10: APB2ENR = IOPAEN | USART1EN.
        wr(&mut machine, RCC_BASE + 0x18, 0x4004);
        drain(&mut machine);
        assert_eq!(rd(&mut machine, RCC_BASE + 0x18), 0x4004);
        // Now the ports answer with their reset values.
        assert_eq!(rd(&mut machine, GPIOA_BASE + 0x04), 0x4444_4444);
        assert_eq!(rd(&mut machine, USART1_BASE + 0x00), 0x00C0);

        // GPIOA: PA5 as push-pull output, then BSRR set. The pin signal
        // reaches the harness probe after the drain.
        wr(&mut machine, GPIOA_BASE + 0x00, 0x4443_4444);
        wr(&mut machine, GPIOA_BASE + 0x10, 1 << PROBE_PIN);
        drain(&mut machine);
        assert_eq!(rd(&mut machine, GPIOA_BASE + 0x0C), 1 << PROBE_PIN);
        assert_eq!(*pin_levels.lock().unwrap(), vec![true]);

        // USART1: enable, then raise the combined IRQ through TXEIE. The
        // line crosses into the wired NVIC input; pending lands on IRQ 37.
        wr(&mut machine, USART1_BASE + 0x0C, 0x2008);
        wr(&mut machine, SCS_BASE + 0x104, 1 << 5); // ISER1: USART1
        wr(&mut machine, USART1_BASE + 0x0C, 0x2008 | 0x0080); // + TXEIE
        drain(&mut machine);
        assert_eq!(rd(&mut machine, SCS_BASE + 0x204), 1 << 5); // ISPR1
                                                                // ICSR: VECTPENDING = 37 + 16 = 53, ISRPENDING set.
        assert_eq!(rd(&mut machine, SCS_BASE + 0xD04), 0x4000_D000);

        // Clearing the source drops the line; the level-approximated input
        // clears the pending bit again.
        wr(&mut machine, USART1_BASE + 0x0C, 0x2008);
        drain(&mut machine);
        assert_eq!(rd(&mut machine, SCS_BASE + 0x204), 0);
        // The pin line never dropped: exactly one edge so far.
        assert_eq!(*pin_levels.lock().unwrap(), vec![true]);
    }

    /// The firmware image starts with the vector table (SP, reset handler
    /// with the T bit) and the reset code begins at 0x8.
    #[test]
    fn firmware_vector_table_and_entry() {
        let image = firmware();
        assert_eq!(image.len(), FIRMWARE_WORDS.len() * 4);
        let word = |offset: usize| -> u32 {
            u32::from_le_bytes(image[offset..offset + 4].try_into().expect("vector word"))
        };
        assert_eq!(word(0), INITIAL_SP as u32);
        assert_eq!(word(4), 0x0000_0009); // reset = 0x8 | 1
    }
}
