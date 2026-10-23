//! P1 vertical-slice bring-up: one ARM core, PL011 UART and SP804 timer on
//! the Versatile/PB sysbus. A small A32 firmware enables the UART and writes
//! `mosaic\n`; the bytes cross the runtime MMIO path, the PL011 model and the
//! UART-pair interconnect before the test harness reads them back. Everything
//! here is composition (ADR 0008): the catalog models never know the addresses
//! or each other.

use core::num::NonZeroU64;
use std::sync::{Arc, Mutex};

use mosaic_assemble::{bind_flat_view, EngineIrqSink, IrqMailbox, MachineDriver};
use mosaic_contract_model::{ContextError, Ctx};
use mosaic_contract_stream::ByteStream;
use mosaic_core_runtime::{FlatView, Machine, Region};
use mosaic_core_types::{Addr, EventId, Mapping, MemoryKind, NodeId, Permissions, SignalId, VTime};
use mosaic_engine_unicorn::{UnicornArm, ARM_IRQ};
use mosaic_interconnect_uart_pair::UartPair;
use mosaic_ip_arm_pl011::{Pl011, Pl011Params};
use mosaic_ip_arm_sp804::{Sp804, Sp804Params};
use mosaic_isa_arm::Register;

/// Sysbus addresses, mirrored from `catalog/soc/arm/versatile/arm926ej-s/soc.toml`.
pub const RAM_BASE: u64 = 0x0000_0000;
pub const RAM_SIZE: u64 = 0x0010_0000;
pub const TIMER_BASE: u64 = 0x101E_2000;
pub const UART_BASE: u64 = 0x101F_1000;
const PAGE: u64 = 4096;

const CPU: NodeId = NodeId(0);
const RAM: NodeId = NodeId(1);
const UART: NodeId = NodeId(2);
const TIMER: NodeId = NodeId(3);
const IRQ_SINK: NodeId = NodeId(4);

const UART_IRQ: SignalId = SignalId(1);
const TIMER_IRQ1: SignalId = SignalId(2);
const TIMER_IRQ2: SignalId = SignalId(3);

/// Calibrated SP804 input clock for the slice: 1 ns per timer tick. Real board
/// calibration (24 MHz/32 kHz) is a board parameter, not an IP constant.
const TIMER_TICK_PS: NonZeroU64 = match NonZeroU64::new(1_000) {
    Some(value) => value,
    None => unreachable!(),
};

/// Observed result of one full firmware run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BringupReport {
    /// Bytes captured on the console end of the UART pair.
    pub output: Vec<u8>,
    /// Retired instruction hooks reported by the engine.
    pub hooks: u64,
    /// Runtime events delivered during the step.
    pub events: usize,
    /// `(line, high)` edges that reached the engine, in delivery order.
    pub irq_edges: Vec<(u32, bool)>,
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

/// Independent A32 machine code, not a vendor firmware image.
///
/// Prologue enables TX with UARTEN (the model resets with UARTEN clear), then a
/// poll-free loop streams the NUL-terminated string at word index 9.
fn firmware() -> Vec<u8> {
    let mut words = vec![
        0xE3A0_1001, // MOV  R1, #1
        0xE381_1C01, // ORR  R1, R1, #0x100 (modified immediate, rot 12)
        0xE582_1030, // STR  R1, [R2, #0x30]      ; UARTCR = UARTEN | TXE
        0xE4D3_0001, // LDRB R0, [R3], #1
        0xE350_0000, // CMP  R0, #0
        0x0A00_0000, // BEQ  spin (patched below)
        0xE582_0000, // STR  R0, [R2]             ; UARTDR
        0xEA00_0000, // B    send (patched below)
        0xEAFF_FFFE, // spin: B .
    ];
    // Branch offsets are relative to the instruction after the branch.
    words[5] |= u32::try_from(8_usize - 6).unwrap();
    words[7] |= u32::try_from((3_i32 - 8) & 0x00FF_FFFF).unwrap();

    let mut bytes = Vec::with_capacity(words.len() * 4 + 8);
    for word in words {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    // Starts at word index 9 -> byte address 36, loaded into R3.
    bytes.extend_from_slice(b"mosaic\n\0");
    bytes
}

/// Build and run the P1 machine once. Two calls must produce identical
/// reports, which the determinism test relies on.
pub fn run_bringup() -> Result<BringupReport, String> {
    let view = FlatView::build(&[
        leaf(RAM_BASE, RAM_SIZE, RAM, MemoryKind::Ram, Permissions::RWX),
        leaf(TIMER_BASE, PAGE, TIMER, MemoryKind::Mmio, Permissions::RW),
        leaf(UART_BASE, PAGE, UART, MemoryKind::Mmio, Permissions::RW),
    ])
    .map_err(|error| format!("layout: {error:?}"))?;

    // ADR 0008 wiring: the UART owns one contract endpoint, the harness the
    // other; neither side names a concrete model.
    let pair = UartPair::new(64);
    let (uart_peer, mut console) = pair.endpoints();

    let mut machine = Machine::new(view.clone(), 32);
    machine
        .add_backing(RAM, RAM_SIZE)
        .map_err(|error| format!("ram backing: {error:?}"))?;
    machine
        .register_device(
            UART,
            Box::new(Pl011::new(
                Pl011Params::new(UART_IRQ),
                Some(Box::new(uart_peer)),
            )),
        )
        .map_err(|error| format!("uart register: {error:?}"))?;
    machine
        .register_device(
            TIMER,
            Box::new(Sp804::new(Sp804Params::new(
                [TIMER_IRQ1, TIMER_IRQ2],
                TIMER_TICK_PS,
            ))),
        )
        .map_err(|error| format!("timer register: {error:?}"))?;

    let mailbox = Arc::new(IrqMailbox::new());
    machine
        .register_sink(
            IRQ_SINK,
            Box::new(EngineIrqSink::new(ARM_IRQ, Arc::clone(&mailbox))),
        )
        .map_err(|error| format!("sink register: {error:?}"))?;
    machine
        .connect(UART, UART_IRQ, IRQ_SINK)
        .map_err(|error| format!("uart wiring: {error:?}"))?;
    machine
        .connect(TIMER, TIMER_IRQ1, IRQ_SINK)
        .map_err(|error| format!("timer wiring: {error:?}"))?;
    machine
        .verify()
        .map_err(|error| format!("verify: {error:?}"))?;
    machine
        .loader_write(RAM, 0, &firmware())
        .map_err(|error| format!("load: {error:?}"))?;

    let mut engine = UnicornArm::new().map_err(|error| format!("engine: {error:?}"))?;
    let shared = bind_flat_view(&mut engine, &view, Arc::new(Mutex::new(machine)), CPU)
        .map_err(|error| format!("bind: {error:?}"))?;
    engine
        .reg_write(Register::Pc.into(), RAM_BASE)
        .map_err(|error| format!("pc: {error:?}"))?;
    engine
        .reg_write(Register::R2.into(), UART_BASE)
        .map_err(|error| format!("r2: {error:?}"))?;
    engine
        .reg_write(Register::R3.into(), 36)
        .map_err(|error| format!("r3: {error:?}"))?;

    let mut driver = MachineDriver::new(engine, shared);
    driver.attach_irq(Arc::clone(&mailbox));
    // 7 payload bytes need under 40 instructions; the remaining budget spins in
    // the firmware's final `B .` until the quantum is exhausted.
    let report = driver
        .step(NonZeroU64::new(64).ok_or("invalid quantum")?, 32)
        .map_err(|error| format!("step: {error:?}"))?;

    let mut buffer = vec![0u8; 16];
    let read = console
        .read_bytes(&mut CaptureCtx, &mut buffer)
        .map_err(|error| format!("console read: {error:?}"))?;
    buffer.truncate(read);
    Ok(BringupReport {
        output: buffer,
        hooks: report.engine.instruction_hooks,
        events: report.events_delivered,
        irq_edges: driver
            .engine()
            .irq_edges()
            .iter()
            .map(|(line, high)| (line.0, *high))
            .collect(),
    })
}

/// Expected golden console output for the P1 firmware.
pub const GOLDEN_OUTPUT: &[u8] = b"mosaic\n";
