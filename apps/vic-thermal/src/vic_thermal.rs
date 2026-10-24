//! P3 interrupt-controller slice: one ARM core, a PL190 VIC aggregating
//! peripheral interrupt lines, a DW-APB-I2C master and an LM75A temperature
//! sensor whose OS thermostat output crosses the VIC into the CPU IRQ line
//! (構成設計書 §13.3 / ADR 0009, P3 exit criterion).
//!
//! Topology (addresses borrow the Versatile/PB map for the VIC and UART;
//! the DW-APB-I2C keeps the P2 slice address):
//! - RAM 1 MiB at `0x0000_0000`
//! - DW-APB-I2C at `0x1000_2000` — master on the shared I2C bus
//! - PL190 VIC at `0x1014_0000`
//! - PL011 UART at `0x101F_1000` — console
//!
//! Signal wiring, all source-identified through the runtime's wiring table:
//! - UART combined IRQ → VIC line 1 (wired, left disabled by the firmware)
//! - DW-APB-I2C controller IRQ → VIC line 10
//! - LM75A OS pin → VIC line 12. The part emits the OS level through the
//!   initiator node's Ctx (see the LM75A crate docs); the composition root
//!   routes it as a wired output of the I2C node.
//! - VIC IRQ output → CPU IRQ line via the ADR 0009 mailbox
//!
//! The firmware is independent A32 machine code (every word disassembled and
//! verified with capstone): it enables the VIC lines, reads the LM75A
//! temperature register over the DW-APB-I2C, prints it, then reprograms the
//! LM75A thermostat (OS polarity active-HIGH, Tos = 24.5 °C against a 25.0 °C
//! scene) so the OS line rises. It observes the aggregation synchronously
//! through the VIC register file: IRQSTATUS `0x1400` (I2C + OS lines), the
//! IACK vector read resolving slot 0, and `0x1000` after clearing the I2C
//! interrupt. CPU exception entry needs an injection-capable backend
//! (ADR 0009), so the guest observes the VIC at register level while the
//! composition-level proof — the `(ARM_IRQ, high)` edge reaching the engine —
//! is asserted by the report.

use core::num::NonZeroU64;
use std::sync::{Arc, Mutex};

use mosaic_assemble::{bind_flat_view, EngineIrqSink, IrqMailbox, MachineDriver};
use mosaic_contract_cpu_engine::CpuEngine;
use mosaic_contract_i2c::{I2cAddr, I2cError, I2cInitiator, I2cSpeed};
use mosaic_contract_model::{ContextError, Ctx};
use mosaic_contract_stream::ByteStream;
use mosaic_core_runtime::{FlatView, Machine, Region};
use mosaic_core_types::{Addr, EventId, Mapping, MemoryKind, NodeId, Permissions, SignalId, VTime};
use mosaic_engine_unicorn::{UnicornArm, ARM_IRQ};
use mosaic_interconnect_i2c_bus::I2cBus;
use mosaic_interconnect_uart_pair::UartPair;
use mosaic_ip_arm_pl011::{Pl011, Pl011Params};
use mosaic_ip_arm_pl190::{Pl190, Pl190Params};
use mosaic_ip_synopsys_dw_apb_i2c::{DwApbI2c, DwApbI2cParams};
use mosaic_isa_arm::Register;
use mosaic_part_nxp_lm75a::{Lm75a, Lm75aParams, DEFAULT_SCENE};

/// Sysbus addresses of the P3 machine map.
pub const RAM_BASE: u64 = 0x0000_0000;
pub const RAM_SIZE: u64 = 0x0010_0000;
pub const I2C_BASE: u64 = 0x1000_2000;
pub const VIC_BASE: u64 = 0x1014_0000;
pub const UART_BASE: u64 = 0x101F_1000;
const PAGE: u64 = 4096;

/// Firmware steps: each step is one instruction quantum followed by the
/// event drain, so guest polling loops observe signal deliveries between
/// quanta (the runtime never re-enters a callback, ADR 0001).
const STEPS: usize = 64;
const QUANTUM: u64 = 32;

const CPU: NodeId = NodeId(0);
const RAM: NodeId = NodeId(1);
const UART: NodeId = NodeId(2);
const I2C: NodeId = NodeId(3);
const VIC: NodeId = NodeId(4);
const IRQ_SINK: NodeId = NodeId(5);

const UART_IRQ: SignalId = SignalId(1);
const I2C_IRQ: SignalId = SignalId(2);
/// LM75A OS level, routed through the I2C initiator node's signal space.
const LM75A_OS: SignalId = SignalId(5);
const VIC_IRQ: SignalId = SignalId(1);
const VIC_FIQ: SignalId = SignalId(2);

/// LM75A strap: A0-A2 tied low (7-bit address 0x48).
const LM75A_ADDRESS: u8 = 0x48;

/// Observed result of one full firmware run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VicThermalReport {
    /// Bytes captured on the console end of the UART pair.
    pub output: Vec<u8>,
    /// Retired instruction hooks reported by the engine.
    pub hooks: u64,
    /// Runtime events delivered during the run.
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

/// The bus entity is shared between the runtime device and any other access
/// path, so the controller binds a clone of the handle (P2 pattern).
struct SharedInitiator(Arc<Mutex<I2cBus>>);

impl I2cInitiator for SharedInitiator {
    fn set_speed(&mut self, speed: I2cSpeed) {
        self.0.lock().expect("i2c bus lock").set_speed(speed);
    }
    fn write(&mut self, ctx: &mut dyn Ctx, addr: I2cAddr, bytes: &[u8]) -> Result<(), I2cError> {
        self.0.lock().expect("i2c bus lock").write(ctx, addr, bytes)
    }
    fn read(&mut self, ctx: &mut dyn Ctx, addr: I2cAddr, buf: &mut [u8]) -> Result<(), I2cError> {
        self.0.lock().expect("i2c bus lock").read(ctx, addr, buf)
    }
    fn write_read(
        &mut self,
        ctx: &mut dyn Ctx,
        addr: I2cAddr,
        out: &[u8],
        input: &mut [u8],
    ) -> Result<(), I2cError> {
        self.0
            .lock()
            .expect("i2c bus lock")
            .write_read(ctx, addr, out, input)
    }
}

/// Independent A32 machine code, hand-encoded and verified word by word with
/// capstone (ARM mode). R8 = UART base, R9 = VIC base, R10 = I2C base;
/// the harness preloads all three plus PC = 0.
///
/// Flow: enable UART TX; TAR/ENABLE/INTR_MASK on the I2C controller; VIC
/// INTENABLE for lines 10+12, slot 0 = line 12 with vector `0x00BEEF00`;
/// one I2C batch (write pointer=Temp, read 2) whose result is printed as
/// `t=`; one batch writing Conf=OS_POL and Tos=24.5 °C so the OS line rises
/// through the VIC (`v=00001400` after the polling loop); IACK `k=00BEEF00`
/// plus EOI; CLR_INTR on the controller dropping line 10 (`c=00001000`).
static FIRMWARE_WORDS: [u32; 92] = [
    0xE3A01001, 0xE3811C01, 0xE5881030, 0xE3A00048, 0xE58A0004, 0xE3A00001, 0xE58A006C, 0xE3A00C02,
    0xE58A0030, 0xE3A00B05, 0xE5890010, 0xE3A00CEF, 0xE38008BE, 0xE5890100, 0xE3A0002C, 0xE5890200,
    0xE3A00000, 0xE58A0010, 0xE3A00C01, 0xE58A0010, 0xE3A00C03, 0xE58A0010, 0xE59A4010, 0xE59A5010,
    0xE1856404, 0xE59F00EC, 0xEB000026, 0xE1A00006, 0xEB00002A, 0xE59F00E0, 0xEB000022, 0xE3A00001,
    0xE58A0010, 0xE3A00F81, 0xE58A0010, 0xE3A00003, 0xE58A0010, 0xE3A00018, 0xE58A0010, 0xE3A00D0A,
    0xE58A0010, 0xE59F00B4, 0xEB000016, 0xE5990000, 0xE3100A01, 0x0AFFFFFC, 0xEB000018, 0xE59F0098,
    0xEB000010, 0xE59F0098, 0xEB00000E, 0xE5990030, 0xEB000012, 0xE5890030, 0xE59F007C, 0xEB000009,
    0xE59F0080, 0xEB000007, 0xE59A0040, 0xE5990000, 0xE3100B01, 0x1AFFFFFC, 0xEB000008, 0xE59F0068,
    0xEB000000, 0xEAFFFFFE, 0xE4D03001, 0xE3530000, 0x0A000001, 0xE5883000, 0xEAFFFFFA, 0xE1A0F00E,
    0xE3A0201C, 0xE1A03230, 0xE203300F, 0xE353000A, 0xB2833030, 0xA2833037, 0xE5883000, 0xE2522004,
    0x5AFFFFF7, 0xE1A0F00E, 0x20003D74, 0x003D7600, 0x63003D6B, 0x000A003D, 0x00000148, 0x0000014B,
    0x0000014D, 0x00000150, 0x00000153, 0x00000156,
];

fn firmware() -> Vec<u8> {
    let mut bytes = Vec::with_capacity(FIRMWARE_WORDS.len() * 4);
    for word in FIRMWARE_WORDS {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    bytes
}

/// The composed P3 machine plus the harness-owned ends of its contracts.
struct P3 {
    view: FlatView,
    machine: Machine,
    mailbox: Arc<IrqMailbox>,
    console: mosaic_interconnect_uart_pair::UartEndpoint,
}

/// Compose the P3 topology. Shared by the engine-driven run and the
/// engine-free device-chain replay test.
fn build_p3() -> Result<P3, String> {
    let view = FlatView::build(&[
        leaf(RAM_BASE, RAM_SIZE, RAM, MemoryKind::Ram, Permissions::RWX),
        leaf(I2C_BASE, PAGE, I2C, MemoryKind::Mmio, Permissions::RW),
        leaf(VIC_BASE, PAGE, VIC, MemoryKind::Mmio, Permissions::RW),
        leaf(UART_BASE, PAGE, UART, MemoryKind::Mmio, Permissions::RW),
    ])
    .map_err(|error| format!("layout: {error:?}"))?;

    // Shared I2C bus with the LM75A at 0x48. The OS pin level rides the
    // initiator node's signal space (LM75A crate contract).
    let mut bus = I2cBus::new();
    bus.add_target(Box::new(Lm75a::new(
        Lm75aParams::new(I2cAddr::seven(LM75A_ADDRESS).map_err(|e| format!("addr: {e:?}"))?)
            .with_os(LM75A_OS)
            .with_scene(DEFAULT_SCENE),
    )));
    let bus = bus.shared();

    // ADR 0008 wiring: the UART owns one contract endpoint, the harness the
    // other; neither side names a concrete model.
    let pair = UartPair::new(64);
    let (uart_peer, console) = pair.endpoints();

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
            I2C,
            Box::new(DwApbI2c::new(
                DwApbI2cParams::new(I2C_IRQ),
                Box::new(SharedInitiator(Arc::clone(&bus))),
            )),
        )
        .map_err(|error| format!("i2c register: {error:?}"))?;
    // One PL190 instance, two faces: MMIO register file for the guest and
    // signal aggregator for the wired input lines (§13.3/§13.4 rule 1).
    let mut vic = Pl190::new(Pl190Params::new(VIC_IRQ, VIC_FIQ));
    vic.bind_input(1, UART, UART_IRQ)
        .map_err(|error| format!("vic uart line: {error:?}"))?;
    vic.bind_input(10, I2C, I2C_IRQ)
        .map_err(|error| format!("vic i2c line: {error:?}"))?;
    vic.bind_input(12, I2C, LM75A_OS)
        .map_err(|error| format!("vic os line: {error:?}"))?;
    machine
        .register_device(VIC, Box::new(vic))
        .map_err(|error| format!("vic register: {error:?}"))?;

    let mailbox = Arc::new(IrqMailbox::new());
    machine
        .register_sink(
            IRQ_SINK,
            Box::new(EngineIrqSink::new(ARM_IRQ, Arc::clone(&mailbox))),
        )
        .map_err(|error| format!("sink register: {error:?}"))?;
    // Peripheral lines terminate in the VIC; the VIC's aggregated output is
    // the only edge that reaches the CPU line.
    machine
        .connect(UART, UART_IRQ, VIC)
        .map_err(|error| format!("uart wiring: {error:?}"))?;
    machine
        .connect(I2C, I2C_IRQ, VIC)
        .map_err(|error| format!("i2c wiring: {error:?}"))?;
    machine
        .connect(I2C, LM75A_OS, VIC)
        .map_err(|error| format!("os wiring: {error:?}"))?;
    machine
        .connect(VIC, VIC_IRQ, IRQ_SINK)
        .map_err(|error| format!("vic wiring: {error:?}"))?;
    machine
        .verify()
        .map_err(|error| format!("verify: {error:?}"))?;

    Ok(P3 {
        view,
        machine,
        mailbox,
        console,
    })
}

/// Build and run the P3 machine once. Two calls must produce identical
/// reports, which the determinism check in `main` relies on.
pub fn run_vic_thermal() -> Result<VicThermalReport, String> {
    let P3 {
        view,
        mut machine,
        mailbox,
        mut console,
    } = build_p3()?;
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
        .reg_write(Register::R8.into(), UART_BASE)
        .map_err(|error| format!("r8: {error:?}"))?;
    engine
        .reg_write(Register::R9.into(), VIC_BASE)
        .map_err(|error| format!("r9: {error:?}"))?;
    engine
        .reg_write(Register::R10.into(), I2C_BASE)
        .map_err(|error| format!("r10: {error:?}"))?;

    let mut driver = MachineDriver::new(engine, shared);
    driver.attach_irq(Arc::clone(&mailbox));
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

    let mut buffer = vec![0u8; 64];
    let read = console
        .read_bytes(&mut CaptureCtx, &mut buffer)
        .map_err(|error| format!("console read: {error:?}"))?;
    buffer.truncate(read);
    Ok(VicThermalReport {
        output: buffer,
        hooks,
        events,
        irq_edges: driver
            .engine()
            .irq_edges()
            .iter()
            .map(|(line, high)| (line.0, *high))
            .collect(),
    })
}

/// Expected golden console output for the P3 firmware: the LM75A temperature
/// register (25.0 °C as 0x1900), the aggregated VIC status (I2C line 10 plus
/// the thermostat OS line 12), the IACK vector read, and the status after the
/// I2C interrupt clear leaves only the OS line.
pub const GOLDEN_OUTPUT: &[u8] = b"t=00001900 v=00001400 k=00BEEF00 c=00001000\n";

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
    /// wiring: the device chain (DW-APB-I2C -> LM75A -> OS line -> VIC ->
    /// CPU line mailbox) must aggregate exactly as the golden output claims.
    /// The A32 encoding is capstone-verified and the guest-level golden runs
    /// in CI; this test pins the register-level behaviour without a backend.
    #[test]
    fn device_chain_aggregates_lm75a_os_through_the_vic() {
        let P3 {
            mut machine,
            mailbox,
            ..
        } = build_p3().expect("composition");
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

        // Firmware words 3..15: TAR, ENABLE, INTR_MASK, VIC INTENABLE and
        // vector slot 0 for line 12.
        wr(&mut machine, I2C_BASE + 0x004, 0x48);
        wr(&mut machine, I2C_BASE + 0x06C, 1);
        wr(&mut machine, I2C_BASE + 0x030, 0x200);
        wr(&mut machine, VIC_BASE + 0x010, 0x1400);
        wr(&mut machine, VIC_BASE + 0x100, 0x00BE_EF00);
        wr(&mut machine, VIC_BASE + 0x200, 0x2C);

        // Transaction A: write pointer=Temp, read 2 bytes.
        wr(&mut machine, I2C_BASE + 0x010, 0x000);
        wr(&mut machine, I2C_BASE + 0x010, 0x100);
        wr(&mut machine, I2C_BASE + 0x010, 0x300);
        let msb = rd(&mut machine, I2C_BASE + 0x010);
        let lsb = rd(&mut machine, I2C_BASE + 0x010);
        assert_eq!((msb << 8) | lsb, 0x1900); // 25.0C as the Temp register.

        // After the step boundary the controller STOP_DET line is aggregated.
        drain(&mut machine);
        assert_eq!(rd(&mut machine, VIC_BASE), 0x0400);
        assert_eq!(mailbox.len(), 1); // VIC IRQ output reached the CPU line.

        // Transaction B: OS_POL active-HIGH, Tos = 24.5C -> OS line rises.
        wr(&mut machine, I2C_BASE + 0x010, 0x001);
        wr(&mut machine, I2C_BASE + 0x010, 0x204);
        wr(&mut machine, I2C_BASE + 0x010, 0x003);
        wr(&mut machine, I2C_BASE + 0x010, 0x018);
        wr(&mut machine, I2C_BASE + 0x010, 0x280);
        drain(&mut machine);
        assert_eq!(rd(&mut machine, VIC_BASE), 0x1400);

        // IACK resolves slot 0 (line 12, highest priority), EOI pops it.
        assert_eq!(rd(&mut machine, VIC_BASE + 0x030), 0x00BE_EF00);
        wr(&mut machine, VIC_BASE + 0x030, 0);

        // Clearing the controller interrupt drops line 10 only; the OS line
        // stays asserted while 25.0C > 24.5C.
        let _ = rd(&mut machine, I2C_BASE + 0x040);
        drain(&mut machine);
        assert_eq!(rd(&mut machine, VIC_BASE), 0x1000);
        // One rising VIC->CPU edge in total; the line never dropped.
        assert_eq!(mailbox.len(), 1);
    }
}
