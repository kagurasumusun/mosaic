//! P3 heterogeneous AMP scenario: two ARM926EJ-S cores plus one RV32I core
//! cooperate over one host-backed shared page and an MHU doorbell mailbox,
//! scheduled round-robin by the [`MulticoreDriver`] (design §12.3–§12.5).
//!
//! Protocol (every step guest-driven, verified against the Python prototype
//! before porting):
//!
//! 1. core0 stores 0x21 into shared word 0 and rings MHU bank 0, then polls
//!    bank 1 for core1's doorbell;
//! 2. core1 polls bank 0, acknowledges with CLEAR, stores word0 + 0x40 into
//!    shared word 1 and rings bank 1;
//! 3. core0 clears bank 1, prints `smp=61` over the PL011, stores 0x40 into
//!    shared word 2 and rings bank 2 for the RV32I coprocessor;
//! 4. core2 polls bank 2, acknowledges, stores word2 XOR 0xFF into shared
//!    word 3 and rings bank 1 bit 1;
//! 5. core0 clears bank 1 and prints `amp=bf`.
//!
//! The observable outcome (console, shared words, MHU state, per-core IRQ
//! edge history) is identical for every quantum/round shape: determinism is
//! the P3 exit criterion (design §22 T3).

use core::num::NonZeroU64;
use std::sync::{Arc, Mutex};

use mosaic_assemble::{
    bind_flat_view_shared, EngineIrqSink, IrqMailbox, MulticoreDriver, SharedRegion,
};
use mosaic_contract_cpu_engine::{CpuEngine, IrqLine};
use mosaic_contract_model::{ContextError, Ctx};
use mosaic_contract_stream::ByteStream;
use mosaic_core_runtime::{FlatView, Machine, Region};
use mosaic_core_types::{
    AccessKind, AccessSize, Addr, Attr, Endian, EventId, Mapping, MemoryKind, NodeId, Permissions,
    Security, SignalId, VTime,
};
use mosaic_cpu_arm_arm926ej_s::Register as ArmRegister;
use mosaic_cpu_sifive_e31::Register as RvRegister;
use mosaic_engine_unicorn::{SharedRam, UnicornArm, UnicornRiscV32, ARM_IRQ, RISCV_MEXT};
use mosaic_interconnect_uart_pair::UartPair;
use mosaic_ip_arm_mhu::{Mhu, MhuParams, UnmappedPolicy as MhuUnmapped};
use mosaic_ip_arm_pl011::{Pl011, Pl011Params};

/// Primary round shape: small quanta, many rounds.
pub const QUANTUM: u64 = 64;
pub const ROUNDS: usize = 48;
/// Alternate round shape. Observable outcomes must match it exactly.
pub const ALT_QUANTUM: u64 = 512;
pub const ALT_ROUNDS: usize = 6;
/// Events drained per core slice (no timers exist in this scenario; the
/// budget mirrors the single-core driver default).
pub const EVENT_BUDGET: usize = 32;

pub const GOLDEN_CONSOLE: &[u8] = b"smp=61\namp=bf\n";
pub const GOLDEN_SHARED: [u32; 4] = [0x21, 0x61, 0x40, 0xBF];
pub const GOLDEN_MHU_STAT: [u32; 3] = [0, 0, 0];
/// Per-core engine IRQ line history, T = asserted, F = released:
/// core0 sees bank1 (T from core1's SET, F from its own CLEAR, then the
/// pair from core2's response), core1 sees bank0, core2 sees bank2.
pub const GOLDEN_EDGE_STRINGS: [&str; 3] = ["TFTF", "TF", "TF"];

const CPU0: NodeId = NodeId(0);
const CPU1: NodeId = NodeId(1);
const CPU2: NodeId = NodeId(2);
const RAM0: NodeId = NodeId(10);
const RAM1: NodeId = NodeId(11);
const RAM2: NodeId = NodeId(12);
const SHARED: NodeId = NodeId(20);
const UART: NodeId = NodeId(30);
const MHU: NodeId = NodeId(31);
const SINK_C0: NodeId = NodeId(40);
const SINK_C1: NodeId = NodeId(41);
const SINK_C2: NodeId = NodeId(42);
const SINK_UART: NodeId = NodeId(43);

const RAM_BASE: u64 = 0;
const RAM_SIZE: u64 = 0x1000;
const SHARED_BASE: u64 = 0x0001_0000;
const SHARED_SIZE: u64 = 0x1000;
const UART_BASE: u64 = 0x1000_0000;
const MHU_BASE: u64 = 0x1000_1000;
const MMIO_SIZE: u64 = 0x1000;
const STACK_TOP: u64 = 0x800;
const UART_IRQ: SignalId = SignalId(0);

/// Verified firmware words (tools/generate_firmware.py; keystone A32 and
/// hand-encoded RV32I, both capstone-checked). Listings: firmware_a32.S and
/// firmware_rv.S.
const PRIMARY_WORDS: &[u32] = &[
    0xE59F2104, 0xE3A00001, 0xE3800C01, 0xE5820030, 0xE59F20F8, 0xE3A03801, 0xE3A00021, 0xE5830000,
    0xE3A00001, 0xE5820008, 0xE5920100, 0xE3100001, 0x0AFFFFFC, 0xE3A00001, 0xE582010C, 0xE5934004,
    0xE59F20C4, 0xE3A00073, 0xEB000023, 0xE3A0006D, 0xEB000021, 0xE3A00070, 0xEB00001F, 0xE3A0003D,
    0xEB00001D, 0xE1A05004, 0xEB00001D, 0xE3A0000A, 0xEB000019, 0xE59F2094, 0xE3A00040, 0xE5830008,
    0xE3A00001, 0xE5820208, 0xE5920100, 0xE3100002, 0x0AFFFFFC, 0xE3A00002, 0xE582010C, 0xE593400C,
    0xE59F2064, 0xE3A00061, 0xEB00000B, 0xE3A0006D, 0xEB000009, 0xE3A00070, 0xEB000007, 0xE3A0003D,
    0xEB000005, 0xE1A05004, 0xEB000005, 0xE3A0000A, 0xEB000001, 0xEAFFFFFF, 0xEAFFFFFE, 0xE5820000,
    0xE12FFF1E, 0xE52DE004, 0xE1A00225, 0xEB000002, 0xE205000F, 0xEB000000, 0xE49DF004, 0xE350000A,
    0xB2800030, 0xA2800057, 0xEAFFFFF3, 0x10000000, 0x10001000,
];
const SECONDARY_WORDS: &[u32] = &[
    0xE59F2030, 0xE3A03801, 0xE5920000, 0xE3100001, 0x0AFFFFFC, 0xE3A00001, 0xE582000C, 0xE5934000,
    0xE2844040, 0xE5834004, 0xE3A00001, 0xE5820108, 0xEAFFFFFF, 0xEAFFFFFE, 0x10001000,
];
const COP_WORDS: &[u32] = &[
    0x100012B7, 0x000103B7, 0x2002A403, 0x00147413, 0xFE040AE3, 0x00100493, 0x2092A623, 0x0083A503,
    0x0FF54593, 0x00B3A623, 0x00200613, 0x10C2A423, 0x0000006F,
];

/// Everything the golden checks and the report consume.
pub struct MulticoreReport {
    pub console: Vec<u8>,
    pub shared: [u32; 4],
    pub mhu_stat: [u32; 3],
    pub core_edges: [Vec<(IrqLine, bool)>; 3],
    /// Total retired instructions per core across all rounds (shape- and
    /// backend-dependent; printed, never asserted).
    pub hooks: [u64; 3],
    pub events: usize,
    pub now: VTime,
    pub rounds: usize,
}

impl MulticoreReport {
    /// Edge history of every core compressed to T/F characters.
    pub fn edge_strings(&self) -> [String; 3] {
        self.core_edges.clone().map(|edges| {
            edges
                .iter()
                .map(|(_, high)| if *high { 'T' } else { 'F' })
                .collect()
        })
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

fn image(words: &[u32]) -> Vec<u8> {
    words.iter().flat_map(|word| word.to_le_bytes()).collect()
}

fn loader_attr() -> Attr {
    Attr {
        master: CPU0,
        security: Security::NonSecure,
        privileged: false,
        kind: AccessKind::Data,
        exclusive: false,
        byte_enable: u8::MAX,
        endian: Endian::Little,
    }
}

/// Console-side Ctx stub: the harness only drains the UART endpoint.
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

/// Build the whole SoC and run `rounds` round-robin passes of `quantum`
/// instructions per core. Same inputs always produce the same report.
pub fn run_multicore(quantum: NonZeroU64, rounds: usize) -> Result<MulticoreReport, String> {
    // Per-master views: same addresses, different targets (design §12.4).
    let view0 = FlatView::build(&[
        leaf(RAM_BASE, RAM_SIZE, RAM0, MemoryKind::Ram, Permissions::RWX),
        leaf(
            SHARED_BASE,
            SHARED_SIZE,
            SHARED,
            MemoryKind::Ram,
            Permissions::RW,
        ),
        leaf(
            UART_BASE,
            MMIO_SIZE,
            UART,
            MemoryKind::Mmio,
            Permissions::RW,
        ),
        leaf(MHU_BASE, MMIO_SIZE, MHU, MemoryKind::Mmio, Permissions::RW),
    ])
    .map_err(|error| format!("view0: {error:?}"))?;
    let view1 = FlatView::build(&[
        leaf(RAM_BASE, RAM_SIZE, RAM1, MemoryKind::Ram, Permissions::RWX),
        leaf(
            SHARED_BASE,
            SHARED_SIZE,
            SHARED,
            MemoryKind::Ram,
            Permissions::RW,
        ),
        leaf(MHU_BASE, MMIO_SIZE, MHU, MemoryKind::Mmio, Permissions::RW),
    ])
    .map_err(|error| format!("view1: {error:?}"))?;
    let view2 = FlatView::build(&[
        leaf(RAM_BASE, RAM_SIZE, RAM2, MemoryKind::Ram, Permissions::RWX),
        leaf(
            SHARED_BASE,
            SHARED_SIZE,
            SHARED,
            MemoryKind::Ram,
            Permissions::RW,
        ),
        leaf(MHU_BASE, MMIO_SIZE, MHU, MemoryKind::Mmio, Permissions::RW),
    ])
    .map_err(|error| format!("view2: {error:?}"))?;

    let mut machine = Machine::new(view0.clone(), 32);
    machine
        .add_view(CPU1, view1.clone())
        .map_err(|error| format!("view1 add: {error:?}"))?;
    machine
        .add_view(CPU2, view2.clone())
        .map_err(|error| format!("view2 add: {error:?}"))?;
    for (node, size) in [
        (RAM0, RAM_SIZE),
        (RAM1, RAM_SIZE),
        (RAM2, RAM_SIZE),
        (SHARED, SHARED_SIZE),
    ] {
        machine
            .add_backing(node, size)
            .map_err(|error| format!("backing {node:?}: {error:?}"))?;
    }

    let pair = UartPair::new(64);
    let (uart_peer, mut console) = pair.endpoints();
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
        .register_device(MHU, Box::new(Mhu::new(MhuParams::new(MhuUnmapped::RazWi))))
        .map_err(|error| format!("mhu register: {error:?}"))?;

    // One mailbox per core; MHU bank b drives the receiving core's line:
    // bank1 -> core0 (ARM IRQ), bank0 -> core1 (ARM IRQ), bank2 -> core2
    // (RISC-V machine-external). The UART IRQ goes to an unattached mailbox:
    // the firmware never enables UART interrupts, and an unattached mailbox
    // guarantees stray edges cannot pollute core IRQ histories.
    let mailbox0 = Arc::new(IrqMailbox::new());
    let mailbox1 = Arc::new(IrqMailbox::new());
    let mailbox2 = Arc::new(IrqMailbox::new());
    let mailbox_uart = Arc::new(IrqMailbox::new());
    for (sink, line, mailbox) in [
        (SINK_C0, ARM_IRQ, Arc::clone(&mailbox0)),
        (SINK_C1, ARM_IRQ, Arc::clone(&mailbox1)),
        (SINK_C2, RISCV_MEXT, Arc::clone(&mailbox2)),
        (SINK_UART, ARM_IRQ, Arc::clone(&mailbox_uart)),
    ] {
        machine
            .register_sink(sink, Box::new(EngineIrqSink::new(line, mailbox)))
            .map_err(|error| format!("sink {sink:?}: {error:?}"))?;
    }
    for (device, signal, sink) in [
        (MHU, SignalId(1), SINK_C0),
        (MHU, SignalId(0), SINK_C1),
        (MHU, SignalId(2), SINK_C2),
        (UART, UART_IRQ, SINK_UART),
    ] {
        machine
            .connect(device, signal, sink)
            .map_err(|error| format!("wire {device:?}/{signal:?}: {error:?}"))?;
    }
    machine
        .verify()
        .map_err(|error| format!("verify: {error:?}"))?;
    for (node, words) in [
        (RAM0, PRIMARY_WORDS),
        (RAM1, SECONDARY_WORDS),
        (RAM2, COP_WORDS),
    ] {
        machine
            .loader_write(node, 0, &image(words))
            .map_err(|error| format!("load {node:?}: {error:?}"))?;
    }
    let machine = Arc::new(Mutex::new(machine));

    // The shared page is ONE host buffer mapped into all three engines.
    let ram = SharedRam::new(SHARED_SIZE).map_err(|error| format!("shared ram: {error:?}"))?;
    let shared = [SharedRegion {
        node: SHARED,
        ram: Arc::clone(&ram),
    }];

    let mut core0 = UnicornArm::new().map_err(|error| format!("engine0: {error:?}"))?;
    let mut core1 = UnicornArm::new().map_err(|error| format!("engine1: {error:?}"))?;
    let mut core2 = UnicornRiscV32::new().map_err(|error| format!("engine2: {error:?}"))?;
    bind_flat_view_shared(&mut core0, &view0, Arc::clone(&machine), CPU0, &shared)
        .map_err(|error| format!("bind0: {error:?}"))?;
    bind_flat_view_shared(&mut core1, &view1, Arc::clone(&machine), CPU1, &shared)
        .map_err(|error| format!("bind1: {error:?}"))?;
    bind_flat_view_shared(&mut core2, &view2, Arc::clone(&machine), CPU2, &shared)
        .map_err(|error| format!("bind2: {error:?}"))?;
    for engine in [&mut core0, &mut core1] {
        engine
            .reg_write(ArmRegister::Pc.into(), RAM_BASE)
            .map_err(|error| format!("pc: {error:?}"))?;
        engine
            .reg_write(ArmRegister::Sp.into(), STACK_TOP)
            .map_err(|error| format!("sp: {error:?}"))?;
    }
    core2
        .reg_write(RvRegister::Pc.into(), RAM_BASE)
        .map_err(|error| format!("rv pc: {error:?}"))?;

    let mut driver = MulticoreDriver::new(machine);
    driver.add_core(Box::new(core0));
    driver.add_core(Box::new(core1));
    driver.add_core(Box::new(core2));
    for (index, mailbox) in [
        (0, Arc::clone(&mailbox0)),
        (1, Arc::clone(&mailbox1)),
        (2, Arc::clone(&mailbox2)),
    ] {
        driver
            .attach_irq(index, mailbox)
            .map_err(|error| format!("attach_irq {index}: {error:?}"))?;
    }

    let reports = driver
        .run(rounds, quantum, EVENT_BUDGET)
        .map_err(|error| format!("run: {error:?}"))?;
    let mut hooks = [0u64; 3];
    let mut events = 0usize;
    let mut now = VTime::ZERO;
    for report in &reports {
        for (index, core) in report.cores.iter().enumerate() {
            hooks[index] += core.engine.instruction_hooks;
            events += core.events_delivered;
        }
        now = report.now;
    }
    let core_edges = [0usize, 1, 2].map(|index| {
        driver
            .core(index)
            .map(|engine| engine.line_edges().to_vec())
            .unwrap_or_default()
    });

    let mut shared_bytes = [0u8; 16];
    ram.read(0, &mut shared_bytes)
        .map_err(|error| format!("shared read: {error:?}"))?;
    let mut shared_words = [0u32; 4];
    for (slot, chunk) in shared_words.iter_mut().zip(shared_bytes.chunks_exact(4)) {
        *slot = u32::from_le_bytes(chunk.try_into().expect("four-byte chunk"));
    }

    let mut mhu_stat = [0u32; 3];
    {
        let machine = driver.machine();
        let mut machine = machine
            .lock()
            .map_err(|_| "machine mutex poisoned".to_string())?;
        for (bank, slot) in mhu_stat.iter_mut().enumerate() {
            let value = machine
                .bus_read(
                    Addr(MHU_BASE + 0x100 * bank as u64),
                    AccessSize::Word,
                    &loader_attr(),
                )
                .map_err(|error| format!("mhu stat {bank}: {error:?}"))?;
            *slot = value as u32;
        }
    }

    let mut console_bytes = Vec::new();
    let mut buffer = [0u8; 64];
    loop {
        let read = console
            .read_bytes(&mut CaptureCtx, &mut buffer)
            .map_err(|error| format!("console read: {error:?}"))?;
        if read == 0 {
            break;
        }
        console_bytes.extend_from_slice(&buffer[..read]);
        if console_bytes.len() > 4096 {
            return Err("console overflow".to_string());
        }
    }

    Ok(MulticoreReport {
        console: console_bytes,
        shared: shared_words,
        mhu_stat,
        core_edges,
        hooks,
        events,
        now,
        rounds: reports.len(),
    })
}
