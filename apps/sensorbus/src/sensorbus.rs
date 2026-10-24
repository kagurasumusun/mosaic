//! P2 bus/pin slice: one ARM core driving the same sensor part over three
//! paths — DW-APB-I2C controller, PL061 bit-bang through the pin decoder,
//! and PL022 SPI (design §13 P2 exit, ADR 0008 composition).
//!
//! Topology (addresses mirror the P2 machine map, not a vendor board):
//! - RAM 1 MiB at `0x0000_0000`
//! - DW-APB-I2C at `0x1000_2000` — master on the shared I2C bus
//! - PL022 SSP at `0x1000_D000` — SPI master, BME280-B on chip-select 0
//! - PL061 GPIO at `0x101E_4000` — pins 0/1 are SDA/SCL bit-bang lines
//! - PL011 UART at `0x101F_1000` — console
//!
//! The shared I2C bus carries BME280-A at `0x76` and a TCA9548A at `0x70`
//! with BME280-C at `0x77` behind channel 3. The pin decoder wraps the very
//! same bus instance, so the bit-bang path and the controller path mutate one
//! shared part — the P2 exit criterion. Both SDA and SCL hang off pull-up
//! nets; the decoder observes them through sinks while the firmware drives
//! them through [`BitBangPort`] adapters that also feed target-driven ACK and
//! read-data levels back to the master.
//!
//! The firmware is independent A32 machine code (see `firmware.S` in this
//! directory for the verified listing): it reads the BME280-A chip id via
//! the DW controller, bit-bangs `ctrl_meas = 0x25` plus a chip-id read to
//! the same part, exchanges the chip-id command with BME280-B over the
//! PL022, then prints `ctl=60 bb=60 spi=60\n` on the PL011 and spins.

use core::num::NonZeroU64;
use std::sync::{Arc, Mutex, MutexGuard};

use mosaic_assemble::{bind_flat_view, EngineIrqSink, IrqMailbox, MachineDriver};
use mosaic_contract_cpu_engine::{CpuEngine, StopReason};
use mosaic_contract_gpio::{PinConfig, PinLevel};
use mosaic_contract_i2c::{I2cAddr, I2cError, I2cInitiator, I2cSpeed};
use mosaic_contract_model::{ContextError, Ctx};
use mosaic_contract_net::{DigitalNet, NetPull};
use mosaic_contract_spi::SpiCs;
use mosaic_contract_stream::ByteStream;
use mosaic_core_runtime::{FlatView, Machine, Region};
use mosaic_core_types::{Addr, EventId, Mapping, MemoryKind, NodeId, Permissions, SignalId, VTime};
use mosaic_engine_unicorn::{UnicornArm, ARM_IRQ};
use mosaic_interconnect_gpio_net::GpioNet;
use mosaic_interconnect_i2c_bus::{I2cBitBang, I2cBus};
use mosaic_interconnect_spi_bus::SpiBus;
use mosaic_interconnect_uart_pair::UartPair;
use mosaic_ip_arm_pl011::{Pl011, Pl011Params};
use mosaic_ip_arm_pl022::{Pl022, Pl022Params};
use mosaic_ip_arm_pl061::{Pl061, Pl061Params};
use mosaic_ip_synopsys_dw_apb_i2c::{DwApbI2c, DwApbI2cParams};
use mosaic_isa_arm::Register;
use mosaic_part_bosch_bme280::{
    bind_i2c::{Bme280I2c, Bme280I2cParams},
    bind_spi::Bme280Spi,
    reg,
};
use mosaic_part_ti_tca9548a::{Tca9548a, Tca9548aParams};

/// Sysbus addresses of the P2 machine map.
pub const RAM_BASE: u64 = 0x0000_0000;
pub const RAM_SIZE: u64 = 0x0010_0000;
pub const I2C_BASE: u64 = 0x1000_2000;
pub const SSP_BASE: u64 = 0x1000_D000;
pub const GPIO_BASE: u64 = 0x101E_4000;
pub const UART_BASE: u64 = 0x101F_1000;
const PAGE: u64 = 4096;

/// Firmware stack top: the print helpers push LR on the RAM-backed stack.
const STACK_TOP: u64 = RAM_BASE + RAM_SIZE;

const CPU: NodeId = NodeId(0);
const RAM: NodeId = NodeId(1);
const UART: NodeId = NodeId(2);
const I2C: NodeId = NodeId(3);
const SSP: NodeId = NodeId(4);
const GPIO: NodeId = NodeId(5);
const IRQ_SINK: NodeId = NodeId(6);

const UART_IRQ: SignalId = SignalId(1);
const I2C_IRQ: SignalId = SignalId(2);
const SSP_IRQ: SignalId = SignalId(3);
const GPIO_IRQ: SignalId = SignalId(4);

/// BME280-A: reachable over the controller path and the bit-bang path.
const SENSOR_A: u8 = 0x76;
/// TCA9548A mux and the BME280-C hidden behind channel 3.
const MUX_ADDR: u8 = 0x70;
const MUX_CHANNEL: u8 = 3;
const SENSOR_C: u8 = 0x77;

/// One engine slice; the firmware needs ~460 instructions and spins out the
/// rest of the quantum.
const QUANTUM: u64 = 1024;
const EVENT_BUDGET: usize = 32;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

fn seven(value: u8) -> Result<I2cAddr, String> {
    I2cAddr::seven(value).map_err(|error| format!("address {value:#04x}: {error:?}"))
}

/// Observed result of one full firmware run plus harness-side probes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SensorbusReport {
    /// Bytes captured on the console end of the UART pair.
    pub output: Vec<u8>,
    /// Retired instruction hooks reported by the engine.
    pub hooks: u64,
    /// Runtime events delivered during the step.
    pub events: usize,
    /// `(line, high)` edges that reached the engine, in delivery order.
    pub irq_edges: Vec<(u32, bool)>,
    /// Why the engine slice ended; the golden run exhausts the quantum.
    pub reason: StopReason,
    /// ctrl_meas of BME280-A read back over the transaction path after the
    /// run: the bit-bang frame wrote `0x25` into the controller's sensor.
    pub shared_ctrl_meas: u8,
    /// True while BME280-C stays hidden behind the disabled mux.
    pub mux_hidden_nack: bool,
    /// BME280-C chip id after enabling mux channel 3.
    pub mux_routed_id: u8,
    /// Phases recorded by the transaction bus and by the pin decoder.
    pub bus_phases: usize,
    pub pin_phases: usize,
}

/// Minimal context for harness-side probes: no events, no signals.
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

/// Initiator adapter handing the shared bus to the DW controller. Lock order
/// is bus-only here; the decoder takes decoder-then-bus and nothing takes
/// bus-then-decoder, so the two paths cannot deadlock.
struct SharedInitiator(Arc<Mutex<I2cBus>>);

impl I2cInitiator for SharedInitiator {
    fn set_speed(&mut self, speed: I2cSpeed) {
        lock(&self.0).set_speed(speed);
    }
    fn write(&mut self, ctx: &mut dyn Ctx, addr: I2cAddr, bytes: &[u8]) -> Result<(), I2cError> {
        lock(&self.0).write(ctx, addr, bytes)
    }
    fn read(&mut self, ctx: &mut dyn Ctx, addr: I2cAddr, buf: &mut [u8]) -> Result<(), I2cError> {
        lock(&self.0).read(ctx, addr, buf)
    }
    fn write_read(
        &mut self,
        ctx: &mut dyn Ctx,
        addr: I2cAddr,
        out: &[u8],
        input: &mut [u8],
    ) -> Result<(), I2cError> {
        lock(&self.0).write_read(ctx, addr, out, input)
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

/// Independent A32 machine code, not a vendor firmware image. Assembled
/// offline with keystone and verified by capstone disassembly plus a full
/// peripheral simulation; `firmware.S` holds the matching listing and the
/// final four words are the literal pool (I2C, GPIO, SSP, UART bases).
static FIRMWARE_WORDS: [u32; 461] = [
    0xE59F271C, 0xE3A00001, 0xE582006C, 0xE3A00076, 0xE5820004, 0xE3A000D0, 0xE5820010, 0xE3A00C07,
    0xE5820010, 0xE5920070, 0xE3100008, 0x0AFFFFFC, 0xE5924010, 0xE3A00000, 0xE582006C, 0xE59F26E4,
    0xE3A000FF, 0xE5820400, 0xE3A00002, 0xE5820008, 0xE3A00000, 0xE582000C, 0xE3A00001, 0xE5820004,
    0xE3A00002, 0xE5820008, 0xE3A00000, 0xE5820008, 0xE3A00002, 0xE5820008, 0xE3A00000, 0xE5820008,
    0xE3A00002, 0xE5820008, 0xE3A00000, 0xE5820008, 0xE3A00000, 0xE5820004, 0xE3A00002, 0xE5820008,
    0xE3A00000, 0xE5820008, 0xE3A00001, 0xE5820004, 0xE3A00002, 0xE5820008, 0xE3A00000, 0xE5820008,
    0xE3A00002, 0xE5820008, 0xE3A00000, 0xE5820008, 0xE3A00000, 0xE5820004, 0xE3A00002, 0xE5820008,
    0xE3A00000, 0xE5820008, 0xE3A00002, 0xE5820008, 0xE3A00000, 0xE5820008, 0xE3A00001, 0xE5820004,
    0xE3A00002, 0xE5820008, 0xE3A00000, 0xE5820008, 0xE3A00002, 0xE5820008, 0xE3A00000, 0xE5820008,
    0xE3A00002, 0xE5820008, 0xE3A00000, 0xE5820008, 0xE3A00002, 0xE5820008, 0xE3A00000, 0xE5820008,
    0xE3A00002, 0xE5820008, 0xE3A00000, 0xE5820008, 0xE3A00000, 0xE5820004, 0xE3A00002, 0xE5820008,
    0xE3A00000, 0xE5820008, 0xE3A00001, 0xE5820004, 0xE3A00002, 0xE5820008, 0xE3A00000, 0xE5820008,
    0xE3A00000, 0xE5820004, 0xE3A00002, 0xE5820008, 0xE3A00000, 0xE5820008, 0xE3A00002, 0xE5820008,
    0xE3A00000, 0xE5820008, 0xE3A00001, 0xE5820004, 0xE3A00002, 0xE5820008, 0xE3A00000, 0xE5820008,
    0xE3A00000, 0xE5820004, 0xE3A00002, 0xE5820008, 0xE3A00000, 0xE5820008, 0xE3A00002, 0xE5820008,
    0xE3A00000, 0xE5820008, 0xE3A00001, 0xE5820004, 0xE3A00002, 0xE5820008, 0xE3A00000, 0xE5820008,
    0xE3A00000, 0xE5820004, 0xE3A00002, 0xE5820008, 0xE3A00000, 0xE5820008, 0xE3A00002, 0xE5820008,
    0xE3A00000, 0xE5820008, 0xE3A00001, 0xE5820004, 0xE3A00002, 0xE5820008, 0xE3A00000, 0xE5820008,
    0xE3A00000, 0xE5820004, 0xE3A00002, 0xE5820008, 0xE3A00000, 0xE5820008, 0xE3A00001, 0xE5820004,
    0xE3A00002, 0xE5820008, 0xE3A00000, 0xE5820008, 0xE3A00002, 0xE5820008, 0xE3A00000, 0xE5820008,
    0xE3A00000, 0xE5820004, 0xE3A00002, 0xE5820008, 0xE3A00003, 0xE582000C, 0xE3A00002, 0xE5820008,
    0xE3A00000, 0xE582000C, 0xE3A00001, 0xE5820004, 0xE3A00002, 0xE5820008, 0xE3A00000, 0xE5820008,
    0xE3A00002, 0xE5820008, 0xE3A00000, 0xE5820008, 0xE3A00002, 0xE5820008, 0xE3A00000, 0xE5820008,
    0xE3A00000, 0xE5820004, 0xE3A00002, 0xE5820008, 0xE3A00000, 0xE5820008, 0xE3A00001, 0xE5820004,
    0xE3A00002, 0xE5820008, 0xE3A00000, 0xE5820008, 0xE3A00002, 0xE5820008, 0xE3A00000, 0xE5820008,
    0xE3A00000, 0xE5820004, 0xE3A00002, 0xE5820008, 0xE3A00000, 0xE5820008, 0xE3A00002, 0xE5820008,
    0xE3A00000, 0xE5820008, 0xE3A00001, 0xE5820004, 0xE3A00002, 0xE5820008, 0xE3A00000, 0xE5820008,
    0xE3A00002, 0xE5820008, 0xE3A00000, 0xE5820008, 0xE3A00002, 0xE5820008, 0xE3A00000, 0xE5820008,
    0xE3A00000, 0xE5820004, 0xE3A00002, 0xE5820008, 0xE3A00000, 0xE5820008, 0xE3A00001, 0xE5820004,
    0xE3A00002, 0xE5820008, 0xE3A00000, 0xE5820008, 0xE3A00000, 0xE5820004, 0xE3A00002, 0xE5820008,
    0xE3A00000, 0xE5820008, 0xE3A00002, 0xE5820008, 0xE3A00000, 0xE5820008, 0xE3A00002, 0xE5820008,
    0xE3A00000, 0xE5820008, 0xE3A00002, 0xE5820008, 0xE3A00000, 0xE5820008, 0xE3A00001, 0xE5820004,
    0xE3A00002, 0xE5820008, 0xE3A00000, 0xE5820008, 0xE3A00000, 0xE5820004, 0xE3A00002, 0xE5820008,
    0xE3A00003, 0xE582000C, 0xE3A00002, 0xE5820008, 0xE3A00000, 0xE582000C, 0xE3A00001, 0xE5820004,
    0xE3A00002, 0xE5820008, 0xE3A00000, 0xE5820008, 0xE3A00002, 0xE5820008, 0xE3A00000, 0xE5820008,
    0xE3A00002, 0xE5820008, 0xE3A00000, 0xE5820008, 0xE3A00000, 0xE5820004, 0xE3A00002, 0xE5820008,
    0xE3A00000, 0xE5820008, 0xE3A00001, 0xE5820004, 0xE3A00002, 0xE5820008, 0xE3A00000, 0xE5820008,
    0xE3A00002, 0xE5820008, 0xE3A00000, 0xE5820008, 0xE3A00000, 0xE5820004, 0xE3A00002, 0xE5820008,
    0xE3A00000, 0xE5820008, 0xE3A00001, 0xE5820004, 0xE3A00002, 0xE5820008, 0xE3A00000, 0xE5820008,
    0xE3A00002, 0xE5820008, 0xE3A00000, 0xE5820008, 0xE3A05000, 0xE1A05085, 0xE59203FC, 0xE2000001,
    0xE1855000, 0xE3A00002, 0xE5820008, 0xE3A00000, 0xE5820008, 0xE1A05085, 0xE59203FC, 0xE2000001,
    0xE1855000, 0xE3A00002, 0xE5820008, 0xE3A00000, 0xE5820008, 0xE1A05085, 0xE59203FC, 0xE2000001,
    0xE1855000, 0xE3A00002, 0xE5820008, 0xE3A00000, 0xE5820008, 0xE1A05085, 0xE59203FC, 0xE2000001,
    0xE1855000, 0xE3A00002, 0xE5820008, 0xE3A00000, 0xE5820008, 0xE1A05085, 0xE59203FC, 0xE2000001,
    0xE1855000, 0xE3A00002, 0xE5820008, 0xE3A00000, 0xE5820008, 0xE1A05085, 0xE59203FC, 0xE2000001,
    0xE1855000, 0xE3A00002, 0xE5820008, 0xE3A00000, 0xE5820008, 0xE1A05085, 0xE59203FC, 0xE2000001,
    0xE1855000, 0xE3A00002, 0xE5820008, 0xE3A00000, 0xE5820008, 0xE1A05085, 0xE59203FC, 0xE2000001,
    0xE1855000, 0xE3A00002, 0xE5820008, 0xE3A00000, 0xE5820008, 0xE3A00002, 0xE5820008, 0xE3A00000,
    0xE5820008, 0xE3A00000, 0xE5820004, 0xE3A00002, 0xE5820008, 0xE3A00003, 0xE582000C, 0xE1A07005,
    0xE59F2104, 0xE3A00007, 0xE5820000, 0xE3A00002, 0xE5820004, 0xE3A000D0, 0xE5820008, 0xE3A00000,
    0xE5820008, 0xE5920008, 0xE5926008, 0xE3A00000, 0xE5820004, 0xE59F20D4, 0xE3A00001, 0xE3800C01,
    0xE5820030, 0xE3A00063, 0xEB000021, 0xE3A00074, 0xEB00001F, 0xE3A0006C, 0xEB00001D, 0xE3A0003D,
    0xEB00001B, 0xE1A05004, 0xEB00001B, 0xE3A00020, 0xEB000017, 0xE3A00062, 0xEB000015, 0xE3A00062,
    0xEB000013, 0xE3A0003D, 0xEB000011, 0xE1A05007, 0xEB000011, 0xE3A00020, 0xEB00000D, 0xE3A00073,
    0xEB00000B, 0xE3A00070, 0xEB000009, 0xE3A00069, 0xEB000007, 0xE3A0003D, 0xEB000005, 0xE1A05006,
    0xEB000005, 0xE3A0000A, 0xEB000001, 0xEAFFFFFF, 0xEAFFFFFE, 0xE5820000, 0xE12FFF1E, 0xE52DE004,
    0xE1A00225, 0xEB000002, 0xE205000F, 0xEB000000, 0xE49DF004, 0xE350000A, 0xB2800030, 0xA2800057,
    0xEAFFFFF3, 0x10002000, 0x101E4000, 0x1000D000, 0x101F1000,
];

fn firmware() -> Vec<u8> {
    let mut bytes = Vec::with_capacity(FIRMWARE_WORDS.len() * 4);
    for word in FIRMWARE_WORDS {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    bytes
}

/// Build and run the P2 machine once. Two calls must produce identical
/// reports, which the determinism test relies on.
pub fn run_sensorbus() -> Result<SensorbusReport, String> {
    let view = FlatView::build(&[
        leaf(RAM_BASE, RAM_SIZE, RAM, MemoryKind::Ram, Permissions::RWX),
        leaf(I2C_BASE, PAGE, I2C, MemoryKind::Mmio, Permissions::RW),
        leaf(SSP_BASE, PAGE, SSP, MemoryKind::Mmio, Permissions::RW),
        leaf(GPIO_BASE, PAGE, GPIO, MemoryKind::Mmio, Permissions::RW),
        leaf(UART_BASE, PAGE, UART, MemoryKind::Mmio, Permissions::RW),
    ])
    .map_err(|error| format!("layout: {error:?}"))?;

    // Shared I2C bus: BME280-A plus the mux with BME280-C on channel 3.
    let mut bus = I2cBus::new();
    bus.add_target(Box::new(Bme280I2c::new(Bme280I2cParams::new(seven(
        SENSOR_A,
    )?))));
    let mut mux = Tca9548a::new(Tca9548aParams::new(seven(MUX_ADDR)?));
    mux.attach(
        MUX_CHANNEL,
        Box::new(Bme280I2c::new(Bme280I2cParams::new(seven(SENSOR_C)?))),
    )
    .map_err(|error| format!("mux attach: {error:?}"))?;
    bus.add_target(Box::new(mux));
    let bus = bus.shared();

    // The pin decoder wraps the very same bus instance: one shared part,
    // two access paths (P2 exit criterion).
    let decoder = Arc::new(Mutex::new(I2cBitBang::new(Arc::clone(&bus))));

    // One GpioNet per wire (a net is a single electrical node): SDA and SCL
    // get separate pull-up nets, each observed by its decoder line sink.
    let mut sda_net = GpioNet::new();
    let sda_node = sda_net.attach(NetPull::Up);
    let mut scl_net = GpioNet::new();
    let scl_node = scl_net.attach(NetPull::Up);
    let (sda_line, scl_line) = I2cBitBang::lines(Arc::clone(&decoder));
    sda_net
        .attach_sink(sda_node, Box::new(sda_line))
        .map_err(|error| format!("sda sink: {error:?}"))?;
    scl_net
        .attach_sink(scl_node, Box::new(scl_line))
        .map_err(|error| format!("scl sink: {error:?}"))?;
    // Master-facing ports wrap the net ports so target-driven ACK and read
    // data become observable to the firmware without corrupting the net.
    let sda_port = I2cBitBang::sda_port(
        Arc::clone(&decoder),
        Box::new(sda_net.port(sda_node, PinConfig::output(PinLevel::High))),
    );
    let scl_port = I2cBitBang::scl_port(
        Arc::clone(&decoder),
        Box::new(scl_net.port(scl_node, PinConfig::output(PinLevel::High))),
    );

    let mut gpio = Pl061::new(Pl061Params::new(GPIO_IRQ));
    gpio.attach_pin(0, Box::new(sda_port))
        .map_err(|error| format!("sda pin: {error:?}"))?;
    gpio.attach_pin(1, Box::new(scl_port))
        .map_err(|error| format!("scl pin: {error:?}"))?;

    // SPI bus: BME280-B behind PL022 chip-select 0.
    let mut spi = SpiBus::new();
    spi.add_target(SpiCs(0), Box::new(Bme280Spi::new()));

    // ADR 0008 wiring: every model meets the others only through contracts.
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
            I2C,
            Box::new(DwApbI2c::new(
                DwApbI2cParams::new(I2C_IRQ),
                Box::new(SharedInitiator(Arc::clone(&bus))),
            )),
        )
        .map_err(|error| format!("i2c register: {error:?}"))?;
    machine
        .register_device(
            SSP,
            Box::new(Pl022::new(Pl022Params::new(SSP_IRQ), Some(Box::new(spi)))),
        )
        .map_err(|error| format!("ssp register: {error:?}"))?;
    machine
        .register_device(GPIO, Box::new(gpio))
        .map_err(|error| format!("gpio register: {error:?}"))?;

    let mailbox = Arc::new(IrqMailbox::new());
    machine
        .register_sink(
            IRQ_SINK,
            Box::new(EngineIrqSink::new(ARM_IRQ, Arc::clone(&mailbox))),
        )
        .map_err(|error| format!("sink register: {error:?}"))?;
    for (device, signal) in [
        (UART, UART_IRQ),
        (I2C, I2C_IRQ),
        (SSP, SSP_IRQ),
        (GPIO, GPIO_IRQ),
    ] {
        machine
            .connect(device, signal, IRQ_SINK)
            .map_err(|error| format!("irq wiring {signal:?}: {error:?}"))?;
    }
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
        .reg_write(Register::Sp.into(), STACK_TOP)
        .map_err(|error| format!("sp: {error:?}"))?;

    let mut driver = MachineDriver::new(engine, shared);
    driver.attach_irq(Arc::clone(&mailbox));
    let report = driver
        .step(
            NonZeroU64::new(QUANTUM).ok_or("invalid quantum")?,
            EVENT_BUDGET,
        )
        .map_err(|error| format!("step: {error:?}"))?;
    let reason = report.engine.reason;
    let hooks = report.engine.instruction_hooks;
    let events = report.events_delivered;

    let mut buffer = vec![0u8; 64];
    let read = console
        .read_bytes(&mut CaptureCtx, &mut buffer)
        .map_err(|error| format!("console read: {error:?}"))?;
    buffer.truncate(read);

    // Harness-side probes over the shared bus (P2 exit evidence).
    let mut ctx = CaptureCtx;
    let shared_ctrl_meas = {
        let mut probe = [0u8; 1];
        lock(&bus)
            .write_read(&mut ctx, seven(SENSOR_A)?, &[reg::CTRL_MEAS], &mut probe)
            .map_err(|error| format!("ctrl_meas probe: {error:?}"))?;
        probe[0]
    };
    let mut hidden = [0u8; 1];
    let mux_hidden_nack = matches!(
        lock(&bus).read(&mut ctx, seven(SENSOR_C)?, &mut hidden),
        Err(I2cError::Nack)
    );
    let mux_routed_id = {
        let mut id = [0u8; 1];
        let mut guard = lock(&bus);
        guard
            .write(&mut ctx, seven(MUX_ADDR)?, &[1 << MUX_CHANNEL])
            .map_err(|error| format!("mux enable: {error:?}"))?;
        guard
            .write_read(&mut ctx, seven(SENSOR_C)?, &[reg::CHIP_ID], &mut id)
            .map_err(|error| format!("mux routed read: {error:?}"))?;
        id[0]
    };
    let bus_phases = lock(&bus).phases().len();
    let pin_phases = lock(&decoder).phases().len();

    Ok(SensorbusReport {
        output: buffer,
        hooks,
        events,
        irq_edges: driver
            .engine()
            .irq_edges()
            .iter()
            .map(|(line, high)| (line.0, *high))
            .collect(),
        reason,
        shared_ctrl_meas,
        mux_hidden_nack,
        mux_routed_id,
        bus_phases,
        pin_phases,
    })
}

/// Expected golden console output for the P2 firmware.
pub const GOLDEN_OUTPUT: &[u8] = b"ctl=60 bb=60 spi=60\n";
